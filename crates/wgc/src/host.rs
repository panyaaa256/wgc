//! What the commands need from their surroundings: running work, reading a
//! recognized world, and writing the output.

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::AtomicBool;

use wgc_core::layout::{Dimension, DimensionFiles, WorldLayout};
use wgc_core::plan::Plan;
use wgc_core::report::LockStatus;
use wgc_core::source::WorldSource;
use wgc_core::types::GuardId;

use crate::gc::Stage;
use crate::observe::{Observer, Warning};

/// Runs independent pieces of work, possibly in parallel.
pub trait Runner: Sync {
    /// Applies `f` to every item and returns the results in item order, or
    /// the first error met. After each finished item, `done` is called with
    /// the number of items finished so far: one call at a time, in increasing
    /// order.
    fn try_map<T, R, E>(
        &self,
        items: &[T],
        f: impl Fn(&T) -> Result<R, E> + Sync,
        done: impl Fn(u64) + Sync,
    ) -> Result<Vec<R>, E>
    where
        T: Sync,
        R: Send,
        E: Send;
}

/// A [`Runner`] that works through the items one after another.
#[derive(Debug, Clone, Copy, Default)]
pub struct Sequential;

impl Runner for Sequential {
    fn try_map<T, R, E>(
        &self,
        items: &[T],
        f: impl Fn(&T) -> Result<R, E> + Sync,
        done: impl Fn(u64) + Sync,
    ) -> Result<Vec<R>, E>
    where
        T: Sync,
        R: Send,
        E: Send,
    {
        items
            .iter()
            .zip(1..)
            .map(|(item, n)| {
                let result = f(item)?;
                done(n);
                Ok(result)
            })
            .collect()
    }
}

/// The path type of a world's source.
pub type PathOf<W> = <<W as World>::Source as WorldSource>::Path;

/// A recognized world: its layout and the files in it.
pub trait World: Sync {
    type Source: WorldSource;
    /// Why the world's files could not be listed or read.
    type Error: From<<Self::Source as WorldSource>::Error> + Send;
    /// Every file under the world folders, as listed at one moment.
    type Files: Sync;
    /// Locks held on the world. Dropping them releases the world.
    type Locks;
    /// Why the world could not be locked.
    type LockError;

    fn source(&self) -> &Self::Source;

    fn layout(&self) -> &WorldLayout<PathOf<Self>>;

    /// The region triples and external chunk files of `dim`.
    fn list_dimension(
        &self,
        dim: &Dimension<PathOf<Self>>,
    ) -> Result<DimensionFiles<PathOf<Self>>, Self::Error>;

    /// Lists every file under the world folders.
    fn files(&self) -> Result<Self::Files, Self::Error>;

    /// Each listed file with its size.
    fn file_sizes<'a>(files: &'a Self::Files) -> impl Iterator<Item = (&'a PathOf<Self>, u64)>
    where
        PathOf<Self>: 'a;

    /// The file an error is about, if it names one.
    fn error_path(&self, error: &Self::Error) -> Option<PathOf<Self>>;

    /// The name of a path in reports: relative to the input path with `/`
    /// separators, or `None` if it has no such name.
    fn report_name(&self, path: &PathOf<Self>) -> Option<String>;

    /// Locks every world folder the way the game does.
    fn lock(&self) -> Result<Self::Locks, Self::LockError>;

    /// How a failure to lock appears in reports.
    fn lock_status(error: &Self::LockError) -> LockStatus;
}

/// The world folders after an attempt to lock each of them.
pub struct Locked<L> {
    pub locks: Vec<L>,
    /// Some world folder has no `session.lock` and was not locked.
    pub missing: bool,
}

/// What `gc --out` needs besides reading the world: an output location, the
/// state of the source, and writing, verifying and recording the output.
pub trait GcHost: World {
    /// Where the output is requested.
    type Target;
    /// The checked output location.
    type Out;
    /// A lock on one world folder.
    type FolderLock;
    /// The state of the source's files, to detect later changes.
    type State;
    type Written;
    /// Why a step failed. It may itself mean that the run was cancelled.
    type Failure: From<<Self as World>::Error>;

    /// Checks that `target` is new or empty and does not overlap the world.
    fn check_output(&self, target: &Self::Target) -> Result<Self::Out, Self::Failure>;

    /// Locks every world folder. A folder without `session.lock` is
    /// reported in [`Locked::missing`]; any other failure is an error.
    fn lock_folders(&self) -> Result<Locked<Self::FolderLock>, Self::Failure>;

    /// Records the state of the listed files.
    fn capture(&self, files: &Self::Files) -> Result<Self::State, Self::Failure>;

    /// Free space where the output goes, in bytes.
    fn available_space(&self, out: &Self::Out) -> Result<u64, Self::Failure>;

    /// Creates the output location.
    fn create_output(&self, out: &Self::Out) -> Result<(), Self::Failure>;

    /// Writes the plan into the output.
    fn write(
        &self,
        plan: &Plan<PathOf<Self>>,
        files: &Self::Files,
        out: &Self::Out,
        observer: &dyn Observer<Warning<PathOf<Self>, Self::LockError>>,
        cancel: &AtomicBool,
    ) -> Result<Self::Written, Self::Failure>;

    /// Checks the output against the plan and the source.
    #[allow(clippy::too_many_arguments)]
    fn verify(
        &self,
        plan: &Plan<PathOf<Self>>,
        files: &Self::Files,
        written: &Self::Written,
        out: &Self::Out,
        state: &Self::State,
        observer: &dyn Observer<Warning<PathOf<Self>, Self::LockError>>,
        cancel: &AtomicBool,
    ) -> Result<(), Self::Failure>;

    /// Writes the records that mark the output as complete.
    fn record(
        &self,
        out: &Self::Out,
        state: &Self::State,
        plan: &Plan<PathOf<Self>>,
        forced: &BTreeSet<GuardId>,
    ) -> Result<(), Self::Failure>;

    /// Records that `stage` failed. Best effort.
    fn record_failure(&self, out: &Self::Out, stage: Stage, error: &Self::Failure);

    /// Whether an error means the run was cancelled.
    fn is_cancelled(error: &Self::Failure) -> bool;

    /// Whether some output file could not get its original owner.
    fn owner_not_preserved(written: &Self::Written) -> bool;
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[test]
    fn sequential_keeps_order_counts_and_stops_at_the_first_error() {
        let seen = Mutex::new(Vec::new());
        let doubled = Sequential.try_map(
            &[1, 2, 3],
            |&x| Ok::<_, ()>(x * 2),
            |n| {
                seen.lock().unwrap().push(n);
            },
        );
        assert_eq!(doubled, Ok(vec![2, 4, 6]));
        assert_eq!(*seen.lock().unwrap(), [1, 2, 3]);

        let result =
            Sequential.try_map(&[1, 2, 3], |&x| if x == 2 { Err(x) } else { Ok(x) }, |_| {});
        assert_eq!(result, Err(2));
    }
}
