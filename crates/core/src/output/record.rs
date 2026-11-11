//! The `.wgc/` records in the output folder.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::{OutputError, RECORD_DIR, io_err, sync_dir};
use crate::layout::{WorldFiles, WorldLayout};
use crate::plan::Plan;
use crate::report::{ReportError, rel_path};
use crate::types::GuardId;

/// The source world when `gc --out` started (`source-state.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceState {
    pub schema: String,
    /// Canonical absolute input path.
    pub source: String,
    pub layout: String,
    /// Relative to `source`.
    pub world_folders: Vec<String>,
    pub files: Vec<SourceStateFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SourceStateFile {
    pub path: String,
    pub size: u64,
    pub mtime_ns: i64,
}

/// The completion marker (`complete.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Complete {
    pub schema: String,
    pub tool_version: String,
    pub finished_at: String,
    pub source: String,
    pub summary: Summary,
    pub forced: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub deleted_chunks: u64,
    pub size_before: u64,
    pub size_after: u64,
}

/// What went wrong (`failure.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub schema: String,
    pub tool_version: String,
    pub stage: String,
    pub error: String,
    pub items: Vec<FailureItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureItem {
    /// The failed verification check, as in [`VerifyFailure::check`](super::VerifyFailure::check).
    pub check: String,
    pub path: String,
    pub detail: String,
}

/// A file of the source world that differs from `source-state.json`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChangedFile {
    Added(String),
    Removed(String),
    Modified(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    Write,
    Verify,
    Record,
}

impl Stage {
    fn as_str(self) -> &'static str {
        match self {
            Stage::Write => "write",
            Stage::Verify => "verify",
            Stage::Record => "record",
        }
    }
}

fn report_err(e: ReportError) -> OutputError {
    match e {
        ReportError::NonUtf8Path(path) => OutputError::Io {
            path,
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, "path is not valid utf-8"),
        },
        ReportError::Plan(p) => OutputError::from(p),
    }
}

pub(crate) fn mtime_ns(meta: &fs::Metadata) -> i64 {
    match meta.modified() {
        Ok(t) => match t.duration_since(UNIX_EPOCH) {
            Ok(d) => i64::try_from(d.as_nanos()).unwrap_or(i64::MAX),
            Err(e) => -i64::try_from(e.duration().as_nanos()).unwrap_or(i64::MAX),
        },
        Err(_) => 0,
    }
}

impl SourceState {
    /// Stats every file of the world folders (including `session.lock`).
    pub(crate) fn capture(
        layout: &WorldLayout,
        walked: &WorldFiles,
    ) -> Result<SourceState, OutputError> {
        let root = &layout.root;
        Ok(SourceState {
            schema: "wgc.source-state/1".to_owned(),
            source: root
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| report_err(ReportError::NonUtf8Path(root.clone())))?,
            layout: layout.kind.as_str().to_owned(),
            world_folders: layout
                .world_folders
                .iter()
                .map(|f| rel_path(root, f))
                .collect::<Result<_, _>>()
                .map_err(report_err)?,
            files: stat_files(root, walked)?,
        })
    }

    /// The same record, with the files as they are now.
    pub(crate) fn recapture(&self) -> Result<SourceState, OutputError> {
        let walked = crate::layout::files_in(&self.world_folder_paths())?;
        Ok(SourceState {
            files: stat_files(Path::new(&self.source), &walked)?,
            ..self.clone()
        })
    }

    /// Absolute world folders.
    pub fn world_folder_paths(&self) -> Vec<PathBuf> {
        let source = Path::new(&self.source);
        self.world_folders
            .iter()
            .map(|f| {
                if f == "." {
                    source.to_path_buf()
                } else {
                    source.join(f)
                }
            })
            .collect()
    }

    /// Differences between this record and `now`.
    pub fn diff(&self, now: &SourceState) -> Vec<ChangedFile> {
        let before: std::collections::BTreeMap<&str, &SourceStateFile> =
            self.files.iter().map(|f| (f.path.as_str(), f)).collect();
        let after: std::collections::BTreeMap<&str, &SourceStateFile> =
            now.files.iter().map(|f| (f.path.as_str(), f)).collect();
        let mut changed = Vec::new();
        for (path, f) in &before {
            match after.get(path) {
                None => changed.push(ChangedFile::Removed((*path).to_owned())),
                Some(g) if g.size != f.size || g.mtime_ns != f.mtime_ns => {
                    changed.push(ChangedFile::Modified((*path).to_owned()));
                }
                Some(_) => {}
            }
        }
        for path in after.keys() {
            if !before.contains_key(path) {
                changed.push(ChangedFile::Added((*path).to_owned()));
            }
        }
        changed.sort();
        changed
    }
}

/// Path, size and modification time of every walked file, sorted.
fn stat_files(root: &Path, walked: &WorldFiles) -> Result<Vec<SourceStateFile>, OutputError> {
    let mut files = Vec::new();
    for f in &walked.files {
        let meta = fs::metadata(&f.path).map_err(io_err(&f.path))?;
        files.push(SourceStateFile {
            path: rel_path(root, &f.path).map_err(report_err)?,
            size: meta.len(),
            mtime_ns: mtime_ns(&meta),
        });
    }
    files.sort();
    Ok(files)
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), OutputError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| OutputError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::other(e),
    })?;
    let mut f = File::create(path).map_err(io_err(path))?;
    f.write_all(&bytes).map_err(io_err(path))?;
    f.sync_all().map_err(io_err(path))
}

/// Writes `source-state.json`, then `complete.json` last.
pub(crate) fn write_records(
    out: &Path,
    state: &SourceState,
    plan: &Plan,
    forced: &BTreeSet<GuardId>,
) -> Result<(), OutputError> {
    let dir = out.join(RECORD_DIR);
    fs::create_dir_all(&dir).map_err(io_err(&dir))?;
    write_json(&dir.join("source-state.json"), state)?;

    let mut summary = Summary {
        deleted_chunks: 0,
        size_before: 0,
        size_after: 0,
    };
    for dim in &plan.dimensions {
        for t in &dim.triples {
            summary.deleted_chunks += t
                .chunks
                .iter()
                .filter(|c| c.decision == crate::plan::Decision::Delete)
                .count() as u64;
        }
    }
    for sizes in plan.estimate.dimensions.values() {
        for s in [sizes.region, sizes.entities, sizes.poi] {
            summary.size_before += s.before;
            summary.size_after += s.after;
        }
    }
    summary.size_before += plan.estimate.other_files_bytes;
    summary.size_after += plan.estimate.other_files_bytes;

    // Everything else is durable before the marker appears.
    sync_dir(&dir)?;
    sync_dir(out)?;
    let tmp = dir.join("complete.json.tmp");
    write_json(
        &tmp,
        &Complete {
            schema: "wgc.complete/1".to_owned(),
            tool_version: crate::TOOL_VERSION.to_owned(),
            finished_at: rfc3339(SystemTime::now()),
            source: state.source.clone(),
            summary,
            forced: forced.iter().map(|g| g.as_str().to_owned()).collect(),
        },
    )?;
    // The rename makes the marker appear complete or not at all.
    let complete = dir.join("complete.json");
    fs::rename(&tmp, &complete).map_err(io_err(&complete))?;
    if let Err(e) = sync_dir(&dir) {
        // Not durable: do not leave a marker for a run that reports failure.
        let _ = fs::remove_file(&complete);
        return Err(e);
    }
    Ok(())
}

/// Writes `failure.json` describing why the output is incomplete.
pub(crate) fn write_failure(
    out: &Path,
    stage: Stage,
    error: &OutputError,
) -> Result<(), OutputError> {
    let dir = out.join(RECORD_DIR);
    fs::create_dir_all(&dir).map_err(io_err(&dir))?;
    let items = match error {
        OutputError::Verification(failures) => failures
            .iter()
            .map(|f| FailureItem {
                check: f.check.to_owned(),
                path: rel_path(out, &f.path)
                    .unwrap_or_else(|_| f.path.to_string_lossy().into_owned()),
                detail: f.detail.clone(),
            })
            .collect(),
        _ => Vec::new(),
    };
    let error_text = {
        let mut text = error.to_string();
        let mut source = std::error::Error::source(error);
        while let Some(s) = source {
            text.push_str(": ");
            text.push_str(&s.to_string());
            source = s.source();
        }
        text
    };
    write_json(
        &dir.join("failure.json"),
        &Failure {
            schema: "wgc.failure/1".to_owned(),
            tool_version: crate::TOOL_VERSION.to_owned(),
            stage: stage.as_str().to_owned(),
            error: error_text,
            items,
        },
    )
}

/// `YYYY-MM-DDTHH:MM:SSZ` in UTC.
pub(crate) fn rfc3339(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn rfc3339_dates() {
        assert_eq!(rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(
            rfc3339(UNIX_EPOCH + Duration::from_secs(1_794_312_896)),
            "2026-11-10T12:14:56Z"
        );
        assert_eq!(
            rfc3339(UNIX_EPOCH + Duration::from_secs(951_782_400)),
            "2000-02-29T00:00:00Z"
        );
    }
}
