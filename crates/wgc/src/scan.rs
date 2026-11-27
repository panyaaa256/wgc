//! `wgc scan` and the `gc` estimate: reports that write nothing.

use alloc::collections::BTreeSet;
use core::sync::atomic::AtomicBool;

use wgc_core::plan::{Plan, PlanOptions};
use wgc_core::report::{GcReport, LockStatus, NonUtf8Path, ScanReport, gc_report};
use wgc_core::trust::{TrustReport, check_trust};
use wgc_core::types::DimensionId;

use crate::host::{PathOf, Runner, World};
use crate::observe::Warning;
use crate::plan::{ObserverOf, PlanError, build_plan};

/// Why a report could not be produced.
#[derive(Debug)]
pub enum ReportError<E, P> {
    Plan(PlanError<E>),
    /// A path the report has to name is not valid UTF-8.
    NonUtf8Path(P),
}

impl<E, P> From<PlanError<E>> for ReportError<E, P> {
    fn from(e: PlanError<E>) -> Self {
        ReportError::Plan(e)
    }
}

impl<E, P> From<NonUtf8Path<P>> for ReportError<E, P> {
    fn from(NonUtf8Path(path): NonUtf8Path<P>) -> Self {
        ReportError::NonUtf8Path(path)
    }
}

/// Locks the world for a read-only report if possible; otherwise warns
/// ([`Warning::LockUnavailable`]) and continues. Keep the returned locks alive
/// while reading.
pub fn try_lock_for_report<W: World>(
    world: &W,
    observer: ObserverOf<'_, W>,
) -> (LockStatus, Option<W::Locks>) {
    match world.lock() {
        Ok(held) => (LockStatus::Acquired, Some(held)),
        Err(e) => {
            let status = W::lock_status(&e);
            observer.warn(Warning::LockUnavailable(e));
            (status, None)
        }
    }
}

/// The scan report of a plan, with paths named by the world.
pub fn scan_report<W: World>(
    world: &W,
    plan: &Plan<PathOf<W>>,
    trust: &TrustReport<PathOf<W>>,
    lock: LockStatus,
) -> Result<ScanReport, NonUtf8Path<PathOf<W>>> {
    wgc_core::report::scan_report(plan, trust, lock, |p| world.report_name(p))
}

/// Runs `wgc scan`: locks if possible, plans every dimension and checks trust.
pub fn run_scan<W: World>(
    world: &W,
    runner: &impl Runner,
    observer: ObserverOf<'_, W>,
    cancel: &AtomicBool,
) -> Result<ScanReport, ReportError<W::Error, PathOf<W>>> {
    let (lock, _held) = try_lock_for_report(world, observer);
    let plan = build_plan(world, runner, PlanOptions::default(), observer, cancel)?;
    let trust = check_trust(world.source(), &plan);
    Ok(scan_report(world, &plan, &trust, lock)?)
}

/// Runs `wgc gc` without an output: what GC would delete in `gc_dims` (all
/// dimensions if `None`), with nothing written.
pub fn run_estimate<W: World>(
    world: &W,
    runner: &impl Runner,
    gc_dims: Option<BTreeSet<DimensionId>>,
    observer: ObserverOf<'_, W>,
    cancel: &AtomicBool,
) -> Result<GcReport, ReportError<W::Error, PathOf<W>>> {
    let (lock, _held) = try_lock_for_report(world, observer);
    let plan = build_plan(world, runner, PlanOptions { gc_dims }, observer, cancel)?;
    let trust = check_trust(world.source(), &plan);
    let scan = scan_report(world, &plan, &trust, lock)?;
    Ok(gc_report(
        scan,
        None,
        &BTreeSet::new(),
        plan.gc_dims.as_ref(),
    ))
}
