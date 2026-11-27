//! Pure Minecraft world data handling for `wgc`: value types, region files,
//! NBT, and the decisions made from them (the plan, the trust checks and the
//! reports). This crate does no file system access and needs only `alloc`;
//! files are read through a [`source::WorldSource`].

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod inflate;
pub mod layout;
pub mod nbt;
pub mod plan;
pub mod region;
pub mod report;
pub mod source;
pub mod trust;
pub mod types;
pub mod versions;

/// Version of the tool, recorded in reports and `.wgc/*.json`.
pub const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");
