//! The channel through which long operations report progress and warnings.

use wgc_core::types::{DataVersion, GuardId};

/// Receives progress and warnings from long-running operations. Nothing below
/// the command line prints; the caller implements this. `W` is the warning
/// type, usually a [`Warning`].
pub trait Observer<W>: Send + Sync {
    fn progress(&self, p: Progress);
    fn warn(&self, w: W);
}

/// Progress of a long-running operation, counted in region triples or files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    Reading { done: u64, total: u64 },
    Writing { done: u64, total: u64 },
    Verifying { done: u64, total: u64 },
}

/// Something the user should know that does not stop the operation. `P` names
/// files, `L` is why the world could not be locked.
#[derive(Debug)]
pub enum Warning<P, L> {
    /// `scan` or an estimate could not lock the world.
    LockUnavailable(L),
    /// L-1 was ignored with `--force`: the world is not locked.
    LockSkipped,
    /// The world is older than the version from which the game locks
    /// `session.lock` with the operating system: a running game cannot be
    /// detected.
    PreLockVersion(DataVersion),
    /// Output files could not get the original owner (reported once).
    OwnerNotPreserved,
    SymlinkFollowed(P),
    SpecialFileSkipped(P),
    /// A failed check was ignored with `--force`.
    Forced(GuardId),
}

/// An [`Observer`] that ignores everything.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopObserver;

impl<W> Observer<W> for NoopObserver {
    fn progress(&self, _: Progress) {}
    fn warn(&self, _: W) {}
}
