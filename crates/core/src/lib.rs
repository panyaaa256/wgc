//! Analysis and processing for `wgc`.
//!
//! This crate never writes to the terminal. Long-running operations report
//! progress and warnings through an observer instead (see
//! `docs/spec/types.md`). Modules are added phase by phase as described in
//! `docs/spec.md`.

/// Version of the tool, recorded in reports and `.wgc/*.json`.
pub const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");
