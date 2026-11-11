//! Analysis and processing for `wgc`.
//!
//! This crate never writes to the terminal. Long-running operations report
//! progress and warnings through a [`types::Observer`] instead.

pub mod layout;
pub mod lock;
pub mod nbt;
pub mod output;
pub mod plan;
pub mod region;
pub mod report;
mod sys;
pub mod trust;
pub mod types;
pub mod versions;

/// Version of the tool, recorded in reports and `.wgc/*.json`.
pub const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");
