use std::collections::BTreeSet;
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use super::*;
use crate::layout::test_util::{gzip, write};
use crate::nbt::test_util::{V, doc};
use crate::region::RegionFile;
use crate::region::test_util::{build, chunk};
use crate::types::{NoopObserver, Progress};

fn zlib(bytes: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(bytes).unwrap();
    e.finish().unwrap()
}

fn region(times: &[i64]) -> Vec<u8> {
    let chunks: Vec<_> = times
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let nbt = doc(vec![
                ("DataVersion", V::Int(4325)),
                ("InhabitedTime", V::Long(*t)),
            ]);
            chunk(i as u16, 2, &zlib(&nbt))
        })
        .collect();
    build(&chunks)
}

const TEMP: &str = "playerdata/0123abcd-0000-0000-0000-00000000beef-1.dat";

/// A vanilla world: chunk 0 is deleted, chunk 1 kept; entities for both.
fn world() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    let w = t.path();
    write(
        &w.join("level.dat"),
        &gzip(&doc(vec![(
            "Data",
            V::Compound(vec![("DataVersion", V::Int(4325))]),
        )])),
    );
    write(&w.join("session.lock"), "\u{2603}".as_bytes());
    write(&w.join("region/r.0.0.mca"), &region(&[0, 100]));
    write(
        &w.join("entities/r.0.0.mca"),
        &build(&[chunk(0, 2, b"e0"), chunk(1, 2, b"e1")]),
    );
    write(&w.join("region/r.1.0.mca"), &region(&[0])); // fully deleted
    write(&w.join("data/raids.dat"), b"raids");
    write(&w.join(TEMP), b"");
    std::fs::create_dir_all(w.join("datapacks")).unwrap();
    t
}

/// T-3 is unknown in these worlds (no players), so tests allow it.
fn options(out: &Path) -> GcOptions {
    GcOptions {
        out: out.to_path_buf(),
        gc_dims: None,
        allowed: BTreeSet::from([GuardId::T3]),
    }
}

fn run(world: &Path, out: &Path) -> Result<GcOutcome, OutputError> {
    let layout = crate::layout::detect(world).unwrap();
    run_gc(
        &layout,
        &options(out),
        &NoopObserver,
        &AtomicBool::new(false),
    )
}

#[test]
fn writes_and_verifies_the_output() {
    let w = world();
    let o = tempfile::tempdir().unwrap();
    let out = o.path().join("out");
    let outcome = run(w.path(), &out).unwrap();
    assert_eq!(outcome.forced, BTreeSet::from([GuardId::T3]));

    // Rewritten region keeps only chunk 1; the fully deleted region is gone.
    let packed = RegionFile::parse(
        crate::types::FileKind::Region,
        crate::types::RegionPos::new(0, 0),
        std::fs::read(out.join("region/r.0.0.mca")).unwrap(),
        &BTreeSet::new(),
    );
    assert_eq!(packed.entries().len(), 1);
    assert_eq!(packed.entries()[0].index.get(), 1);
    assert!(!out.join("region/r.1.0.mca").exists());
    let entities = std::fs::read(out.join("entities/r.0.0.mca")).unwrap();
    assert!(entities.windows(2).any(|w| w == b"e1") && !entities.windows(2).any(|w| w == b"e0"));

    // Other files are copied, session.lock and the temp file are not; empty dirs exist.
    assert_eq!(std::fs::read(out.join("data/raids.dat")).unwrap(), b"raids");
    assert!(!out.join("session.lock").exists());
    assert!(!out.join(TEMP).exists());
    assert!(out.join("datapacks").is_dir());

    // Records.
    let complete: Complete =
        serde_json::from_slice(&std::fs::read(out.join(".wgc/complete.json")).unwrap()).unwrap();
    assert_eq!(complete.forced, ["T-3"]);
    assert_eq!(complete.summary.deleted_chunks, 2);
    let state: SourceState =
        serde_json::from_slice(&std::fs::read(out.join(".wgc/source-state.json")).unwrap())
            .unwrap();
    assert!(state.files.iter().any(|f| f.path == "session.lock"));
    assert!(!out.join(".wgc/failure.json").exists());

    // Metadata.
    let a = std::fs::metadata(w.path().join("data/raids.dat")).unwrap();
    let b = std::fs::metadata(out.join("data/raids.dat")).unwrap();
    assert_eq!(a.modified().unwrap(), b.modified().unwrap());
    assert_eq!(a.permissions(), b.permissions());

    // The source is untouched (verify sees no change).
    assert!(verify(&out, None).is_ok());
}

#[test]
fn bad_output_folders_are_refused_before_writing() {
    let w = world();
    let o = tempfile::tempdir().unwrap();
    write(&o.path().join("taken/file"), b"x");
    assert!(matches!(
        run(w.path(), &o.path().join("taken")),
        Err(OutputError::OutputNotEmpty { .. })
    ));
    assert!(matches!(
        run(w.path(), &w.path().join("inside")),
        Err(OutputError::OutputOverlapsSource { .. })
    ));
    assert!(!w.path().join("inside").exists());
    // An output folder that contains the world is refused.
    assert!(run(&w.path().join("."), w.path().parent().unwrap()).is_err());
}

#[test]
fn trust_stops_unless_forced() {
    let w = world();
    let o = tempfile::tempdir().unwrap();
    let layout = crate::layout::detect(w.path()).unwrap();
    let mut opts = options(&o.path().join("out"));
    opts.allowed.clear();
    let err = run_gc(&layout, &opts, &NoopObserver, &AtomicBool::new(false)).unwrap_err();
    assert!(matches!(err, OutputError::Stopped(ids) if ids == [GuardId::T3]));
    assert!(!o.path().join("out").exists());
}

#[test]
fn missing_session_lock_is_l1() {
    let w = world();
    std::fs::remove_file(w.path().join("session.lock")).unwrap();
    let o = tempfile::tempdir().unwrap();
    assert!(
        matches!(run(w.path(), &o.path().join("a")), Err(OutputError::Stopped(ids)) if ids == [GuardId::L1])
    );

    let layout = crate::layout::detect(w.path()).unwrap();
    let mut opts = options(&o.path().join("b"));
    opts.allowed.insert(GuardId::L1);
    let outcome = run_gc(&layout, &opts, &NoopObserver, &AtomicBool::new(false)).unwrap();
    assert!(outcome.forced.contains(&GuardId::L1));
}

#[test]
fn world_in_use_is_refused() {
    let w = world();
    let _held = crate::lock::SessionLock::try_acquire(w.path()).unwrap();
    let o = tempfile::tempdir().unwrap();
    assert!(matches!(
        run(w.path(), &o.path().join("out")),
        Err(OutputError::Lock(_))
    ));
}

/// Runs a closure on the first progress event of a kind.
struct Hook<F: Fn(&Progress) -> bool + Send + Sync> {
    on: F,
    fired: Mutex<bool>,
}

impl<F: Fn(&Progress) -> bool + Send + Sync> crate::types::Observer for Hook<F> {
    fn progress(&self, p: Progress) {
        let mut fired = self.fired.lock().unwrap();
        if !*fired && (self.on)(&p) {
            *fired = true;
        }
    }
    fn warn(&self, _: crate::types::Warning) {}
}

#[test]
fn cancellation_during_writing_leaves_an_incomplete_output() {
    let w = world();
    let o = tempfile::tempdir().unwrap();
    let out = o.path().join("out");
    let cancel = AtomicBool::new(false);
    let hook = Hook {
        on: |p: &Progress| {
            if matches!(p, Progress::Writing { .. }) {
                cancel.store(true, Ordering::Relaxed);
                true
            } else {
                false
            }
        },
        fired: Mutex::new(false),
    };
    let layout = crate::layout::detect(w.path()).unwrap();
    let result = run_gc(&layout, &options(&out), &hook, &cancel);
    assert!(matches!(result, Err(OutputError::Cancelled)));
    assert!(!out.join(".wgc/complete.json").exists());
    assert!(!out.join(".wgc/failure.json").exists());
    // The lock was released.
    assert!(!crate::lock::SessionLock::is_locked(w.path()).unwrap());
}

#[test]
fn source_change_during_the_run_fails_verification() {
    let w = world();
    let o = tempfile::tempdir().unwrap();
    let out = o.path().join("out");
    let level = w.path().join("level.dat");
    let hook = Hook {
        on: |p: &Progress| {
            if matches!(p, Progress::Verifying { .. }) {
                let mut f = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&level)
                    .unwrap();
                f.write_all(b"x").unwrap();
                true
            } else {
                false
            }
        },
        fired: Mutex::new(false),
    };
    let layout = crate::layout::detect(w.path()).unwrap();
    let result = run_gc(&layout, &options(&out), &hook, &AtomicBool::new(false));
    assert!(
        matches!(&result, Err(OutputError::Verification(f)) if f.iter().any(|f| f.check == "V-6"))
    );
    assert!(!out.join(".wgc/complete.json").exists());
    let failure: Failure =
        serde_json::from_slice(&std::fs::read(out.join(".wgc/failure.json")).unwrap()).unwrap();
    assert_eq!(failure.stage, "verify");
    assert!(failure.items.iter().any(|i| i.check == "V-6"));
}

#[test]
fn verify_detects_changes_locks_and_incomplete_outputs() {
    let w = world();
    let o = tempfile::tempdir().unwrap();
    let out = o.path().join("out");
    run(w.path(), &out).unwrap();
    assert!(verify(&out, Some(w.path())).is_ok());

    let other = tempfile::tempdir().unwrap();
    assert!(matches!(
        verify(&out, Some(other.path())),
        Err(VerifyError::SourceMismatch { .. })
    ));

    {
        let _held = crate::lock::SessionLock::try_acquire(w.path()).unwrap();
        // Our own lock counts as in use.
        assert!(matches!(verify(&out, None), Err(VerifyError::InUse { .. })));
    }

    std::fs::write(w.path().join("level.dat"), b"changed").unwrap();
    assert!(
        matches!(verify(&out, None), Err(VerifyError::Changed(c)) if c == [ChangedFile::Modified("level.dat".into())])
    );

    std::fs::remove_file(out.join(".wgc/complete.json")).unwrap();
    assert!(matches!(
        verify(&out, None),
        Err(VerifyError::Incomplete { .. })
    ));
}

#[test]
fn records_of_an_earlier_run_are_not_copied() {
    let w = world();
    write(&w.path().join(".wgc/complete.json"), b"{}");
    write(&w.path().join(".wgc/source-state.json"), b"{}");
    let o = tempfile::tempdir().unwrap();
    let out = o.path().join("out");

    // Cancelled during writing: no stale complete.json may appear in the output.
    let cancel = AtomicBool::new(false);
    let hook = Hook {
        on: |p: &Progress| {
            if matches!(p, Progress::Writing { .. }) {
                cancel.store(true, Ordering::Relaxed);
                true
            } else {
                false
            }
        },
        fired: Mutex::new(false),
    };
    let layout = crate::layout::detect(w.path()).unwrap();
    assert!(matches!(
        run_gc(&layout, &options(&out), &hook, &cancel),
        Err(OutputError::Cancelled)
    ));
    assert!(!out.join(".wgc/complete.json").exists());
    assert!(matches!(
        verify(&out, None),
        Err(VerifyError::Incomplete { .. })
    ));

    // A full run writes its own records only.
    let out2 = o.path().join("out2");
    run(w.path(), &out2).unwrap();
    let complete = std::fs::read_to_string(out2.join(".wgc/complete.json")).unwrap();
    assert!(complete.contains("wgc.complete/1"));
}

#[test]
fn dot_dot_after_the_existing_part_is_refused() {
    let w = world();
    let o = tempfile::tempdir().unwrap();
    let sneaky = o.path().join("missing/../../escape");
    assert!(matches!(
        run(w.path(), &sneaky),
        Err(OutputError::InvalidOutputPath { .. })
    ));
    assert!(!o.path().join("missing").exists());
}

#[test]
fn failure_json_makes_verify_incomplete() {
    let w = world();
    let o = tempfile::tempdir().unwrap();
    let out = o.path().join("out");
    run(w.path(), &out).unwrap();
    write(&out.join(".wgc/failure.json"), b"{}");
    assert!(matches!(
        verify(&out, None),
        Err(VerifyError::Incomplete { .. })
    ));
}
