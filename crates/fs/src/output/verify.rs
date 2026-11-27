//! Checking the output against the source after writing, and `wgc verify`.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use rayon::prelude::*;

use super::copy::{Written, WrittenFile};
use super::record::{ChangedFile, Complete, SourceState};
use super::{OutputError, RECORD_DIR, VerifyFailure, destination, io_err, is_excluded};
use crate::Outcome;
use crate::layout::WorldFiles;
use crate::lock::{LockError, SessionLock};
use crate::plan::{ExternalAction, FileAction, Plan};
use crate::types::{Observer, Progress, Warning};

/// Checks V-1..V-6. All failures are collected.
pub(crate) fn check_output(
    plan: &Plan,
    walked: &WorldFiles,
    written: &Written,
    out: &Path,
    state: &SourceState,
    observer: &dyn Observer<Warning>,
    cancel: &AtomicBool,
) -> Result<(), OutputError> {
    // Counting and reporting under one lock keeps `done` increasing for the observer.
    let total = written.files.len() as u64;
    let done = Mutex::new(0u64);
    observer.progress(Progress::Verifying { done: 0, total });
    let per_file: Vec<Vec<VerifyFailure>> = written
        .files
        .par_iter()
        .map(|(dest, file)| {
            if cancel.load(Ordering::Relaxed) {
                return Err(OutputError::Cancelled);
            }
            let failures = check_file(plan, dest, file)?;
            let mut n = done.lock().unwrap_or_else(PoisonError::into_inner);
            *n += 1;
            observer.progress(Progress::Verifying { done: *n, total });
            Ok(failures)
        })
        .collect::<Result<_, OutputError>>()?;
    let mut failures: Vec<VerifyFailure> = per_file.into_iter().flatten().collect();

    // V-5: compare the output with what the plan requires, derived here
    // independently of what the writer chose to write.
    let expected = expected_outputs(plan, walked, out);
    let found = crate::layout::files_in(std::slice::from_ref(&out.to_path_buf()))?;
    let record_dir = out.join(RECORD_DIR);
    let found_paths: BTreeSet<&PathBuf> = found
        .files
        .iter()
        .map(|f| &f.path)
        .filter(|p| !p.starts_with(&record_dir))
        .collect();
    for path in found_paths.difference(&expected.iter().collect()) {
        failures.push(VerifyFailure {
            check: "V-5",
            path: (*path).clone(),
            detail: "unexpected file".to_owned(),
        });
    }
    for path in &expected {
        if !found_paths.contains(path) {
            failures.push(VerifyFailure {
                check: "V-5",
                path: path.clone(),
                detail: "missing file".to_owned(),
            });
        }
    }

    // V-6: the source did not change while we worked.
    let now = state.recapture()?;
    for change in state.diff(&now) {
        let (ChangedFile::Added(p) | ChangedFile::Removed(p) | ChangedFile::Modified(p)) = &change;
        failures.push(VerifyFailure {
            check: "V-6",
            path: plan.layout.root.join(p),
            detail: format!("{change:?}"),
        });
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(OutputError::Verification(failures))
    }
}

/// V-1..V-4 for one written file.
fn check_file(
    plan: &Plan,
    dest: &Path,
    file: &WrittenFile,
) -> Result<Vec<VerifyFailure>, OutputError> {
    let mut failures = Vec::new();
    let fail = |check, detail: String| VerifyFailure {
        check,
        path: dest.to_path_buf(),
        detail,
    };
    let Some(r) = &file.rewrite else {
        // V-4: copies have the source's size.
        let a = fs::metadata(&file.source)
            .map_err(io_err(&file.source))?
            .len();
        let b = fs::metadata(dest).map_err(io_err(dest))?.len();
        if a != b {
            failures.push(fail("V-4", format!("size {b}, expected {a}")));
        }
        return Ok(failures);
    };
    let t = &plan.dimensions[r.dim].triples[r.triple];
    let Some(FileAction::Rewrite { keep }) = t.action(r.kind) else {
        failures.push(fail(
            "V-2",
            "file was not planned to be rewritten".to_owned(),
        ));
        return Ok(failures);
    };
    let source = t.parse_file(
        r.kind,
        fs::read(&file.source).map_err(io_err(&file.source))?,
    );
    let output = t.parse_file(r.kind, fs::read(dest).map_err(io_err(dest))?);
    // V-1: no structural issues.
    if !output.issues().is_empty() {
        failures.push(fail("V-1", format!("{:?}", output.issues())));
    }
    // V-2: exactly the kept entries.
    let indices: Vec<_> = output.entries().iter().map(|e| e.index).collect();
    if indices != keep {
        failures.push(fail(
            "V-2",
            format!("entries {indices:?}, expected {keep:?}"),
        ));
    }
    // V-3: identical payloads and timestamps.
    for index in &keep {
        let same_payload = output.payload(*index) == source.payload(*index);
        let same_time =
            output.entry(*index).map(|e| e.timestamp) == source.entry(*index).map(|e| e.timestamp);
        if !same_payload || !same_time {
            failures.push(fail("V-3", format!("chunk {} differs", index.get())));
        }
    }
    Ok(failures)
}

/// Destinations of every source file that the plan keeps.
fn expected_outputs(plan: &Plan, walked: &WorldFiles, out: &Path) -> BTreeSet<PathBuf> {
    let layout = &plan.layout;
    let mut omitted: BTreeSet<&Path> = plan
        .omitted_other_files
        .iter()
        .map(|o| o.path.as_path())
        .collect();
    for dim in &plan.dimensions {
        for t in &dim.triples {
            for kind in crate::types::FileKind::ALL {
                let Some(file) = t.file(kind) else {
                    continue;
                };
                if matches!(t.action(kind), Some(FileAction::Omit(_))) {
                    omitted.insert(&file.path);
                }
                for x in &file.externals {
                    if t.external_action(kind, x.pos) == ExternalAction::Omit {
                        omitted.insert(&x.path);
                    }
                }
            }
        }
    }
    walked
        .files
        .iter()
        .filter(|f| !is_excluded(layout, &f.path) && !omitted.contains(f.path.as_path()))
        .map(|f| destination(layout, out, &f.path))
        .collect()
}

/// Result of a successful `wgc verify`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyOutcome {
    /// The original world.
    pub source: PathBuf,
    /// Checks that `gc --out` ignored with `--force`.
    pub forced: Vec<String>,
    /// Layout kind as in `source-state.json` (`vanilla_legacy`, ...).
    pub layout: String,
    /// World folders relative to `source`.
    pub world_folders: Vec<String>,
}

/// Why the output cannot be swapped in.
#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("{out} has no complete.json")]
    Incomplete { out: PathBuf },
    #[error("output was made from {recorded}, not {given}")]
    SourceMismatch { recorded: PathBuf, given: PathBuf },
    #[error("the world changed since the output was made")]
    Changed(Vec<ChangedFile>),
    #[error("{path} is locked")]
    InUse { path: PathBuf },
    #[error("cannot access {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl VerifyError {
    /// How a `wgc verify` that ended with this error ended.
    pub const fn outcome(&self) -> Outcome {
        match self {
            VerifyError::Io { .. } => Outcome::Failed,
            _ => Outcome::NotReady,
        }
    }

    /// The result as `wgc.verify/1` names it.
    const fn result_name(&self) -> &'static str {
        match self {
            VerifyError::Incomplete { .. } => "incomplete",
            VerifyError::SourceMismatch { .. } => "source_mismatch",
            VerifyError::Changed(_) => "changed",
            VerifyError::InUse { .. } => "in_use",
            VerifyError::Io { .. } => "error",
        }
    }
}

/// The `wgc verify` report (`wgc.verify/1`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct VerifyJson {
    schema: &'static str,
    result: &'static str,
    source: Option<String>,
    changed: Vec<String>,
    forced: Vec<String>,
}

impl VerifyJson {
    pub fn new(result: &Result<VerifyOutcome, VerifyError>) -> Self {
        let (name, source, forced) = match result {
            Ok(ok) => (
                "ready",
                Some(ok.source.display().to_string()),
                ok.forced.clone(),
            ),
            Err(e) => (e.result_name(), None, Vec::new()),
        };
        let changed = match result {
            Err(VerifyError::Changed(changed)) => changed
                .iter()
                .map(|c| match c {
                    ChangedFile::Added(p) | ChangedFile::Removed(p) | ChangedFile::Modified(p) => {
                        p.clone()
                    }
                })
                .collect(),
            _ => Vec::new(),
        };
        VerifyJson {
            schema: "wgc.verify/1",
            result: name,
            source,
            changed,
            forced,
        }
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, VerifyError> {
    let bytes = fs::read(path).map_err(|source| VerifyError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_slice(&bytes).map_err(|e| VerifyError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, e),
    })
}

/// `wgc verify`: whether `out` can replace the original world now.
pub fn verify(out: &Path, source: Option<&Path>) -> Result<VerifyOutcome, VerifyError> {
    let record = out.join(RECORD_DIR);
    if !record.join("complete.json").is_file() || record.join("failure.json").exists() {
        return Err(VerifyError::Incomplete {
            out: out.to_path_buf(),
        });
    }
    let complete: Complete = read_json(&record.join("complete.json"))?;
    let state: SourceState = read_json(&record.join("source-state.json"))?;
    let recorded = PathBuf::from(&state.source);
    if let Some(given) = source {
        let canonical = fs::canonicalize(given).map_err(|source| VerifyError::Io {
            path: given.to_path_buf(),
            source,
        })?;
        let recorded_canonical = fs::canonicalize(&recorded).unwrap_or_else(|_| recorded.clone());
        if canonical != recorded_canonical {
            return Err(VerifyError::SourceMismatch {
                recorded,
                given: canonical,
            });
        }
    }

    let folders = state.world_folder_paths();
    let now = state.recapture().map_err(|e| VerifyError::Io {
        path: recorded.clone(),
        source: std::io::Error::other(e),
    })?;
    let changed = state.diff(&now);
    if !changed.is_empty() {
        return Err(VerifyError::Changed(changed));
    }

    for folder in &folders {
        match SessionLock::is_locked(folder) {
            Ok(false) => {}
            Ok(true) => {
                return Err(VerifyError::InUse {
                    path: folder.join(crate::lock::LOCK_FILE),
                });
            }
            Err(LockError::Io { path, source }) => return Err(VerifyError::Io { path, source }),
            Err(LockError::NotWritable { path } | LockError::HeldByThisProcess { path }) => {
                return Err(VerifyError::InUse { path });
            }
            Err(LockError::InUse { path }) => return Err(VerifyError::InUse { path }),
            Err(LockError::Missing { .. }) => {}
        }
    }
    Ok(VerifyOutcome {
        source: recorded,
        forced: complete.forced,
        layout: state.layout,
        world_folders: state.world_folders,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(result: &Result<VerifyOutcome, VerifyError>) -> String {
        serde_json::to_string(&VerifyJson::new(result)).unwrap()
    }

    #[test]
    fn verify_json_golden() {
        let ready = Ok(VerifyOutcome {
            source: PathBuf::from("/abs/server"),
            forced: vec!["T-1".to_owned(), "L-1".to_owned()],
            layout: "bukkit".to_owned(),
            world_folders: vec!["world".to_owned()],
        });
        assert_eq!(
            json(&ready),
            concat!(
                r#"{"schema":"wgc.verify/1","result":"ready","source":"/abs/server","#,
                r#""changed":[],"forced":["T-1","L-1"]}"#,
            )
        );

        let changed = Err(VerifyError::Changed(vec![
            ChangedFile::Modified("world/level.dat".to_owned()),
            ChangedFile::Added("world/region/r.0.0.mca".to_owned()),
        ]));
        assert_eq!(
            json(&changed),
            concat!(
                r#"{"schema":"wgc.verify/1","result":"changed","source":null,"#,
                r#""changed":["world/level.dat","world/region/r.0.0.mca"],"forced":[]}"#,
            )
        );
    }
}
