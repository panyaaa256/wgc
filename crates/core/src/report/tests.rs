use std::io::Write;

use serde_json::Value;

use super::*;
use crate::layout::test_util::{gzip, write};
use crate::lock::SessionLock;
use crate::nbt::test_util::{V, doc};
use crate::region::test_util::{build, chunk};
use crate::types::NoopObserver;

fn zlib(bytes: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(bytes).unwrap();
    e.finish().unwrap()
}

fn region(times: &[(i64, i32)]) -> Vec<u8> {
    let chunks: Vec<_> = times
        .iter()
        .enumerate()
        .map(|(i, (t, dv))| {
            let nbt = doc(vec![
                ("DataVersion", V::Int(*dv)),
                ("InhabitedTime", V::Long(*t)),
            ]);
            chunk(i as u16, 2, &zlib(&nbt))
        })
        .collect();
    build(&chunks)
}

fn world() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    let level = doc(vec![(
        "Data",
        V::Compound(vec![("DataVersion", V::Int(4325))]),
    )]);
    write(&t.path().join("level.dat"), &gzip(&level));
    write(&t.path().join("session.lock"), b"x");
    write(
        &t.path().join("region/r.0.0.mca"),
        &region(&[(0, 4325), (100, 3000), (-5, 4325)]),
    );
    let mut bad = region(&[(1, 4325)]);
    bad.truncate(8192 + 2);
    write(&t.path().join("region/r.1.0.mca"), &bad);
    write(&t.path().join("poi/r.5.5.mca"), b"");
    t
}

fn scan(path: &Path) -> ScanReport {
    let layout = crate::layout::detect(path).unwrap();
    run_scan(&layout, &NoopObserver, &AtomicBool::new(false)).unwrap()
}

fn json(report: &ScanReport) -> Value {
    serde_json::to_value(report).unwrap()
}

#[test]
fn bucket_boundaries() {
    let cases = [
        (-1, "negative"),
        (0, "zero"),
        (1, "lt_1s"),
        (19, "lt_1s"),
        (20, "lt_10s"),
        (199, "lt_10s"),
        (200, "lt_1m"),
        (1_199, "lt_1m"),
        (1_200, "lt_10m"),
        (11_999, "lt_10m"),
        (12_000, "lt_1h"),
        (71_999, "lt_1h"),
        (72_000, "lt_10h"),
        (719_999, "lt_10h"),
        (720_000, "ge_10h"),
    ];
    for (ticks, bucket) in cases {
        let mut b = InhabitedBuckets::default();
        b.add(&if ticks < 0 {
            Inhabited::Negative(ticks)
        } else {
            Inhabited::Value(ticks)
        });
        let v = serde_json::to_value(b).unwrap();
        assert_eq!(v[bucket], 1, "{ticks} -> {bucket}");
        assert_eq!(
            v.as_object().unwrap().values().filter(|n| *n == 1).count(),
            1
        );
    }
}

#[test]
fn report_shape() {
    let t = world();
    let v = json(&scan(t.path()));
    assert_eq!(v["schema"], "wgc.scan/1");
    assert_eq!(v["layout"]["kind"], "vanilla_legacy");
    assert_eq!(v["layout"]["world_folders"], serde_json::json!(["."]));
    assert_eq!(v["world_data_version"], 4325);
    assert_eq!(v["lock"], "acquired");
    assert_eq!(v["trust"][0]["id"], "T-1");
    assert_eq!(v["trust"][0]["status"], "not_applicable");
    assert_eq!(v["trust"][2]["status"], "unknown");
    assert_eq!(v["trust"][2]["reason"]["kind"], "no_players_checked");
    assert_eq!(
        v["notices"],
        serde_json::json!([
            "pregenerated_chunks",
            "boundary_general",
            "run_official_optimize"
        ])
    );

    let ow = &v["dimensions"][0];
    assert_eq!(ow["id"], "minecraft:overworld");
    assert_eq!(ow["dir"], ".");
    assert_eq!(ow["chunks"]["total"], 3); // the truncated chunk in r.1.0 is not readable
    assert_eq!(ow["chunks"]["older_data_version"], 1);
    assert_eq!(ow["inhabited_time"]["zero"], 1);
    assert_eq!(ow["inhabited_time"]["negative"], 1);
    assert_eq!(ow["gc"]["delete"], 1);
    assert_eq!(ow["integrity"]["structural"][0]["path"], "region/r.1.0.mca");
    assert_eq!(ow["integrity"]["structural"][0]["id"], "S-4");
    assert_eq!(ow["integrity"]["structural"][0]["index"], 0);
    assert_eq!(
        ow["integrity"]["verbatim_triples"],
        serde_json::json!([[1, 0]])
    );
    assert_eq!(ow["empty_files"], serde_json::json!(["poi/r.5.5.mca"]));
}

#[test]
fn json_is_deterministic() {
    let t = world();
    let a = serde_json::to_string(&scan(t.path())).unwrap();
    let b = serde_json::to_string(&scan(t.path())).unwrap();
    assert_eq!(a, b);
}

#[test]
fn lock_unavailable_is_reported() {
    struct Count(std::sync::Mutex<u32>);
    impl Observer for Count {
        fn progress(&self, _: crate::types::Progress) {}
        fn warn(&self, w: Warning) {
            assert!(matches!(w, Warning::LockUnavailable(_)));
            *self.0.lock().unwrap() += 1;
        }
    }
    let t = world();
    let _held = SessionLock::try_acquire(t.path()).unwrap();
    let layout = crate::layout::detect(t.path()).unwrap();
    let count = Count(std::sync::Mutex::new(0));
    let report = run_scan(&layout, &count, &AtomicBool::new(false)).unwrap();
    assert_eq!(report.lock, LockStatus::InUse);
    assert_eq!(*count.0.lock().unwrap(), 1);
}

#[test]
fn gc_report_adds_its_fields() {
    let t = world();
    let layout = crate::layout::detect(t.path()).unwrap();
    let plan = build_plan(
        &layout,
        &PlanOptions::default(),
        &NoopObserver,
        &AtomicBool::new(false),
    )
    .unwrap();
    let scan = scan_report(&plan, &check_trust(&plan), LockStatus::Acquired).unwrap();
    let gc = gc_report(scan, None, &BTreeSet::from([GuardId::T3]), &plan).unwrap();
    let v = serde_json::to_value(&gc).unwrap();
    assert_eq!(v["schema"], "wgc.gc/1");
    assert_eq!(v["mode"], "estimate");
    assert_eq!(v["forced"], serde_json::json!(["T-3"]));
    assert_eq!(v["dimensions_targeted"], Value::Null);
    assert_eq!(v["dimensions"][0]["id"], "minecraft:overworld");
}

#[test]
fn relative_paths() {
    let root = Path::new("/srv/mc");
    assert_eq!(rel_path(root, root).unwrap(), ".");
    assert_eq!(
        rel_path(root, &root.join("world/region/r.0.0.mca")).unwrap(),
        "world/region/r.0.0.mca"
    );
    assert_eq!(
        rel_path(root, Path::new("/elsewhere/x")).unwrap(),
        "/elsewhere/x"
    );
}

#[cfg(unix)]
#[test]
fn non_utf8_path_is_an_error() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let root = Path::new("/srv");
    let bad = root.join(OsStr::from_bytes(b"w\xffrld"));
    assert!(matches!(
        rel_path(root, &bad),
        Err(ReportError::NonUtf8Path(_))
    ));
}
