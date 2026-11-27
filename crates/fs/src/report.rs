//! Reports and their JSON form, for worlds on disk. Paths are relative to the
//! input path with `/` separators; a path that is not UTF-8 makes the report
//! fail.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::AtomicBool;

pub use wgc_core::report::*;

use crate::layout::WorldLayout;
use crate::lock::SessionLocks;
use crate::plan::{Plan, PlanError};
use crate::trust::TrustReport;
use crate::types::{GuardId, Observer, Warning};
use crate::world::{FsWorld, Rayon};

/// Why a report could not be produced.
#[derive(Debug, thiserror::Error)]
pub enum ReportError {
    #[error("path is not valid utf-8: {0}")]
    NonUtf8Path(PathBuf),
    #[error(transparent)]
    Plan(#[from] PlanError),
}

impl From<NonUtf8Path<PathBuf>> for ReportError {
    fn from(NonUtf8Path(path): NonUtf8Path<PathBuf>) -> Self {
        ReportError::NonUtf8Path(path)
    }
}

/// Locks the world for a read-only report if possible; otherwise warns
/// ([`Warning::LockUnavailable`]) and continues. Keep the returned locks alive while reading.
pub fn try_lock_for_report(
    layout: &WorldLayout,
    observer: &dyn Observer<Warning>,
) -> (LockStatus, Option<SessionLocks>) {
    wgc::scan::try_lock_for_report(&FsWorld::new(layout), observer)
}

/// Runs `wgc scan`: locks if possible, plans every dimension and checks trust.
pub fn run_scan(
    layout: &WorldLayout,
    observer: &dyn Observer<Warning>,
    cancel: &AtomicBool,
) -> Result<ScanReport, ReportError> {
    wgc::scan::run_scan(&FsWorld::new(layout), &Rayon, observer, cancel).map_err(ReportError::from)
}

impl From<wgc::scan::ReportError<PlanError, PathBuf>> for ReportError {
    fn from(e: wgc::scan::ReportError<PlanError, PathBuf>) -> Self {
        match e {
            wgc::scan::ReportError::Plan(e) => ReportError::Plan(e.into()),
            wgc::scan::ReportError::NonUtf8Path(path) => ReportError::NonUtf8Path(path),
        }
    }
}

/// Builds the scan report for a plan made without `--dimension`.
pub fn scan_report(
    plan: &Plan,
    trust: &TrustReport,
    lock: LockStatus,
) -> Result<ScanReport, ReportError> {
    Ok(wgc::scan::scan_report(
        &FsWorld::new(&plan.layout),
        plan,
        trust,
        lock,
    )?)
}

/// Turns a scan report into the `wgc gc` report.
///
/// Pass `out` only after the output was written and verified: the report
/// then says `output` and carries the `no_trash` notice. `out` is
/// canonicalized (made absolute if it cannot be resolved).
pub fn gc_report(
    scan: ScanReport,
    out: Option<&Path>,
    forced: &BTreeSet<GuardId>,
    plan: &Plan,
) -> Result<GcReport, ReportError> {
    let out = out
        .map(|p| {
            let full = std::fs::canonicalize(p)
                .or_else(|_| std::path::absolute(p))
                .unwrap_or_else(|_| p.to_path_buf());
            full.to_str()
                .map(str::to_owned)
                .ok_or(ReportError::NonUtf8Path(full.clone()))
        })
        .transpose()?;
    Ok(wgc_core::report::gc_report(scan, out, forced, plan))
}

/// `path` relative to `root`, with `/` separators. `root` itself is `"."`.
/// Paths outside `root` (e.g. the server defaults of a world folder input)
/// are given as they are.
pub fn rel_path(root: &Path, path: &Path) -> Result<String, ReportError> {
    let non_utf8 = || ReportError::NonUtf8Path(path.to_path_buf());
    let Ok(relative) = path.strip_prefix(root) else {
        return path.to_str().map(str::to_owned).ok_or_else(non_utf8);
    };
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(s) => parts.push(s.to_str().ok_or_else(non_utf8)?),
            Component::CurDir => {}
            // A path relative to root has no prefix, root or parent components.
            other => parts.push(other.as_os_str().to_str().ok_or_else(non_utf8)?),
        }
    }
    if parts.is_empty() {
        return Ok(".".to_owned());
    }
    Ok(parts.join("/"))
}

#[cfg(test)]
mod tests;
