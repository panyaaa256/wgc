//! The steps of each `wgc` command: `scan`, the `gc` estimate and `gc --out`.
//!
//! The steps are written against the traits in [`host`], so they do not depend
//! on where the world is stored or how work runs in parallel. Like
//! `wgc-core`, this crate needs only `alloc`.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod gc;
pub mod host;
pub mod observe;
pub mod outcome;
pub mod plan;
pub mod scan;
