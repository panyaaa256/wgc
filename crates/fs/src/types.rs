//! The shared value types, plus results that may be unknown and the channel
//! through which long operations report progress and warnings.

use std::collections::BTreeSet;
use std::path::PathBuf;

pub use wgc_core::types::*;

/// A value that was read, or why it could not be confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Known<T> {
    Value(T),
    Unknown(UnknownReason),
}

impl<T> From<Result<T, UnknownReason>> for Known<T> {
    fn from(result: Result<T, UnknownReason>) -> Self {
        match result {
            Ok(v) => Known::Value(v),
            Err(reason) => Known::Unknown(reason),
        }
    }
}

/// Why something, such as a trust check, could not be confirmed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum UnknownReason {
    UnreadableFile { path: PathBuf },
    UnexpectedValue { path: PathBuf, key: String },
    UnknownVersion(DataVersion),
    NoPlayersChecked,
    ForcedChunksUnknown(BTreeSet<DimensionId>),
}

/// Receives progress and warnings from long-running operations.
/// This crate never prints; the CLI implements this.
pub trait Observer: Send + Sync {
    fn progress(&self, p: Progress);
    fn warn(&self, w: Warning);
}

/// Progress of a long-running operation, counted in region triples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    Reading { done: u64, total: u64 },
    Writing { done: u64, total: u64 },
    Verifying { done: u64, total: u64 },
}

/// Something the user should know that does not stop the operation.
#[derive(Debug)]
pub enum Warning {
    /// `scan` or an estimate could not lock the world.
    LockUnavailable(crate::lock::LockError),
    /// L-1 was ignored with `--force`: the world is not locked.
    LockSkipped,
    /// The world is older than `DV_SESSION_LOCK_OS`: a running game cannot be detected.
    PreLockVersion(DataVersion),
    /// Output files could not get the original owner (reported once).
    OwnerNotPreserved,
    SymlinkFollowed(PathBuf),
    SpecialFileSkipped(PathBuf),
    /// A failed check was ignored with `--force`.
    Forced(GuardId),
}

/// An [`Observer`] that ignores everything.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopObserver;

impl Observer for NoopObserver {
    fn progress(&self, _: Progress) {}
    fn warn(&self, _: Warning) {}
}
