//! Writing the result to a new folder and verifying it (docs/spec/output.md).
//!
//! The source world is only read. The one exception is opening
//! `session.lock` for writing to lock it; its content is never changed.

mod copy;
mod record;
mod verify;

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::layout::{LayoutError, WorldLayout};
use crate::lock::{LockError, SessionLock};
use crate::plan::{Plan, PlanError, PlanOptions, build_plan};
use crate::trust::{TrustReport, check_trust};
use crate::types::{DimensionId, GuardId, Observer, Warning};
use crate::versions::DV_SESSION_LOCK_OS;

pub use record::{ChangedFile, Complete, Failure, FailureItem, SourceState, SourceStateFile};
pub use verify::{VerifyError, VerifyOutcome, verify};

/// Name of the record folder in the output.
pub const RECORD_DIR: &str = ".wgc";

/// Options for [`run_gc`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GcOptions {
    /// New or empty output folder.
    pub out: PathBuf,
    pub gc_dims: Option<BTreeSet<DimensionId>>,
    /// Stops that `--force` allows to continue past.
    pub allowed: BTreeSet<GuardId>,
}

/// What a successful `gc --out` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GcOutcome {
    pub plan: Plan,
    pub trust: TrustReport,
    /// Stops that were ignored with `--force`.
    pub forced: BTreeSet<GuardId>,
    /// The output folder, canonicalized.
    pub out: PathBuf,
    /// Whether some output files could not get their original owner.
    pub owner_not_preserved: bool,
    /// Whether some world folder was not locked because `session.lock` was
    /// missing and L-1 was forced.
    pub lock_skipped: bool,
}

/// One failed verification check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyFailure {
    /// `V-1` .. `V-6`.
    pub check: &'static str,
    pub path: PathBuf,
    pub detail: String,
}

/// Why `gc --out` stopped or failed.
#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    #[error("{path} is not empty")]
    OutputNotEmpty { path: PathBuf },
    /// The part of the output path that does not exist yet contains `..`.
    #[error("{path} must not contain '..' after its existing part")]
    InvalidOutputPath { path: PathBuf },
    #[error("{out} overlaps the world at {world}")]
    OutputOverlapsSource { out: PathBuf, world: PathBuf },
    #[error(transparent)]
    Lock(LockError),
    /// Stopped by trust checks or L-1 that `--force` did not allow.
    #[error("stopped by {0:?}")]
    Stopped(Vec<GuardId>),
    #[error("not enough free space: {needed} bytes needed, {available} available")]
    InsufficientSpace { needed: u64, available: u64 },
    #[error(transparent)]
    Layout(#[from] LayoutError),
    #[error(transparent)]
    Plan(PlanError),
    #[error("verification failed")]
    Verification(Vec<VerifyFailure>),
    #[error("cannot access {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cancelled")]
    Cancelled,
}

impl From<PlanError> for OutputError {
    fn from(e: PlanError) -> Self {
        match e {
            PlanError::Cancelled => OutputError::Cancelled,
            other => OutputError::Plan(other),
        }
    }
}

pub(crate) fn io_err(path: &Path) -> impl FnOnce(io::Error) -> OutputError + '_ {
    move |source| OutputError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Extra space required on top of the estimate.
const SPACE_MARGIN: u64 = 64 * 1024 * 1024;

/// Runs `gc --out` (docs/spec/output.md "実行の順序").
pub fn run_gc(
    layout: &WorldLayout,
    options: &GcOptions,
    observer: &dyn Observer,
    cancel: &AtomicBool,
) -> Result<GcOutcome, OutputError> {
    // 1. Output folder.
    let out = check_output(layout, &options.out)?;

    // 2. Locks.
    let mut forced = BTreeSet::new();
    let mut stops = Vec::new();
    let mut locks = Vec::new();
    let mut folders: Vec<PathBuf> = layout
        .world_folders
        .iter()
        .map(|f| fs::canonicalize(f).unwrap_or_else(|_| f.clone()))
        .collect();
    folders.sort();
    folders.dedup();
    for folder in &folders {
        match SessionLock::try_acquire(folder) {
            Ok(lock) => locks.push(lock),
            Err(LockError::Missing { .. }) => {
                if options.allowed.contains(&GuardId::L1) {
                    if forced.insert(GuardId::L1) {
                        observer.warn(Warning::LockSkipped);
                    }
                } else if !stops.contains(&GuardId::L1) {
                    stops.push(GuardId::L1);
                }
            }
            Err(e) => return Err(OutputError::Lock(e)),
        }
    }
    if let Some(v) = layout.data_version
        && v < DV_SESSION_LOCK_OS
    {
        observer.warn(Warning::PreLockVersion(v));
    }

    // 3. Source state, under the lock.
    let walked = crate::layout::world_files(layout)?;
    let state = record::SourceState::capture(layout, &walked)?;

    // 4. Plan.
    let plan = build_plan(
        layout,
        &PlanOptions {
            gc_dims: options.gc_dims.clone(),
        },
        observer,
        cancel,
    )?;

    // 5. Trust.
    let trust = check_trust(&plan);
    for id in trust.stops() {
        if options.allowed.contains(&id) {
            forced.insert(id);
            observer.warn(Warning::Forced(id));
        } else {
            stops.push(id);
        }
    }
    if !stops.is_empty() {
        stops.sort();
        return Err(OutputError::Stopped(stops));
    }

    // 6. Free space.
    let needed = plan
        .estimate
        .total_output_bytes
        .saturating_add(plan.estimate.total_output_bytes / 100)
        + SPACE_MARGIN;
    let probe = existing_ancestor(&out);
    let available = crate::sys::available_space(&probe).map_err(io_err(&probe))?;
    if needed > available {
        return Err(OutputError::InsufficientSpace { needed, available });
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(OutputError::Cancelled);
    }

    // 7-9. Write, verify, record. A failure leaves `failure.json` behind.
    fs::create_dir_all(&out).map_err(io_err(&out))?;
    let result = (|| {
        let written = copy::write_output(&plan, &walked, &out, observer, cancel)
            .map_err(|e| (record::Stage::Write, e))?;
        verify::check_output(&plan, &walked, &written, &out, &state, observer, cancel)
            .map_err(|e| (record::Stage::Verify, e))?;
        record::write_records(&out, &state, &plan, &forced)
            .map_err(|e| (record::Stage::Record, e))?;
        Ok::<_, (record::Stage, OutputError)>(written)
    })();
    let written = match result {
        Ok(written) => written,
        Err((_, OutputError::Cancelled)) => return Err(OutputError::Cancelled),
        Err((stage, e)) => {
            // Best effort: the missing complete.json already marks the output as incomplete.
            let _ = record::write_failure(&out, stage, &e);
            return Err(e);
        }
    };
    drop(locks);
    let lock_skipped = forced.contains(&GuardId::L1);
    Ok(GcOutcome {
        lock_skipped,
        plan,
        trust,
        forced,
        out,
        owner_not_preserved: written.owner_not_preserved,
    })
}

/// Checks that `out` is new or empty and does not overlap the world.
/// Returns its absolute form (canonical as far as it exists).
fn check_output(layout: &WorldLayout, out: &Path) -> Result<PathBuf, OutputError> {
    let absolute = std::path::absolute(out).map_err(io_err(out))?;
    let base = existing_ancestor(&absolute);
    let canonical_base = fs::canonicalize(&base).map_err(io_err(&base))?;
    let rest = absolute.strip_prefix(&base).unwrap_or(Path::new(""));
    // `..` in a part that does not exist yet would be resolved only when it is
    // created, so the overlap check below could not see where it leads.
    if rest.components().any(|c| {
        !matches!(
            c,
            std::path::Component::Normal(_) | std::path::Component::CurDir
        )
    }) {
        return Err(OutputError::InvalidOutputPath { path: absolute });
    }
    let resolved = canonical_base.join(rest);

    match fs::read_dir(&resolved) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                return Err(OutputError::OutputNotEmpty { path: resolved });
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(_) if resolved.exists() && !resolved.is_dir() => {
            return Err(OutputError::OutputNotEmpty { path: resolved });
        }
        Err(e) => return Err(io_err(&resolved)(e)),
    }

    for source in &layout.world_folders {
        let source = fs::canonicalize(source).map_err(io_err(source))?;
        if resolved.starts_with(&source) || source.starts_with(&resolved) {
            return Err(OutputError::OutputOverlapsSource {
                out: resolved,
                world: source,
            });
        }
    }
    Ok(resolved)
}

/// The deepest existing ancestor of `path` (or `path` itself).
fn existing_ancestor(path: &Path) -> PathBuf {
    path.ancestors()
        .find(|p| p.exists())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Files of the source that never go to the output: `session.lock` at a
/// world folder root and the `.wgc/` records of an earlier run.
pub(crate) fn is_excluded(layout: &WorldLayout, path: &Path) -> bool {
    layout.world_folders.iter().any(|folder| {
        path == folder.join(crate::lock::LOCK_FILE) || path.starts_with(folder.join(RECORD_DIR))
    })
}

/// Makes the entries of `dir` durable.
pub(crate) fn sync_dir(dir: &Path) -> Result<(), OutputError> {
    #[cfg(unix)]
    fs::File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(io_err(dir))?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Where `source` (inside the world) goes in the output.
pub(crate) fn destination(layout: &WorldLayout, out: &Path, source: &Path) -> PathBuf {
    match source.strip_prefix(&layout.root) {
        Ok(rel) => out.join(rel),
        Err(_) => out.join(source.file_name().unwrap_or_default()),
    }
}

#[cfg(test)]
mod tests;
