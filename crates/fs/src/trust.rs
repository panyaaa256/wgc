//! The trust checks T-1..T-5 for host paths.

use std::path::PathBuf;

/// Results of the trust checks, in ID order T-1..T-5. Each check is described
/// on its `GuardId` variant.
pub type TrustReport = wgc_core::trust::TrustReport<PathBuf>;
pub type CheckResult = wgc_core::trust::CheckResult<PathBuf>;
pub type CheckStatus = wgc_core::trust::CheckStatus<PathBuf>;
pub type TrustDetail = wgc_core::trust::TrustDetail<PathBuf>;

#[cfg(test)]
mod tests;
