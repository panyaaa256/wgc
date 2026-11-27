//! Reports and their JSON form, for worlds on disk. Paths are relative to the
//! input path with `/` separators; a path that is not UTF-8 makes the report
//! fail.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

pub use wgc_core::report::*;

use crate::Outcome;
use crate::layout::WorldLayout;
use crate::output::GcOutcome;
use crate::plan::PlanError;
use crate::types::{DimensionId, GuardId};
use crate::world::FsWorld;

/// Why a report could not be produced.
#[derive(Debug, thiserror::Error)]
pub enum ReportError {
    #[error("path is not valid utf-8: {0}")]
    NonUtf8Path(PathBuf),
    #[error(transparent)]
    Plan(#[from] PlanError),
}

impl ReportError {
    /// How a report that ended with this error ended.
    pub const fn outcome(&self) -> Outcome {
        match self {
            ReportError::Plan(PlanError::Cancelled) => Outcome::Interrupted,
            _ => Outcome::Failed,
        }
    }
}

impl From<NonUtf8Path<PathBuf>> for ReportError {
    fn from(NonUtf8Path(path): NonUtf8Path<PathBuf>) -> Self {
        ReportError::NonUtf8Path(path)
    }
}

impl From<wgc::scan::ReportError<PlanError, PathBuf>> for ReportError {
    fn from(e: wgc::scan::ReportError<PlanError, PathBuf>) -> Self {
        match e {
            wgc::scan::ReportError::Plan(e) => ReportError::Plan(e.into()),
            wgc::scan::ReportError::NonUtf8Path(path) => ReportError::NonUtf8Path(path),
        }
    }
}

/// The `wgc gc` report of a finished `gc --out`.
pub fn gc_output_report(outcome: &GcOutcome) -> Result<GcReport, ReportError> {
    let world = FsWorld::new(&outcome.plan.layout);
    let scan =
        wgc::scan::scan_report(&world, &outcome.plan, &outcome.trust, outcome.lock_status())?;
    gc_report(
        scan,
        Some(&outcome.out),
        &outcome.forced,
        outcome.plan.gc_dims.as_ref(),
    )
}

/// The world folders relative to the input path, for the steps that swap the
/// output in. A folder that cannot be named so is given as it is.
pub fn world_folder_names(layout: &WorldLayout) -> Vec<String> {
    layout
        .world_folders
        .iter()
        .map(|f| rel_path(&layout.root, f).unwrap_or_else(|_| f.display().to_string()))
        .collect()
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
    gc_dims: Option<&BTreeSet<DimensionId>>,
) -> Result<GcReport, ReportError> {
    let out = out
        .map(|p| {
            let full = std::fs::canonicalize(p)
                .or_else(|_| std::path::absolute(p))
                .unwrap_or_else(|_| p.to_path_buf());
            utf8(&full)
        })
        .transpose()?;
    Ok(wgc_core::report::gc_report(scan, out, forced, gc_dims))
}

/// `path` as a string, if it is valid UTF-8.
pub(crate) fn utf8(path: &Path) -> Result<String, ReportError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| ReportError::NonUtf8Path(path.to_path_buf()))
}

/// `path` relative to `root`, with `/` separators. `root` itself is `"."`.
/// Paths outside `root` (e.g. the server defaults of a world folder input)
/// are given as they are.
pub fn rel_path(root: &Path, path: &Path) -> Result<String, ReportError> {
    let non_utf8 = || ReportError::NonUtf8Path(path.to_path_buf());
    let Ok(relative) = path.strip_prefix(root) else {
        return utf8(path);
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
