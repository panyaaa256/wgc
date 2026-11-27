//! Reading and writing Minecraft worlds on the host file system for `wgc`.
//!
//! This crate never writes to the terminal. Long-running operations report
//! progress and warnings through a [`types::Observer`] instead.

pub use wgc_core::{TOOL_VERSION, nbt, region, versions};

pub mod layout;
pub mod lock;
pub mod output;
pub mod plan;
pub mod report;
pub mod source;
mod sys;
pub mod trust;
pub mod types;
