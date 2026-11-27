//! Writing the result to a new folder and verifying it.
//!
//! The source world is only read. The one exception is opening
//! `session.lock` for writing to lock it; its content is never changed.

pub(crate) mod copy;
pub(crate) mod record;
pub(crate) mod verify;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use wgc::gc::GcError;

use crate::Outcome;
use crate::layout::{LayoutError, WorldLayout};
use crate::lock::LockError;
use crate::plan::PlanError;
use crate::types::{GuardId, Observer, Warning};
use crate::world::{FsWorld, Rayon};

pub use record::{ChangedFile, Complete, Failure, FailureItem, SourceState, SourceStateFile};
pub use verify::{VerifyError, VerifyJson, VerifyOutcome, verify};

/// Name of the record folder in the output.
pub const RECORD_DIR: &str = ".wgc";

/// Options for [`run_gc`].
pub type GcOptions = wgc::gc::GcOptions<PathBuf>;

/// What a successful `gc --out` did. `out` is the output folder, canonicalized.
pub type GcOutcome = wgc::gc::GcOutcome<PathBuf, PathBuf>;

/// One failed verification check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyFailure {
    /// The check that failed:
    ///
    /// - `V-1`: a rewritten region file has structural issues.
    /// - `V-2`: a rewritten file's entries differ from those the plan keeps.
    /// - `V-3`: a kept entry's payload or timestamp differs from the source.
    /// - `V-4`: a copied file's size differs from the source.
    /// - `V-5`: the output's files differ from those the plan requires
    ///   (`.wgc/` aside).
    /// - `V-6`: the source world changed since its state was recorded.
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

impl OutputError {
    /// How a `gc --out` that ended with this error ended.
    pub const fn outcome(&self) -> Outcome {
        match self {
            OutputError::OutputNotEmpty { .. }
            | OutputError::InvalidOutputPath { .. }
            | OutputError::OutputOverlapsSource { .. }
            | OutputError::Stopped(_)
            | OutputError::InsufficientSpace { .. } => Outcome::Stopped,
            OutputError::Lock(LockError::Io { .. }) => Outcome::Failed,
            OutputError::Lock(_) => Outcome::Stopped,
            OutputError::Verification(_) => Outcome::NotReady,
            OutputError::Cancelled => Outcome::Interrupted,
            OutputError::Layout(_) | OutputError::Plan(_) | OutputError::Io { .. } => {
                Outcome::Failed
            }
        }
    }
}

pub(crate) fn io_err(path: &Path) -> impl FnOnce(io::Error) -> OutputError + '_ {
    move |source| OutputError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Runs `gc --out`. Every stop that `--force` did not allow is found before
/// the output folder is created and before the plan reads every region file.
pub fn run_gc(
    layout: &WorldLayout,
    options: &GcOptions,
    observer: &dyn Observer<Warning>,
    cancel: &AtomicBool,
) -> Result<GcOutcome, OutputError> {
    wgc::gc::run_gc(&FsWorld::new(layout), &Rayon, options, observer, cancel).map_err(|e| match e {
        GcError::Stopped(ids) => OutputError::Stopped(ids),
        GcError::InsufficientSpace { needed, available } => {
            OutputError::InsufficientSpace { needed, available }
        }
        GcError::Cancelled => OutputError::Cancelled,
        GcError::Host(e) => e,
    })
}

/// Checks that `out` is new or empty and does not overlap the world.
/// Returns its absolute form (canonical as far as it exists).
pub(crate) fn check_output(layout: &WorldLayout, out: &Path) -> Result<PathBuf, OutputError> {
    let (base, rest) = split_existing(out);
    // `..` in a part that does not exist yet would be resolved only when it is
    // created, so the overlap check below could not see where it leads.
    if rest.components().any(|c| {
        !matches!(
            c,
            std::path::Component::Normal(_) | std::path::Component::CurDir
        )
    }) {
        // Not `std::path::absolute`: it would drop the `..` on Windows.
        let path = std::env::current_dir().map_or_else(|_| out.to_path_buf(), |d| d.join(out));
        return Err(OutputError::InvalidOutputPath { path });
    }
    let canonical_base = fs::canonicalize(&base).map_err(io_err(&base))?;
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

/// Splits `path` into its longest existing leading part (`.` if none) and the
/// rest.
///
/// Components are checked one at a time from the start, before anything
/// normalizes the path: Windows resolves `..` lexically (so `missing\..`
/// "exists"), and so does `std::path::absolute` there.
fn split_existing(path: &Path) -> (PathBuf, PathBuf) {
    let mut base = PathBuf::new();
    let mut rest = path.components();
    while let Some(component) = rest.clone().next() {
        let next = base.join(component);
        if !next.exists() {
            break;
        }
        base = next;
        rest.next();
    }
    if base.as_os_str().is_empty() {
        base = PathBuf::from(".");
    }
    (base, rest.as_path().to_path_buf())
}

/// The deepest existing ancestor of `path` (or `path` itself).
pub(crate) fn existing_ancestor(path: &Path) -> PathBuf {
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
