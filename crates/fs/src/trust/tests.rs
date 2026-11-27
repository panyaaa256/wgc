use std::io::Write;
use std::sync::atomic::AtomicBool;

use super::*;
use crate::layout::test_util::{gzip, write};
use crate::nbt::test_util::{V, doc};
use crate::plan::{PlanOptions, build_plan};
use crate::region::test_util::{build, chunk};
use crate::types::NoopObserver;

fn zlib(bytes: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(bytes).unwrap();
    e.finish().unwrap()
}

/// Region bytes with chunk i at x = i, z = 0 having InhabitedTime `times[i]`.
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

fn level(path: &std::path::Path, data: Vec<(&'static str, V)>) {
    write(
        &path.join("level.dat"),
        &gzip(&doc(vec![("Data", V::Compound(data))])),
    );
}

fn player(dir: &std::path::Path, uuid: &str, dim: V, x: f64, game_type: i32) {
    let nbt = doc(vec![
        ("Pos", V::DoubleList(vec![x, 64.0, 1.0])),
        ("Dimension", dim),
        ("playerGameType", V::Int(game_type)),
    ]);
    write(&dir.join(format!("{uuid}.dat")), &gzip(&nbt));
}

const UUID_A: &str = "00000000-0000-0000-0000-00000000000a";
const UUID_B: &str = "00000000-0000-0000-0000-00000000000b";
const UUID_C: &str = "00000000-0000-0000-0000-00000000000c";
const UUID_D: &str = "00000000-0000-0000-0000-00000000000d";

fn report(path: &std::path::Path, options: PlanOptions) -> TrustReport {
    let layout = crate::layout::detect(path).unwrap();
    let plan = build_plan(&layout, &options, &NoopObserver, &AtomicBool::new(false)).unwrap();
    check_trust(&plan)
}

fn status(report: &TrustReport, id: GuardId) -> &CheckStatus {
    &report.checks.iter().find(|c| c.id == id).unwrap().status
}

/// A vanilla world: chunk (0,0) has 0 ticks, chunk (1,0) has 100.
fn vanilla() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    write(&t.path().join("region/r.0.0.mca"), &region(&[0, 100]));
    level(t.path(), vec![("DataVersion", V::Int(4325))]);
    t
}

/// A Bukkit server with world and world_nether, DataVersion 4325.
fn bukkit() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    let r = t.path();
    write(&r.join("server.properties"), b"level-name=world\n");
    write(&r.join("bukkit.yml"), b"");
    write(&r.join("world/region/r.0.0.mca"), &region(&[100]));
    write(
        &r.join("world_nether/DIM-1/region/r.0.0.mca"),
        &region(&[100]),
    );
    level(&r.join("world"), vec![("DataVersion", V::Int(4325))]);
    level(&r.join("world_nether"), vec![("DataVersion", V::Int(4325))]);
    player(
        &r.join("world/playerdata"),
        UUID_A,
        V::Str("minecraft:overworld"),
        20.0,
        0,
    );
    t
}

#[test]
fn t1_paper_fixed_inhabited_time() {
    let t = bukkit();
    let r = t.path();
    let defaults = r.join("config/paper-world-defaults.yml");
    let world = r.join("world/paper-world.yml");
    let t1 = |r: &std::path::Path| status(&report(r, PlanOptions::default()), GuardId::T1).clone();

    assert_eq!(t1(r), CheckStatus::Pass); // no files
    write(&world, b"chunks:\n  fixed-chunk-inhabited-time: -1\n");
    assert_eq!(t1(r), CheckStatus::Pass);
    write(&world, b"chunks:\n  fixed-chunk-inhabited-time: 0\n");
    assert_eq!(
        t1(r),
        CheckStatus::Fail(vec![TrustDetail::PaperFixedInhabitedTime {
            file: fs::canonicalize(r).unwrap().join("world/paper-world.yml"),
            value: 0
        }])
    );
    write(&world, b"chunks:\n  fixed-chunk-inhabited-time: default\n");
    write(&defaults, b"chunks:\n  fixed-chunk-inhabited-time: 5\n");
    assert!(matches!(t1(r), CheckStatus::Fail(d) if d.len() == 1));
    write(&defaults, b"chunks:\n  other: 1\n");
    assert_eq!(t1(r), CheckStatus::Pass);
    write(&defaults, b"chunks: [unclosed\n");
    assert!(matches!(
        t1(r),
        CheckStatus::Unknown(UnknownReason::UnreadableFile { .. })
    ));
    write(&defaults, b"chunks:\n  fixed-chunk-inhabited-time: maybe\n");
    write(&world, b"");
    assert!(matches!(
        t1(r),
        CheckStatus::Unknown(UnknownReason::UnexpectedValue { .. })
    ));
}

#[test]
fn t1_wrong_node_types_are_unexpected() {
    let t = bukkit();
    let r = t.path();
    let world = r.join("world/paper-world.yml");
    let t1 = |r: &std::path::Path| status(&report(r, PlanOptions::default()), GuardId::T1).clone();
    for text in [
        &b"chunks: 0\n"[..],
        b"- a list\n",
        b"chunks:\n  fixed-chunk-inhabited-time: [1]\n",
    ] {
        write(&world, text);
        assert!(
            matches!(
                t1(r),
                CheckStatus::Unknown(UnknownReason::UnexpectedValue { .. })
            ),
            "{}",
            String::from_utf8_lossy(text)
        );
    }
    write(&world, b"chunks:\n");
    assert_eq!(t1(r), CheckStatus::Pass);
}

#[test]
fn t1_legacy_paper_yml() {
    let t = bukkit();
    let r = t.path();
    let t1 = |r: &std::path::Path| status(&report(r, PlanOptions::default()), GuardId::T1).clone();
    write(
        &r.join("paper.yml"),
        b"world-settings:\n  default:\n    fixed-chunk-inhabited-time: 0\n",
    );
    assert!(
        matches!(t1(r), CheckStatus::Fail(d) if matches!(&d[..], [TrustDetail::PaperFixedInhabitedTime { value: 0, .. }]))
    );
    // Per-world values override the default.
    write(
        &r.join("paper.yml"),
        b"world-settings:\n  default:\n    fixed-chunk-inhabited-time: 0\n  world:\n    fixed-chunk-inhabited-time: -1\n  world_nether:\n    fixed-chunk-inhabited-time: -1\n",
    );
    assert_eq!(t1(r), CheckStatus::Pass);
    // Once config/paper-world-defaults.yml exists, paper.yml is ignored.
    write(
        &r.join("paper.yml"),
        b"world-settings:\n  default:\n    fixed-chunk-inhabited-time: 0\n",
    );
    write(
        &r.join("config/paper-world-defaults.yml"),
        b"chunks:\n  fixed-chunk-inhabited-time: -1\n",
    );
    assert_eq!(t1(r), CheckStatus::Pass);
}

#[test]
fn t1_and_t2_do_not_apply_to_vanilla() {
    let t = vanilla();
    player(&t.path().join("playerdata"), UUID_A, V::Int(0), 20.0, 0);
    let r = report(t.path(), PlanOptions::default());
    assert_eq!(status(&r, GuardId::T1), &CheckStatus::NotApplicable);
    assert_eq!(status(&r, GuardId::T2), &CheckStatus::NotApplicable);
}

#[test]
fn t2_mob_spawning_disabled_on_paper() {
    let t = bukkit();
    let r = t.path();
    level(
        &r.join("world_nether"),
        vec![
            ("DataVersion", V::Int(4325)),
            (
                "GameRules",
                V::Compound(vec![("doMobSpawning", V::Str("false"))]),
            ),
        ],
    );
    let report = report(r, PlanOptions::default());
    assert!(matches!(
        status(&report, GuardId::T2),
        CheckStatus::Fail(d) if matches!(&d[..], [TrustDetail::MobSpawningDisabled { world_folder }] if world_folder.ends_with("world_nether"))
    ));
    // Not part of the GC: not checked.
    let only_ow = PlanOptions {
        gc_dims: Some(BTreeSet::from([DimensionId::overworld()])),
    };
    assert_eq!(
        status(&super::tests::report(r, only_ow), GuardId::T2),
        &CheckStatus::Pass
    );
}

#[test]
fn t3_player_positions() {
    let t = vanilla();
    let dir = t.path().join("playerdata");
    // In chunk (1,0) with 100 ticks.
    player(&dir, UUID_A, V::Int(0), 20.0, 0);
    assert_eq!(
        status(&report(t.path(), PlanOptions::default()), GuardId::T3),
        &CheckStatus::Pass
    );

    // A spectator in the zero chunk is skipped; a survival player there fails.
    player(&dir, UUID_B, V::Str("minecraft:overworld"), 3.0, 3);
    assert_eq!(
        status(&report(t.path(), PlanOptions::default()), GuardId::T3),
        &CheckStatus::Pass
    );
    player(&dir, UUID_B, V::Str("minecraft:overworld"), 3.0, 0);
    assert_eq!(
        status(&report(t.path(), PlanOptions::default()), GuardId::T3),
        &CheckStatus::Fail(vec![TrustDetail::PlayerInZeroChunk {
            uuid: UUID_B.into(),
            dimension: DimensionId::overworld(),
            pos: ChunkPos::new(0, 0)
        }])
    );
}

#[test]
fn t3_unreadable_player_file_is_unknown() {
    let t = vanilla();
    let dir = t.path().join("playerdata");
    player(&dir, UUID_A, V::Int(0), 20.0, 0);
    write(&dir.join(format!("{UUID_B}.dat")), b"truncated");
    assert!(matches!(
        status(&report(t.path(), PlanOptions::default()), GuardId::T3),
        CheckStatus::Unknown(UnknownReason::UnreadableFile { .. })
    ));
}

#[test]
fn t3_host_with_a_player_file_is_counted_once() {
    let t = vanilla();
    player(&t.path().join("playerdata"), UUID_A, V::Int(0), 3.0, 0);
    let host = V::Compound(vec![
        ("Pos", V::DoubleList(vec![3.0, 64.0, 1.0])),
        ("Dimension", V::Int(0)),
        ("playerGameType", V::Int(0)),
    ]);
    level(
        t.path(),
        vec![("DataVersion", V::Int(4325)), ("Player", host)],
    );
    assert!(matches!(
        status(&report(t.path(), PlanOptions::default()), GuardId::T3),
        CheckStatus::Fail(d) if d.len() == 1
    ));
}

#[test]
fn t3_unknown_without_checkable_players() {
    let t = vanilla();
    let dir = t.path().join("playerdata");
    // A player in a chunk that does not exist, and one in another dimension.
    player(&dir, UUID_A, V::Int(0), 5000.0, 0);
    player(&dir, UUID_B, V::Int(-1), 3.0, 0);
    assert_eq!(
        status(&report(t.path(), PlanOptions::default()), GuardId::T3),
        &CheckStatus::Unknown(UnknownReason::NoPlayersChecked)
    );
}

#[test]
fn t3_reads_the_singleplayer_host() {
    let t = vanilla();
    let host = V::Compound(vec![
        ("Pos", V::DoubleList(vec![-0.5, 70.0, 2.0])), // chunk (-1, 0): does not exist
        ("Dimension", V::Str("minecraft:overworld")),
    ]);
    level(
        t.path(),
        vec![("DataVersion", V::Int(4325)), ("Player", host)],
    );
    let r = report(t.path(), PlanOptions::default());
    assert_eq!(
        status(&r, GuardId::T3),
        &CheckStatus::Unknown(UnknownReason::NoPlayersChecked)
    );

    let host = V::Compound(vec![
        ("Pos", V::DoubleList(vec![1.5, 70.0, 2.0])), // chunk (0, 0): zero ticks
        ("Dimension", V::Str("minecraft:overworld")),
    ]);
    level(
        t.path(),
        vec![("DataVersion", V::Int(4325)), ("Player", host)],
    );
    let r = report(t.path(), PlanOptions::default());
    assert!(
        matches!(status(&r, GuardId::T3), CheckStatus::Fail(d) if matches!(&d[0], TrustDetail::PlayerInZeroChunk { uuid, .. } if uuid == "level.dat"))
    );
}

#[test]
fn t4_and_t5_and_stops() {
    let t = vanilla();
    player(&t.path().join("playerdata"), UUID_A, V::Int(0), 20.0, 0);
    write(&t.path().join("data/chunks.dat"), b"broken");
    let r = report(t.path(), PlanOptions::default());
    assert_eq!(status(&r, GuardId::T4), &CheckStatus::Pass);
    assert!(matches!(
        status(&r, GuardId::T5),
        CheckStatus::Unknown(UnknownReason::ForcedChunksUnknown(_))
    ));
    assert_eq!(r.stops(), BTreeSet::from([GuardId::T5]));

    // Only the end is targeted: the overworld's problems do not count.
    let end = PlanOptions {
        gc_dims: Some(BTreeSet::from([DimensionId::the_end()])),
    };
    let r = report(t.path(), end);
    assert_eq!(status(&r, GuardId::T5), &CheckStatus::Pass);
    assert_eq!(
        status(&r, GuardId::T3),
        &CheckStatus::Unknown(UnknownReason::NoPlayersChecked)
    );

    level(t.path(), vec![("DataVersion", V::Int(DV_MAX_KNOWN.0 + 1))]);
    let r = report(t.path(), PlanOptions::default());
    assert!(matches!(
        status(&r, GuardId::T4),
        CheckStatus::Unknown(UnknownReason::UnknownVersion(_))
    ));
}

#[test]
fn checks_before_the_plan_match_the_checks_against_the_plan() {
    let t = vanilla();
    write(&t.path().join("data/chunks.dat"), b"broken");
    let players = t.path().join("playerdata");
    // In chunk 0 (InhabitedTime 0), chunk 1 (> 0), a missing chunk, and the end.
    player(&players, UUID_A, V::Int(0), 8.0, 0);
    player(&players, UUID_B, V::Int(0), 24.0, 0);
    player(&players, UUID_C, V::Int(0), 5000.0, 0);
    player(&players, UUID_D, V::Int(1), 8.0, 0);
    let layout = crate::layout::detect(t.path()).unwrap();

    let early = check_trust_before_plan(&layout, None, false);
    assert_eq!(early, report(t.path(), PlanOptions::default()));
    assert!(
        matches!(status(&early, GuardId::T3), CheckStatus::Fail(d) if matches!(&d[..], [TrustDetail::PlayerInZeroChunk { uuid, .. }] if uuid == UUID_A))
    );
    assert_eq!(early.stops(), BTreeSet::from([GuardId::T3, GuardId::T5]));

    let end = PlanOptions {
        gc_dims: Some(BTreeSet::from([DimensionId::the_end()])),
    };
    assert_eq!(
        check_trust_before_plan(&layout, end.gc_dims.as_ref(), false),
        report(t.path(), end)
    );
}

#[test]
fn t3_can_be_skipped_before_the_plan() {
    let t = vanilla();
    player(&t.path().join("playerdata"), UUID_A, V::Int(0), 8.0, 0);
    let layout = crate::layout::detect(t.path()).unwrap();
    let skipped = check_trust_before_plan(&layout, None, true);
    let without_t3: Vec<_> = report(t.path(), PlanOptions::default())
        .checks
        .into_iter()
        .filter(|c| c.id != GuardId::T3)
        .collect();
    assert_eq!(skipped.checks, without_t3);
    assert!(skipped.stops().is_empty());
}

#[test]
fn t3_before_the_plan_reads_external_chunks() {
    let t = vanilla();
    let nbt = doc(vec![
        ("DataVersion", V::Int(4325)),
        ("InhabitedTime", V::Long(0)),
    ]);
    write(
        &t.path().join("region/r.0.0.mca"),
        &build(&[chunk(0, 2 + crate::region::EXTERNAL_FLAG, b"")]),
    );
    write(&t.path().join("region/c.0.0.mcc"), &zlib(&nbt));
    player(&t.path().join("playerdata"), UUID_A, V::Int(0), 8.0, 0);
    let layout = crate::layout::detect(t.path()).unwrap();
    let early = check_trust_before_plan(&layout, None, false);
    assert_eq!(early, report(t.path(), PlanOptions::default()));
    assert!(matches!(status(&early, GuardId::T3), CheckStatus::Fail(_)));
}

#[test]
fn block_to_chunk_floors() {
    assert_eq!(block_to_chunk(-0.5, 15.9), Some(ChunkPos::new(-1, 0)));
    assert_eq!(block_to_chunk(16.0, -16.0), Some(ChunkPos::new(1, -1)));
    assert_eq!(block_to_chunk(f64::NAN, 0.0), None);
}
