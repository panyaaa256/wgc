//! Pure Minecraft world data handling for `wgc`: value types, region files and
//! NBT. This crate does no file system access and needs only `alloc`.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod inflate;
pub mod nbt;
pub mod region;
pub mod types;
pub mod versions;

/// Version of the tool, recorded in reports and `.wgc/*.json`.
pub const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");
