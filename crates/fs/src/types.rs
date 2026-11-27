//! The shared value types for host paths, and the channel through which long
//! operations report progress and warnings.

use std::path::PathBuf;

pub use wgc_core::types::*;

/// A value that was read, or why it could not be confirmed.
pub type Known<T> = wgc_core::types::Known<T, PathBuf>;

/// Why something, such as a trust check, could not be confirmed.
pub type UnknownReason = wgc_core::types::UnknownReason<PathBuf>;

pub use wgc::observe::{NoopObserver, Observer, Progress};

/// Something the user should know that does not stop the operation.
pub type Warning = wgc::observe::Warning<PathBuf, crate::lock::LockError>;
