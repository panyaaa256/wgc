use std::io::Write;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use super::*;
use crate::layout::test_util::{gzip, level_dat, write};
use crate::nbt::test_util::{V, doc, pack};
use crate::region::test_util::{Chunk, build, chunk};
use crate::types::NoopObserver;

fn zlib(bytes: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(bytes).unwrap();
    e.finish().unwrap()
}

/// Chunk NBT with an InhabitedTime (or none) and structure references.
fn nbt(inhabited: Option<V>, refs: &[(i32, i32)], blending: bool) -> Vec<u8> {
    let mut root = vec![("DataVersion", V::Int(4325))];
    if let Some(v) = inhabited {
        root.push(("InhabitedTime", v));
    }
    if blending {
        root.push(("blending_data", V::Compound(vec![])));
    }
    root.push((
        "structures",
        V::Compound(vec![(
            "References",
            V::Compound(vec![(
                "minecraft:village",
                V::LongArray(refs.iter().map(|&(x, z)| pack(x, z)).collect()),
            )]),
        )]),
    ));
    doc(root)
}

fn c(index: u16, inhabited: i64) -> Chunk {
    chunk(index, 2, &zlib(&nbt(Some(V::Long(inhabited)), &[], false)))
}

fn idx(i: u16) -> ChunkIndex {
    ChunkIndex::new(i).unwrap()
}

/// A vanilla world with one region file per entry of `regions`.
fn world(regions: &[(&str, Vec<u8>)]) -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    level_dat(t.path(), 4325);
    for (rel, bytes) in regions {
        write(&t.path().join(rel), bytes);
    }
    t
}

fn plan(path: &Path) -> Plan {
    plan_with(path, PlanOptions::default())
}

fn plan_with(path: &Path, options: PlanOptions) -> Plan {
    let layout = crate::layout::detect(path).unwrap();
    build_plan(&layout, &options, &NoopObserver, &AtomicBool::new(false)).unwrap()
}

fn decisions(plan: &Plan) -> BTreeMap<ChunkPos, Decision> {
    plan.dimensions
        .iter()
        .flat_map(|d| &d.triples)
        .flat_map(|t| &t.chunks)
        .map(|c| (c.pos, c.decision))
        .collect()
}

fn overworld(plan: &Plan) -> &DimensionPlan {
    &plan.dimensions[0]
}

#[test]
fn only_zero_inhabited_time_is_deleted() {
    let chunks = vec![
        c(0, 0),
        c(1, 5),
        chunk(2, 2, &zlib(&nbt(None, &[], false))),
        chunk(3, 2, &zlib(&nbt(Some(V::Int(0)), &[], false))),
        c(4, -1),
        chunk(5, 2, b"not zlib"),
    ];
    let t = world(&[("region/r.0.0.mca", build(&chunks))]);
    let p = plan(t.path());
    let d = decisions(&p);
    assert_eq!(d[&ChunkPos::new(0, 0)], Decision::Delete);
    assert_eq!(
        d[&ChunkPos::new(1, 0)],
        Decision::Keep(KeepReason::Inhabited)
    );
    assert_eq!(
        d[&ChunkPos::new(2, 0)],
        Decision::Keep(KeepReason::InhabitedUnknown)
    );
    assert_eq!(
        d[&ChunkPos::new(3, 0)],
        Decision::Keep(KeepReason::InhabitedUnknown)
    );
    assert_eq!(
        d[&ChunkPos::new(4, 0)],
        Decision::Keep(KeepReason::InhabitedUnknown)
    );
    assert_eq!(
        d[&ChunkPos::new(5, 0)],
        Decision::Keep(KeepReason::ContentError)
    );
    let triple = &overworld(&p).triples[0];
    assert_eq!(
        triple.action(FileKind::Region),
        Some(FileAction::Rewrite {
            keep: vec![idx(1), idx(2), idx(3), idx(4), idx(5)]
        })
    );
}

#[test]
fn forceload_protects_two_rings() {
    let chunks: Vec<Chunk> = (0..8).map(|x| c(x, 0)).collect();
    let t = world(&[("region/r.0.0.mca", build(&chunks))]);
    let forced = doc(vec![(
        "data",
        V::Compound(vec![("Forced", V::LongArray(vec![pack(2, 0)]))]),
    )]);
    write(&t.path().join("data/chunks.dat"), &gzip(&forced));
    let d = decisions(&plan(t.path()));
    for x in 0..=4 {
        assert_eq!(
            d[&ChunkPos::new(x, 0)],
            Decision::Keep(KeepReason::Forceload),
            "x={x}"
        );
    }
    assert_eq!(d[&ChunkPos::new(5, 0)], Decision::Delete);
}

#[test]
fn chunks_kept_by_forceload_protect_their_structure_starts() {
    // Chunk (1,0) is kept by forceload around (0,0) and references (9,0).
    let referrer = chunk(1, 2, &zlib(&nbt(Some(V::Long(0)), &[(9, 0)], false)));
    let t = world(&[("region/r.0.0.mca", build(&[c(0, 0), referrer, c(9, 0)]))]);
    let forced = doc(vec![(
        "data",
        V::Compound(vec![("Forced", V::LongArray(vec![pack(0, 0)]))]),
    )]);
    write(&t.path().join("data/chunks.dat"), &gzip(&forced));
    let d = decisions(&plan(t.path()));
    assert_eq!(
        d[&ChunkPos::new(1, 0)],
        Decision::Keep(KeepReason::Forceload)
    );
    assert_eq!(
        d[&ChunkPos::new(9, 0)],
        Decision::Keep(KeepReason::StructureRef)
    );
}

#[test]
fn orphan_mcc_and_nested_session_lock_count_as_other_files() {
    let t = world(&[
        ("region/r.0.0.mca", build(&[c(0, 5)])),
        ("entities/c.5.5.mcc", vec![0; 50]),
        ("backups/session.lock", vec![0; 7]),
    ]);
    std::fs::write(t.path().join("session.lock"), "x").unwrap();
    let p = plan(t.path());
    let level_dat = std::fs::metadata(t.path().join("level.dat")).unwrap().len();
    assert_eq!(p.estimate.other_files_bytes, level_dat + 50 + 7);
}

#[test]
fn unreadable_forced_chunks_are_recorded() {
    let t = world(&[("region/r.0.0.mca", build(&[c(0, 0)]))]);
    write(&t.path().join("data/chunks.dat"), b"broken");
    let p = plan(t.path());
    assert_eq!(
        p.protection_unknown,
        BTreeSet::from([DimensionId::overworld()])
    );
}

#[test]
fn structure_references_protect_across_regions() {
    // Region (1,0) chunk 32 is inhabited and references chunk (3,0) in region (0,0).
    // Chunk (4,0) is only referenced by a zero chunk.
    let inhabited = chunk(0, 2, &zlib(&nbt(Some(V::Long(9)), &[(3, 0)], false)));
    let zero_ref = chunk(5, 2, &zlib(&nbt(Some(V::Long(0)), &[(4, 0)], false)));
    let t = world(&[
        ("region/r.0.0.mca", build(&[c(3, 0), c(4, 0), zero_ref])),
        ("region/r.1.0.mca", build(&[inhabited])),
    ]);
    let d = decisions(&plan(t.path()));
    assert_eq!(
        d[&ChunkPos::new(3, 0)],
        Decision::Keep(KeepReason::StructureRef)
    );
    assert_eq!(d[&ChunkPos::new(4, 0)], Decision::Delete);
}

#[test]
fn structural_issue_keeps_the_whole_triple() {
    let mut bad_poi = build(&[c(0, 0)]);
    bad_poi.truncate(8192 + 2); // S-4
    let t = world(&[
        ("region/r.0.0.mca", build(&[c(0, 0), c(1, 0)])),
        ("poi/r.0.0.mca", bad_poi),
    ]);
    let p = plan(t.path());
    let triple = &overworld(&p).triples[0];
    assert!(triple.is_verbatim());
    assert!(
        triple
            .chunks
            .iter()
            .all(|c| c.decision == Decision::Keep(KeepReason::TripleVerbatim))
    );
    assert_eq!(triple.action(FileKind::Region), Some(FileAction::Verbatim));
    assert_eq!(triple.action(FileKind::Poi), Some(FileAction::Verbatim));
}

#[test]
fn orphans_are_removed_only_when_region_is_readable() {
    let entities = build(&[chunk(0, 2, b"e0"), chunk(7, 2, b"e7"), chunk(9, 2, b"e9")]);
    let t = world(&[
        ("region/r.0.0.mca", build(&[c(0, 5), c(9, 0)])),
        ("entities/r.0.0.mca", entities.clone()),
        // Region (1,0) has no region file at all: all entity data is orphaned.
        ("entities/r.1.0.mca", entities.clone()),
        // Region (2,0)'s region file is unreadable (S-1): nothing is removed.
        ("region/r.2.0.mca", vec![1, 2, 3]),
        ("entities/r.2.0.mca", entities),
    ]);
    let p = plan(t.path());
    let triples = &overworld(&p).triples;
    let t0 = &triples[0];
    assert_eq!(t0.entry_fate(FileKind::Entities, idx(0)), EntryFate::Keep);
    assert_eq!(
        t0.entry_fate(FileKind::Entities, idx(7)),
        EntryFate::Remove(RemoveReason::Orphan)
    );
    assert_eq!(
        t0.entry_fate(FileKind::Entities, idx(9)),
        EntryFate::Remove(RemoveReason::GcDeleted)
    );
    assert_eq!(
        t0.action(FileKind::Entities),
        Some(FileAction::Rewrite { keep: vec![idx(0)] })
    );
    assert_eq!(
        triples[1].action(FileKind::Entities),
        Some(FileAction::Omit(OmitReason::NoEntriesLeft))
    );
    assert_eq!(
        triples[2].action(FileKind::Entities),
        Some(FileAction::Verbatim)
    );
}

#[test]
fn empty_files_and_player_temp_files_are_omitted() {
    let uuid = "0123abcd-0000-0000-0000-00000000beef";
    let t = world(&[
        ("region/r.0.0.mca", build(&[c(0, 5)])),
        ("poi/r.0.0.mca", Vec::new()),
        (&format!("playerdata/{uuid}.dat"), b"p".to_vec()),
        (&format!("playerdata/{uuid}-123.dat"), Vec::new()),
        (
            "playerdata/ffffffff-0000-0000-0000-000000000000.dat_old",
            b"o".to_vec(),
        ),
    ]);
    let p = plan(t.path());
    assert_eq!(
        overworld(&p).triples[0].action(FileKind::Poi),
        Some(FileAction::Omit(OmitReason::EmptyFile))
    );
    let omitted: Vec<_> = p
        .omitted_other_files
        .iter()
        .map(|o| o.path.file_name().unwrap().to_owned())
        .collect();
    assert_eq!(
        omitted,
        [std::ffi::OsString::from(format!("{uuid}-123.dat"))]
    );
    assert_eq!(p.dat_old_without_dat.len(), 1);
}

#[test]
fn player_temp_file_pattern() {
    assert!(is_player_temp_file(
        "0123abcd-0000-0000-0000-00000000beef-1.dat"
    ));
    assert!(!is_player_temp_file(
        "0123abcd-0000-0000-0000-00000000beef.dat"
    ));
    assert!(!is_player_temp_file(
        "0123ABCD-0000-0000-0000-00000000beef-1.dat"
    ));
    assert!(!is_player_temp_file(
        "0123abcd-0000-0000-0000-00000000beef-.dat"
    ));
    assert!(!is_player_temp_file(
        "0123abcd-0000-0000-0000-00000000beef-1.dat_old"
    ));
}

#[test]
fn untargeted_dimensions_only_get_clean_rules() {
    let t = world(&[
        ("region/r.0.0.mca", build(&[c(0, 0)])),
        ("entities/r.0.0.mca", build(&[chunk(5, 2, b"orphan")])),
    ]);
    let options = PlanOptions {
        gc_dims: Some(BTreeSet::from([DimensionId::the_end()])),
    };
    let p = plan_with(t.path(), options);
    let triple = &overworld(&p).triples[0];
    assert_eq!(
        triple.chunks[0].decision,
        Decision::Keep(KeepReason::NotTargeted)
    );
    assert_eq!(triple.action(FileKind::Region), Some(FileAction::Verbatim));
    assert_eq!(
        triple.action(FileKind::Entities),
        Some(FileAction::Omit(OmitReason::NoEntriesLeft))
    );
}

#[test]
fn external_files_follow_their_chunk() {
    let region = build(&[chunk(1, 2 + 128, b""), c(2, 5)]);
    // Chunk 1's data lives in c.1.0.mcc and has InhabitedTime 0.
    let mcc = zlib(&nbt(Some(V::Long(0)), &[], false));
    let t = world(&[
        ("region/r.0.0.mca", region),
        ("region/c.1.0.mcc", mcc.clone()),
    ]);
    let p = plan(t.path());
    let triple = &overworld(&p).triples[0];
    assert_eq!(triple.chunks[0].decision, Decision::Delete);
    assert_eq!(
        triple.external_action(FileKind::Region, ChunkPos::new(1, 0)),
        ExternalAction::Omit
    );
    let sizes = triple.sizes(FileKind::Region);
    assert_eq!(sizes.after, 8192 + 4096);
    assert!(sizes.before > sizes.after);
}

#[test]
fn boundary_with_old_terrain() {
    let blended = |i| chunk(i, 2, &zlib(&nbt(Some(V::Long(7)), &[], true)));
    // Row z=0: 0 deleted, 1 and 2 blended kept; chunk 40 (8,1) blended but not adjacent.
    let t = world(&[(
        "region/r.0.0.mca",
        build(&[c(0, 0), blended(1), blended(2), blended(40)]),
    )]);
    let p = plan(t.path());
    assert_eq!(
        overworld(&p).boundary_warnings,
        [BoundaryCluster {
            chunks: 1,
            min: ChunkPos::new(1, 0),
            max: ChunkPos::new(1, 0)
        }]
    );
}

#[test]
fn estimate_counts_other_files() {
    let t = world(&[
        ("region/r.0.0.mca", build(&[c(0, 5)])),
        ("data/raids.dat", vec![0; 100]),
    ]);
    std::fs::write(t.path().join("session.lock"), "x").unwrap();
    let p = plan(t.path());
    let level_dat = std::fs::metadata(t.path().join("level.dat")).unwrap().len();
    assert_eq!(p.estimate.other_files_bytes, 100 + level_dat);
    assert_eq!(p.estimate.total_output_bytes, 100 + level_dat + 8192 + 4096);
}

#[test]
fn plans_are_deterministic() {
    let regions: Vec<(String, Vec<u8>)> = (0..6)
        .map(|i| {
            (
                format!("region/r.{i}.0.mca"),
                build(&[c(0, 0), c(1, i64::from(i)), c(2, 0)]),
            )
        })
        .collect();
    let refs: Vec<(&str, Vec<u8>)> = regions
        .iter()
        .map(|(p, b)| (p.as_str(), b.clone()))
        .collect();
    let t = world(&refs);
    assert_eq!(plan(t.path()), plan(t.path()));
}

#[test]
fn cancellation() {
    let t = world(&[("region/r.0.0.mca", build(&[c(0, 0)]))]);
    let layout = crate::layout::detect(t.path()).unwrap();
    let result = build_plan(
        &layout,
        &PlanOptions::default(),
        &NoopObserver,
        &AtomicBool::new(true),
    );
    assert!(matches!(result, Err(PlanError::Cancelled)));
}
