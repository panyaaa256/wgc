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
    assert!(
        !v["notices"]
            .as_array()
            .unwrap()
            .contains(&Value::from("no_trash"))
    );

    let scan = scan_report(&plan, &check_trust(&plan), LockStatus::Acquired).unwrap();
    let out = tempfile::tempdir().unwrap();
    let gc = gc_report(scan, Some(&out.path().join(".")), &BTreeSet::new(), &plan).unwrap();
    let v = serde_json::to_value(&gc).unwrap();
    assert_eq!(v["mode"], "output");
    assert_eq!(
        v["out"],
        std::fs::canonicalize(out.path()).unwrap().to_str().unwrap()
    );
    assert!(
        v["notices"]
            .as_array()
            .unwrap()
            .contains(&Value::from("no_trash"))
    );
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
    assert_eq!(
        rel_path(root, Path::new("/srv/mcx/y")).unwrap(),
        "/srv/mcx/y"
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

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

fn scan_fixture() -> ScanReport {
    let busy = DimensionReport {
        id: "minecraft:overworld".to_owned(),
        dir: "world".to_owned(),
        size: KindBytes {
            region: 1_048_576,
            entities: 65_536,
            poi: 8_192,
        },
        chunks: ChunkCounts {
            total: 175,
            older_data_version: 12,
        },
        inhabited_time: InhabitedBuckets {
            zero: 120,
            missing: 1,
            negative: 2,
            unreadable: 3,
            lt_1s: 4,
            lt_10s: 5,
            lt_1m: 6,
            lt_10m: 7,
            lt_1h: 8,
            lt_10h: 9,
            ge_10h: 10,
        },
        gc: GcSummary {
            delete: 100,
            protected: Protected {
                forceload: 4,
                structure_ref: 16,
            },
            size_after: KindBytes {
                region: 524_288,
                entities: 32_768,
                poi: 4_096,
            },
            emptied_files: strings(&["world/region/r.1.1.mca"]),
        },
        integrity: Integrity {
            structural: [
                (StructuralId::S1, "world/region/r.0.0.mca", None),
                (StructuralId::S2, "world/region/r.0.0.mca", Some(0)),
                (StructuralId::S3, "world/region/r.0.0.mca", Some(1)),
                (StructuralId::S4, "world/region/r.0.0.mca", Some(2)),
                (StructuralId::S5, "world/region/r.0.0.mca", Some(3)),
                (StructuralId::S6, "world/region/r.0.0.mca", Some(4)),
                (StructuralId::S7, "world/region/r.0.0.mca", Some(5)),
                (StructuralId::S8, "world/region/r.0.0.mca", Some(1023)),
            ]
            .into_iter()
            .map(|(id, path, index)| StructuralJson {
                path: path.to_owned(),
                id,
                index,
            })
            .collect(),
            content: vec![
                ContentJson {
                    pos: [3, -4],
                    kind: ContentKind::Decompress,
                    detail: "invalid zlib stream".to_owned(),
                },
                ContentJson {
                    pos: [-1, 31],
                    kind: ContentKind::Nbt,
                    detail: "unexpected end of data".to_owned(),
                },
            ],
            verbatim_triples: vec![[0, 0], [-1, 2]],
        },
        orphans: OrphanCounts {
            entities: 7,
            poi: 9,
        },
        empty_files: strings(&["world/region/r.2.2.mca"]),
        boundary_warnings: vec![
            BoundaryJson {
                chunks: 24,
                min: [-3, 0],
                max: [5, 7],
            },
            BoundaryJson {
                chunks: 1,
                min: [10, 10],
                max: [10, 10],
            },
        ],
    };
    let quiet = DimensionReport {
        id: "minecraft:the_nether".to_owned(),
        dir: "world_nether/DIM-1".to_owned(),
        size: KindBytes::default(),
        chunks: ChunkCounts::default(),
        inhabited_time: InhabitedBuckets::default(),
        gc: GcSummary {
            delete: 0,
            protected: Protected::default(),
            size_after: KindBytes::default(),
            emptied_files: Vec::new(),
        },
        integrity: Integrity {
            structural: Vec::new(),
            content: Vec::new(),
            verbatim_triples: Vec::new(),
        },
        orphans: OrphanCounts::default(),
        empty_files: Vec::new(),
        boundary_warnings: Vec::new(),
    };
    ScanReport {
        schema: "wgc.scan/1",
        tool_version: "0.1.0",
        layout: LayoutReport {
            kind: "bukkit_legacy",
            base: Some("world".to_owned()),
            world_folders: strings(&["world", "world_nether"]),
        },
        world_data_version: Some(4189),
        lock: LockStatus::Acquired,
        trust: vec![
            TrustItem {
                id: GuardId::T1,
                status: TrustStatus::Pass,
                details: Vec::new(),
                reason: None,
            },
            TrustItem {
                id: GuardId::T2,
                status: TrustStatus::NotApplicable,
                details: Vec::new(),
                reason: None,
            },
            TrustItem {
                id: GuardId::T3,
                status: TrustStatus::Fail,
                details: vec![DetailJson::PlayerInZeroChunk {
                    uuid: "069a79f4-44e9-4726-a5be-fca90e38aaf5".to_owned(),
                    dimension: "minecraft:overworld".to_owned(),
                    pos: [-12, 40],
                }],
                reason: None,
            },
            TrustItem {
                id: GuardId::T4,
                status: TrustStatus::Unknown,
                details: Vec::new(),
                reason: Some(ReasonJson::UnknownVersion { data_version: 9999 }),
            },
            TrustItem {
                id: GuardId::T5,
                status: TrustStatus::Unknown,
                details: Vec::new(),
                reason: Some(ReasonJson::ForcedChunksUnknown {
                    dimensions: strings(&["minecraft:overworld", "minecraft:the_end"]),
                }),
            },
        ],
        other_files: OtherFiles {
            omitted: vec![
                OmittedJson {
                    path: "world/data/empty.dat".to_owned(),
                    reason: OmitJson::EmptyFile,
                },
                OmittedJson {
                    path: "world/region/r.2.2.mca".to_owned(),
                    reason: OmitJson::NoEntriesLeft,
                },
                OmittedJson {
                    path: "world/playerdata/069a79f4-44e9-4726-a5be-fca90e38aaf5-123.dat"
                        .to_owned(),
                    reason: OmitJson::PlayerTempFile,
                },
            ],
            dat_old_without_dat: strings(&[
                "world/playerdata/8667ba71-b85a-4004-af54-457a9734eed7.dat_old",
            ]),
        },
        notices: vec![
            Notice::PregeneratedChunks,
            Notice::BoundaryGeneral,
            Notice::RunOfficialOptimize,
        ],
        dimensions: vec![busy, quiet],
    }
}

#[test]
fn scan_json_golden() {
    assert_eq!(
        serde_json::to_string(&scan_fixture()).unwrap(),
        concat!(
            r#"{"schema":"wgc.scan/1","tool_version":"0.1.0","#,
            r#""layout":{"kind":"bukkit_legacy","base":"world","world_folders":["world","world_nether"]},"#,
            r#""world_data_version":4189,"lock":"acquired","#,
            r#""trust":["#,
            r#"{"id":"T-1","status":"pass"},"#,
            r#"{"id":"T-2","status":"not_applicable"},"#,
            r#"{"id":"T-3","status":"fail","details":[{"kind":"player_in_zero_chunk","uuid":"069a79f4-44e9-4726-a5be-fca90e38aaf5","dimension":"minecraft:overworld","pos":[-12,40]}]},"#,
            r#"{"id":"T-4","status":"unknown","reason":{"kind":"unknown_version","data_version":9999}},"#,
            r#"{"id":"T-5","status":"unknown","reason":{"kind":"forced_chunks_unknown","dimensions":["minecraft:overworld","minecraft:the_end"]}}],"#,
            r#""other_files":{"omitted":["#,
            r#"{"path":"world/data/empty.dat","reason":"empty_file"},"#,
            r#"{"path":"world/region/r.2.2.mca","reason":"no_entries_left"},"#,
            r#"{"path":"world/playerdata/069a79f4-44e9-4726-a5be-fca90e38aaf5-123.dat","reason":"player_temp_file"}],"#,
            r#""dat_old_without_dat":["world/playerdata/8667ba71-b85a-4004-af54-457a9734eed7.dat_old"]},"#,
            r#""notices":["pregenerated_chunks","boundary_general","run_official_optimize"],"#,
            r#""dimensions":["#,
            r#"{"id":"minecraft:overworld","dir":"world","#,
            r#""size":{"region":1048576,"entities":65536,"poi":8192},"#,
            r#""chunks":{"total":175,"older_data_version":12},"#,
            r#""inhabited_time":{"zero":120,"missing":1,"negative":2,"unreadable":3,"lt_1s":4,"lt_10s":5,"lt_1m":6,"lt_10m":7,"lt_1h":8,"lt_10h":9,"ge_10h":10},"#,
            r#""gc":{"delete":100,"protected":{"forceload":4,"structure_ref":16},"size_after":{"region":524288,"entities":32768,"poi":4096},"emptied_files":["world/region/r.1.1.mca"]},"#,
            r#""integrity":{"structural":["#,
            r#"{"path":"world/region/r.0.0.mca","id":"S-1","index":null},"#,
            r#"{"path":"world/region/r.0.0.mca","id":"S-2","index":0},"#,
            r#"{"path":"world/region/r.0.0.mca","id":"S-3","index":1},"#,
            r#"{"path":"world/region/r.0.0.mca","id":"S-4","index":2},"#,
            r#"{"path":"world/region/r.0.0.mca","id":"S-5","index":3},"#,
            r#"{"path":"world/region/r.0.0.mca","id":"S-6","index":4},"#,
            r#"{"path":"world/region/r.0.0.mca","id":"S-7","index":5},"#,
            r#"{"path":"world/region/r.0.0.mca","id":"S-8","index":1023}],"#,
            r#""content":["#,
            r#"{"pos":[3,-4],"kind":"decompress","detail":"invalid zlib stream"},"#,
            r#"{"pos":[-1,31],"kind":"nbt","detail":"unexpected end of data"}],"#,
            r#""verbatim_triples":[[0,0],[-1,2]]},"#,
            r#""orphans":{"entities":7,"poi":9},"#,
            r#""empty_files":["world/region/r.2.2.mca"],"#,
            r#""boundary_warnings":[{"chunks":24,"min":[-3,0],"max":[5,7]},{"chunks":1,"min":[10,10],"max":[10,10]}]},"#,
            r#"{"id":"minecraft:the_nether","dir":"world_nether/DIM-1","#,
            r#""size":{"region":0,"entities":0,"poi":0},"#,
            r#""chunks":{"total":0,"older_data_version":0},"#,
            r#""inhabited_time":{"zero":0,"missing":0,"negative":0,"unreadable":0,"lt_1s":0,"lt_10s":0,"lt_1m":0,"lt_10m":0,"lt_1h":0,"lt_10h":0,"ge_10h":0},"#,
            r#""gc":{"delete":0,"protected":{"forceload":0,"structure_ref":0},"size_after":{"region":0,"entities":0,"poi":0},"emptied_files":[]},"#,
            r#""integrity":{"structural":[],"content":[],"verbatim_triples":[]},"#,
            r#""orphans":{"entities":0,"poi":0},"#,
            r#""empty_files":[],"boundary_warnings":[]}]}"#,
        )
    );
}

#[test]
fn gc_json_golden() {
    let mut scan = scan_fixture();
    scan.layout = LayoutReport {
        kind: "vanilla",
        base: None,
        world_folders: strings(&["."]),
    };
    scan.schema = "wgc.gc/1";
    scan.world_data_version = None;
    scan.lock = LockStatus::Missing;
    scan.trust = vec![
        TrustItem {
            id: GuardId::T1,
            status: TrustStatus::Fail,
            details: vec![DetailJson::PaperFixedInhabitedTime {
                file: "config/paper-world-defaults.yml".to_owned(),
                value: 0,
            }],
            reason: None,
        },
        TrustItem {
            id: GuardId::T2,
            status: TrustStatus::Fail,
            details: vec![DetailJson::MobSpawningDisabled {
                world_folder: "world".to_owned(),
            }],
            reason: None,
        },
        TrustItem {
            id: GuardId::T3,
            status: TrustStatus::Unknown,
            details: Vec::new(),
            reason: Some(ReasonJson::NoPlayersChecked),
        },
        TrustItem {
            id: GuardId::T4,
            status: TrustStatus::Unknown,
            details: Vec::new(),
            reason: Some(ReasonJson::UnreadableFile {
                path: "level.dat".to_owned(),
            }),
        },
        TrustItem {
            id: GuardId::T5,
            status: TrustStatus::Unknown,
            details: Vec::new(),
            reason: Some(ReasonJson::UnexpectedValue {
                path: "data/chunks.dat".to_owned(),
                key: "Forced".to_owned(),
            }),
        },
    ];
    scan.other_files = OtherFiles {
        omitted: Vec::new(),
        dat_old_without_dat: Vec::new(),
    };
    scan.notices = vec![Notice::PregeneratedChunks];
    scan.dimensions.clear();

    let estimate = GcReport {
        scan: scan.clone(),
        mode: GcMode::Estimate,
        out: None,
        forced: Vec::new(),
        dimensions_targeted: None,
    };
    assert_eq!(
        serde_json::to_string(&estimate).unwrap(),
        concat!(
            r#"{"schema":"wgc.gc/1","tool_version":"0.1.0","#,
            r#""layout":{"kind":"vanilla","base":null,"world_folders":["."]},"#,
            r#""world_data_version":null,"lock":"missing","#,
            r#""trust":["#,
            r#"{"id":"T-1","status":"fail","details":[{"kind":"paper_fixed_inhabited_time","file":"config/paper-world-defaults.yml","value":0}]},"#,
            r#"{"id":"T-2","status":"fail","details":[{"kind":"mob_spawning_disabled","world_folder":"world"}]},"#,
            r#"{"id":"T-3","status":"unknown","reason":{"kind":"no_players_checked"}},"#,
            r#"{"id":"T-4","status":"unknown","reason":{"kind":"unreadable_file","path":"level.dat"}},"#,
            r#"{"id":"T-5","status":"unknown","reason":{"kind":"unexpected_value","path":"data/chunks.dat","key":"Forced"}}],"#,
            r#""other_files":{"omitted":[],"dat_old_without_dat":[]},"#,
            r#""notices":["pregenerated_chunks"],"dimensions":[],"#,
            r#""mode":"estimate","out":null,"forced":[],"dimensions_targeted":null}"#,
        )
    );

    let mut output = GcReport {
        scan,
        mode: GcMode::Output,
        out: Some("/abs/out".to_owned()),
        forced: vec![GuardId::T1, GuardId::T5, GuardId::L1],
        dimensions_targeted: Some(strings(&["minecraft:overworld"])),
    };
    output.scan.lock = LockStatus::Acquired;
    output.scan.notices.push(Notice::NoTrash);
    assert_eq!(
        serde_json::to_string(&output).unwrap(),
        concat!(
            r#"{"schema":"wgc.gc/1","tool_version":"0.1.0","#,
            r#""layout":{"kind":"vanilla","base":null,"world_folders":["."]},"#,
            r#""world_data_version":null,"lock":"acquired","#,
            r#""trust":["#,
            r#"{"id":"T-1","status":"fail","details":[{"kind":"paper_fixed_inhabited_time","file":"config/paper-world-defaults.yml","value":0}]},"#,
            r#"{"id":"T-2","status":"fail","details":[{"kind":"mob_spawning_disabled","world_folder":"world"}]},"#,
            r#"{"id":"T-3","status":"unknown","reason":{"kind":"no_players_checked"}},"#,
            r#"{"id":"T-4","status":"unknown","reason":{"kind":"unreadable_file","path":"level.dat"}},"#,
            r#"{"id":"T-5","status":"unknown","reason":{"kind":"unexpected_value","path":"data/chunks.dat","key":"Forced"}}],"#,
            r#""other_files":{"omitted":[],"dat_old_without_dat":[]},"#,
            r#""notices":["pregenerated_chunks","no_trash"],"dimensions":[],"#,
            r#""mode":"output","out":"/abs/out","forced":["T-1","T-5","L-1"],"#,
            r#""dimensions_targeted":["minecraft:overworld"]}"#,
        )
    );
}

#[test]
fn lock_status_json() {
    let all = [
        LockStatus::Acquired,
        LockStatus::InUse,
        LockStatus::Missing,
        LockStatus::NotWritable,
        LockStatus::Error,
    ];
    assert_eq!(
        serde_json::to_string(&all).unwrap(),
        r#"["acquired","in_use","missing","not_writable","error"]"#
    );
}
