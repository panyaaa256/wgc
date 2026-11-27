//! A world on the host file system, and parallel work with `rayon`, as the
//! `wgc` commands need them.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, PoisonError};

use rayon::prelude::*;
use wgc::gc::Stage;
use wgc::host::{GcHost, Locked, Runner, World};
use wgc_core::report::LockStatus;

use crate::layout::{self, Dimension, DimensionFiles, WorldFiles, WorldLayout};
use crate::lock::{self, LockError, SessionLock, SessionLocks};
use crate::output::copy::{self, Written};
use crate::output::record::{self, SourceState};
use crate::output::{self, OutputError, io_err, verify};
use crate::plan::{Plan, PlanError};
use crate::report::rel_path;
use crate::source::FsSource;
use crate::types::{GuardId, Observer, Warning};

/// Runs work in parallel on the `rayon` thread pool.
#[derive(Debug, Clone, Copy, Default)]
pub struct Rayon;

impl Runner for Rayon {
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
        // Counting and reporting under one lock keeps `done` increasing.
        let finished = Mutex::new(0u64);
        items
            .par_iter()
            .map(|item| {
                let result = f(item)?;
                let mut n = finished.lock().unwrap_or_else(PoisonError::into_inner);
                *n += 1;
                done(*n);
                Ok(result)
            })
            .collect()
    }
}

/// A recognized world on the host file system.
#[derive(Debug, Clone, Copy)]
pub struct FsWorld<'a> {
    layout: &'a WorldLayout,
}

impl<'a> FsWorld<'a> {
    pub const fn new(layout: &'a WorldLayout) -> Self {
        Self { layout }
    }
}

pub(crate) fn lock_status(error: &LockError) -> LockStatus {
    match error {
        LockError::InUse { .. } | LockError::HeldByThisProcess { .. } => LockStatus::InUse,
        LockError::Missing { .. } => LockStatus::Missing,
        LockError::NotWritable { .. } => LockStatus::NotWritable,
        LockError::Io { .. } => LockStatus::Error,
    }
}

impl World for FsWorld<'_> {
    type Source = FsSource;
    type Error = PlanError;
    type Files = WorldFiles;
    type Locks = SessionLocks;
    type LockError = LockError;

    fn source(&self) -> &FsSource {
        &FsSource
    }

    fn layout(&self) -> &WorldLayout {
        self.layout
    }

    fn list_dimension(&self, dim: &Dimension) -> Result<DimensionFiles, PlanError> {
        Ok(layout::list_dimension_files(dim)?)
    }

    fn files(&self) -> Result<WorldFiles, PlanError> {
        Ok(layout::world_files(self.layout)?)
    }

    fn copied_files<'a>(&'a self, files: &'a WorldFiles) -> impl Iterator<Item = (&'a PathBuf, u64)>
    where
        PathBuf: 'a,
    {
        files
            .files
            .iter()
            .filter(|f| !output::is_excluded(self.layout, &f.path))
            .map(|f| (&f.path, f.size))
    }

    fn error_path(&self, error: &PlanError) -> Option<PathBuf> {
        match error {
            PlanError::Io { path, .. } => Some(path.clone()),
            _ => None,
        }
    }

    fn report_name(&self, path: &PathBuf) -> Option<String> {
        rel_path(&self.layout.root, path).ok()
    }

    fn lock(&self) -> Result<SessionLocks, LockError> {
        SessionLocks::try_acquire_all(&self.layout.world_folders)
    }

    fn lock_status(error: &LockError) -> LockStatus {
        lock_status(error)
    }
}

impl GcHost for FsWorld<'_> {
    type Target = PathBuf;
    type Out = PathBuf;
    type FolderLock = SessionLock;
    type State = SourceState;
    type Written = Written;
    type Failure = OutputError;

    fn check_output(&self, target: &PathBuf) -> Result<PathBuf, OutputError> {
        output::check_output(self.layout, target)
    }

    fn lock_folders(&self) -> Result<Locked<SessionLock>, OutputError> {
        let folders = lock::distinct_folders(&self.layout.world_folders);
        let mut locked = Locked {
            locks: Vec::new(),
            missing: false,
        };
        for folder in &folders {
            match SessionLock::try_acquire(folder) {
                Ok(lock) => locked.locks.push(lock),
                Err(LockError::Missing { .. }) => locked.missing = true,
                Err(e) => return Err(OutputError::Lock(e)),
            }
        }
        Ok(locked)
    }

    fn capture(&self, files: &WorldFiles) -> Result<SourceState, OutputError> {
        SourceState::capture(self.layout, files)
    }

    fn available_space(&self, out: &PathBuf) -> Result<u64, OutputError> {
        let probe = output::existing_ancestor(out);
        crate::sys::available_space(&probe).map_err(io_err(&probe))
    }

    fn create_output(&self, out: &PathBuf) -> Result<(), OutputError> {
        fs::create_dir_all(out).map_err(io_err(out))
    }

    fn write(
        &self,
        plan: &Plan,
        walked: &WorldFiles,
        out: &PathBuf,
        observer: &dyn Observer<Warning>,
        cancel: &AtomicBool,
    ) -> Result<Written, OutputError> {
        copy::write_output(plan, walked, out, observer, cancel)
    }

    fn verify(
        &self,
        plan: &Plan,
        walked: &WorldFiles,
        written: &Written,
        out: &PathBuf,
        state: &SourceState,
        observer: &dyn Observer<Warning>,
        cancel: &AtomicBool,
    ) -> Result<(), OutputError> {
        verify::check_output(plan, walked, written, out, state, observer, cancel)
    }

    fn record(
        &self,
        out: &PathBuf,
        state: &SourceState,
        plan: &Plan,
        forced: &BTreeSet<GuardId>,
    ) -> Result<(), OutputError> {
        record::write_records(out, state, plan, forced)
    }

    fn record_failure(&self, out: &PathBuf, stage: Stage, error: &OutputError) {
        // Best effort: the missing complete.json already marks the output as incomplete.
        let _ = record::write_failure(out, stage, error);
    }

    fn is_cancelled(error: &OutputError) -> bool {
        matches!(error, OutputError::Cancelled)
    }

    fn owner_not_preserved(written: &Written) -> bool {
        written.owner_not_preserved
    }
}
