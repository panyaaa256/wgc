//! Running the trust checks T-1..T-5 against the files on disk.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use wgc_core::trust::ChunkKey;

pub use wgc_core::trust::InhabitedMap;

use crate::layout::WorldLayout;
use crate::plan::{Plan, PlanError, read_inhabited};
use crate::source::FsSource;
use crate::types::{ChunkPos, DimensionId, UnknownReason};

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
    let inhabited_of =
        (!skip_t3).then_some(|wanted: &BTreeSet<ChunkKey>| read_inhabited_of(layout, wanted));
    wgc_core::trust::check_trust_before_plan(&FsSource, layout, gc_dims, inhabited_of)
}

/// Reads the chunks from the region files, one dimension at a time.
fn read_inhabited_of(
    layout: &WorldLayout,
    wanted: &BTreeSet<ChunkKey>,
) -> Result<InhabitedMap, UnknownReason> {
    let mut found = BTreeMap::new();
    for dim in &layout.dimensions {
        let chunks: BTreeSet<ChunkPos> = wanted
            .iter()
            .filter(|(id, _)| *id == dim.id)
            .map(|(_, pos)| *pos)
            .collect();
        let inhabited =
            read_inhabited(dim, &chunks).map_err(|e| UnknownReason::UnreadableFile {
                path: match e {
                    PlanError::Io { path, .. } => path,
                    _ => dim.dir.clone(),
                },
            })?;
        found.extend(
            inhabited
                .into_iter()
                .map(|(pos, inhabited)| ((dim.id.clone(), pos), inhabited)),
        );
    }
    Ok(found)
}

#[cfg(test)]
mod tests;
