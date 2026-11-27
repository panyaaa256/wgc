//! Running the trust checks T-1..T-5 against the files on disk.

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::layout::WorldLayout;
use crate::plan::Plan;
use crate::source::FsSource;
use crate::types::DimensionId;
use crate::world::FsWorld;

/// Results of the trust checks, in ID order T-1..T-5. Each check is described
/// on its `GuardId` variant.
pub type TrustReport = wgc_core::trust::TrustReport<PathBuf>;
pub type CheckResult = wgc_core::trust::CheckResult<PathBuf>;
pub type CheckStatus = wgc_core::trust::CheckStatus<PathBuf>;
pub type TrustDetail = wgc_core::trust::TrustDetail<PathBuf>;

/// Runs T-1..T-5 against a plan.
pub fn check_trust(plan: &Plan) -> TrustReport {
    wgc_core::trust::check_trust(&FsSource, plan)
}

/// Runs T-1..T-5 without a plan, so that `gc --out` can stop before reading
/// every region file. T-3 reads only the region files that hold the chunks
/// players stand in, and T-5 reads the forced chunks directly.
///
/// With `skip_t3`, T-3 reads nothing and is left out of the report: when
/// `--force` allows it, it cannot stop the run, and the check against the plan
/// decides whether it was forced.
pub fn check_trust_before_plan(
    layout: &WorldLayout,
    gc_dims: Option<&BTreeSet<DimensionId>>,
    skip_t3: bool,
) -> TrustReport {
    wgc::plan::trust_before_plan(&FsWorld::new(layout), gc_dims, skip_t3)
}

#[cfg(test)]
mod tests;
