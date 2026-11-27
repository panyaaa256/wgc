//! The plan: what to delete and how to write each file, built from the files
//! on disk.

mod build;

use std::io;
use std::path::PathBuf;

pub use wgc_core::plan::BoundaryCluster;
pub use wgc_core::plan::{
    ChunkDecision, Decision, EntryFate, EntryInfo, Estimate, ExternalAction, FileAction, Inhabited,
    KeepReason, KindSizes, OmitReason, PlanOptions, RemoveReason, Sizes, is_targeted,
};

use crate::layout::LayoutError;

pub use build::build_plan;

/// One region-format file of a triple.
pub type FileInfo = wgc_core::plan::FileInfo<PathBuf>;
pub type ExternalFile = wgc_core::plan::ExternalFile<PathBuf>;
pub type TriplePlan = wgc_core::plan::TriplePlan<PathBuf>;
pub type OmittedFile = wgc_core::plan::OmittedFile<PathBuf>;
pub type DimensionPlan = wgc_core::plan::DimensionPlan<PathBuf>;
/// The plan for a whole world.
pub type Plan = wgc_core::plan::Plan<PathBuf>;

/// Why a plan could not be built.
#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error("cannot read {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Layout(#[from] LayoutError),
    #[error("cancelled")]
    Cancelled,
}

impl From<wgc::plan::PlanError<PlanError>> for PlanError {
    fn from(e: wgc::plan::PlanError<PlanError>) -> Self {
        match e {
            wgc::plan::PlanError::World(e) => e,
            wgc::plan::PlanError::Cancelled => PlanError::Cancelled,
        }
    }
}
