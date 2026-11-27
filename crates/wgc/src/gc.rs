//! `gc --out`: writing the result to a new location and verifying it.
//!
//! The source world is only read. Every stop that `--force` does not allow is
//! found before the output is created and before the plan reads every region
//! file.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use wgc_core::plan::{Plan, PlanOptions};
use wgc_core::report::LockStatus;
use wgc_core::trust::{TrustReport, check_trust};
use wgc_core::types::{DimensionId, GuardId};
use wgc_core::versions::DV_SESSION_LOCK_OS;

use crate::host::{GcHost, PathOf, Runner};
use crate::observe::Warning;
use crate::plan::{ObserverOf, PlanError, build_plan, trust_before_plan};

/// Options for [`run_gc`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GcOptions<T> {
    /// Where the output goes: a new or empty location.
    pub out: T,
    pub gc_dims: Option<BTreeSet<DimensionId>>,
    /// Stops that `--force` allows to continue past.
    pub allowed: BTreeSet<GuardId>,
}

/// What a successful `gc --out` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GcOutcome<P, O> {
    pub plan: Plan<P>,
    pub trust: TrustReport<P>,
    /// Stops that were ignored with `--force`.
    pub forced: BTreeSet<GuardId>,
    /// The checked output location.
    pub out: O,
    /// Whether some output files could not get their original owner.
    pub owner_not_preserved: bool,
    /// Whether some world folder was not locked because `session.lock` was
    /// missing and L-1 was forced.
    pub lock_skipped: bool,
}

impl<P, O> GcOutcome<P, O> {
    /// The state of `session.lock` for the report: locked, unless L-1 was
    /// forced and some world folder could not be locked.
    pub const fn lock_status(&self) -> LockStatus {
        if self.lock_skipped {
            LockStatus::Missing
        } else {
            LockStatus::Acquired
        }
    }
}

/// Why `gc --out` stopped or failed.
#[derive(Debug)]
pub enum GcError<F> {
    /// Stopped by trust checks or L-1 that `--force` did not allow.
    Stopped(Vec<GuardId>),
    InsufficientSpace {
        needed: u64,
        available: u64,
    },
    Cancelled,
    Host(F),
}

impl<F> From<F> for GcError<F> {
    fn from(e: F) -> Self {
        GcError::Host(e)
    }
}

/// The step of writing the output that failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Write,
    Verify,
    Record,
}

impl Stage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Stage::Write => "write",
            Stage::Verify => "verify",
            Stage::Record => "record",
        }
    }
}

/// Extra space required on top of the estimate.
const SPACE_MARGIN: u64 = 64 * 1024 * 1024;

/// Space the output needs: the estimate plus 1% and a fixed margin.
const fn space_needed(estimate: u64) -> u64 {
    estimate.saturating_add(estimate / 100) + SPACE_MARGIN
}

/// What [`run_gc`] returns for a host.
pub type GcResult<H> =
    Result<GcOutcome<PathOf<H>, <H as GcHost>::Out>, GcError<<H as GcHost>::Failure>>;

/// Runs `gc --out`.
pub fn run_gc<H: GcHost>(
    host: &H,
    runner: &impl Runner,
    options: &GcOptions<H::Target>,
    observer: ObserverOf<'_, H>,
    cancel: &AtomicBool,
) -> GcResult<H> {
    let plan_error = |e: PlanError<H::Error>| match e {
        PlanError::Cancelled => GcError::Cancelled,
        PlanError::World(e) => GcError::Host(H::Failure::from(e)),
    };

    // 1. Output location.
    let out = host.check_output(&options.out)?;

    // 2. Locks.
    let mut forced = BTreeSet::new();
    let mut stops = BTreeSet::new();
    let locked = host.lock_folders()?;
    if locked.missing {
        stops.insert(GuardId::L1);
    }

    // 3. Trust checks before the long read of step 5, so that every stop
    // `--force` can continue past is known before it.
    let skip_t3 = options.allowed.contains(&GuardId::T3);
    stops.extend(trust_before_plan(host, options.gc_dims.as_ref(), skip_t3).stops());
    apply_force(stops, &options.allowed, &mut forced, observer)?;
    if let Some(v) = host.layout().data_version
        && v < DV_SESSION_LOCK_OS
    {
        observer.warn(Warning::PreLockVersion(v));
    }

    // 4. Source state, under the lock.
    let (walked, state) = host.capture()?;

    // 5. Plan.
    let plan_options = PlanOptions {
        gc_dims: options.gc_dims.clone(),
    };
    let plan = build_plan(host, runner, &plan_options, observer, cancel).map_err(plan_error)?;

    // 6. The same checks against the plan. They match step 3 unless a file
    // changed meanwhile, e.g. under a running game when L-1 was forced.
    let trust = check_trust(host.source(), &plan);
    let new_stops = trust
        .stops()
        .into_iter()
        .filter(|id| !forced.contains(id))
        .collect();
    apply_force(new_stops, &options.allowed, &mut forced, observer)?;

    // 7. Free space.
    let needed = space_needed(plan.estimate.total_output_bytes);
    let available = host.available_space(&out)?;
    if needed > available {
        return Err(GcError::InsufficientSpace { needed, available });
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(GcError::Cancelled);
    }

    // 8-10. Write, verify, record. A failure leaves a failure record behind.
    host.create_output(&out)?;
    let result = (|| {
        let written = host
            .write(&plan, &walked, &out, observer, cancel)
            .map_err(|e| (Stage::Write, e))?;
        host.verify(&plan, &walked, &written, &out, &state, observer, cancel)
            .map_err(|e| (Stage::Verify, e))?;
        host.record(&out, &state, &plan, &forced)
            .map_err(|e| (Stage::Record, e))?;
        Ok::<_, (Stage, H::Failure)>(written)
    })();
    let written = match result {
        Ok(written) => written,
        Err((_, e)) if H::is_cancelled(&e) => return Err(GcError::Cancelled),
        Err((stage, e)) => {
            // The missing completion record already marks the output as incomplete.
            host.record_failure(&out, stage, &e);
            return Err(GcError::Host(e));
        }
    };
    drop(locked);
    Ok(GcOutcome {
        lock_skipped: forced.contains(&GuardId::L1),
        plan,
        trust,
        forced,
        out,
        owner_not_preserved: H::owner_not_preserved(&written),
    })
}

/// Stops with every stop that `--force` did not allow. Otherwise records the
/// allowed ones as forced and warns about each.
fn apply_force<F, P, L>(
    stops: BTreeSet<GuardId>,
    allowed: &BTreeSet<GuardId>,
    forced: &mut BTreeSet<GuardId>,
    observer: &dyn crate::observe::Observer<Warning<P, L>>,
) -> Result<(), GcError<F>> {
    let (ignored, stopping): (BTreeSet<_>, BTreeSet<_>) =
        stops.into_iter().partition(|id| allowed.contains(id));
    if !stopping.is_empty() {
        return Err(GcError::Stopped(stopping.into_iter().collect()));
    }
    for id in ignored {
        forced.insert(id);
        observer.warn(match id {
            GuardId::L1 => Warning::LockSkipped,
            _ => Warning::Forced(id),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::observe::{Observer, Progress};

    #[derive(Default)]
    struct Warnings(Mutex<Vec<String>>);

    impl Observer<Warning<(), ()>> for Warnings {
        fn progress(&self, _: Progress) {}
        fn warn(&self, w: Warning<(), ()>) {
            self.0.lock().unwrap().push(format!("{w:?}"));
        }
    }

    #[test]
    fn force_allows_only_listed_stops_and_warns_once_each() {
        let warnings = Warnings::default();
        let mut forced = BTreeSet::new();
        let stops = BTreeSet::from([GuardId::L1, GuardId::T2]);
        let allowed = BTreeSet::from([GuardId::L1, GuardId::T2, GuardId::T5]);
        apply_force::<(), _, _>(stops, &allowed, &mut forced, &warnings).unwrap();
        assert_eq!(forced, BTreeSet::from([GuardId::L1, GuardId::T2]));
        assert_eq!(*warnings.0.lock().unwrap(), ["Forced(T2)", "LockSkipped"]);

        let stops = BTreeSet::from([GuardId::T1, GuardId::T3, GuardId::T5]);
        let result = apply_force::<(), _, _>(stops, &allowed, &mut forced, &warnings);
        assert!(matches!(result, Err(GcError::Stopped(ids)) if ids == [GuardId::T1, GuardId::T3]));
    }

    #[test]
    fn space_needed_adds_a_percent_and_a_margin() {
        assert_eq!(space_needed(0), SPACE_MARGIN);
        assert_eq!(space_needed(1000), 1010 + SPACE_MARGIN);
    }
}
