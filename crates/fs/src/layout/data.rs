//! Reading `level.dat` while recognizing a layout.

use std::fs;
use std::io;
use std::path::Path;

use super::LayoutError;
use crate::nbt::{self, LevelInfo};

pub(super) fn read_level_dat(world_folder: &Path) -> Result<LevelInfo, LayoutError> {
    let path = world_folder.join("level.dat");
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(LayoutError::MissingLevelDat { path });
        }
        Err(e) => return Err(LayoutError::io(&path, e)),
    };
    nbt::gunzip(&bytes)
        .and_then(|nbt| nbt::read_level_info(&nbt))
        .map_err(|source| LayoutError::UnreadableLevelDat { path, source })
}

#[cfg(test)]
mod tests {
    use super::super::detect;
    use super::super::test_util::*;
    use std::collections::BTreeSet;

    use super::*;
    use crate::layout::{Dimension, WorldLayout};
    use crate::nbt::test_util::{V, doc, pack};
    use crate::source::FsSource;
    use crate::types::{ChunkPos, Known};
    use crate::types::{DimensionId, UnknownReason};
    use crate::versions::DV_MAX_KNOWN;
    use wgc_core::layout::{
        forced_chunks, paper_defaults_config, paper_world_config, player_data_dir, spawn_mobs,
    };

    fn level(world: &Path, entries: Vec<(&'static str, V)>) {
        write(
            &world.join("level.dat"),
            &gzip(&doc(vec![("Data", V::Compound(entries))])),
        );
    }

    fn dim<'a>(layout: &'a WorldLayout, id: &str) -> &'a Dimension {
        let id = DimensionId::parse(id).unwrap();
        layout.dimensions.iter().find(|d| d.id == id).unwrap()
    }

    #[test]
    fn spawn_mobs_in_each_format() {
        // Before 4658: level.dat GameRules string.
        let t = tempfile::tempdir().unwrap();
        region(t.path(), "r.0.0.mca");
        level(
            t.path(),
            vec![
                ("DataVersion", V::Int(4325)),
                (
                    "GameRules",
                    V::Compound(vec![("doMobSpawning", V::Str("false"))]),
                ),
            ],
        );
        let l = detect(t.path()).unwrap();
        assert_eq!(
            spawn_mobs(&FsSource, &l, dim(&l, "overworld")),
            Known::Value(false)
        );

        // 4658..4771: level.dat game_rules byte; missing key means enabled.
        level(
            t.path(),
            vec![
                ("DataVersion", V::Int(4671)),
                ("game_rules", V::Compound(vec![])),
            ],
        );
        let l = detect(t.path()).unwrap();
        assert_eq!(
            spawn_mobs(&FsSource, &l, dim(&l, "overworld")),
            Known::Value(true)
        );

        // 4772 and later: data/minecraft/game_rules.dat.
        level(t.path(), vec![("DataVersion", V::Int(4786))]);
        let rules = doc(vec![(
            "data",
            V::Compound(vec![("minecraft:spawn_mobs", V::Byte(0))]),
        )]);
        write(
            &t.path().join("data/minecraft/game_rules.dat"),
            &gzip(&rules),
        );
        let l = detect(t.path()).unwrap();
        assert_eq!(
            spawn_mobs(&FsSource, &l, dim(&l, "overworld")),
            Known::Value(false)
        );

        // Unknown version.
        level(t.path(), vec![("DataVersion", V::Int(DV_MAX_KNOWN.0 + 1))]);
        let l = detect(t.path()).unwrap();
        assert!(matches!(
            spawn_mobs(&FsSource, &l, dim(&l, "overworld")),
            Known::Unknown(UnknownReason::UnknownVersion(_))
        ));
    }

    #[test]
    fn spawn_mobs_per_world_on_bukkit_legacy() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        region(&r.join("world"), "r.0.0.mca");
        region(&r.join("world_nether/DIM-1"), "r.0.0.mca");
        level(&r.join("world"), vec![("DataVersion", V::Int(4325))]);
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
        let l = detect(r).unwrap();
        assert_eq!(
            spawn_mobs(&FsSource, &l, dim(&l, "overworld")),
            Known::Value(true)
        );
        assert_eq!(
            spawn_mobs(&FsSource, &l, dim(&l, "the_nether")),
            Known::Value(false)
        );
    }

    #[test]
    fn forced_chunk_files() {
        let t = tempfile::tempdir().unwrap();
        let w = t.path();
        region(w, "r.0.0.mca");
        level(w, vec![("DataVersion", V::Int(4189))]);
        let l = detect(w).unwrap();
        let ow = dim(&l, "overworld").clone();
        assert_eq!(
            forced_chunks(&FsSource, &l, &ow),
            Known::Value(BTreeSet::new())
        );

        let forced = doc(vec![(
            "data",
            V::Compound(vec![("Forced", V::LongArray(vec![pack(1, 2)]))]),
        )]);
        write(&w.join("data/chunks.dat"), &gzip(&forced));
        assert_eq!(
            forced_chunks(&FsSource, &l, &ow),
            Known::Value(BTreeSet::from([ChunkPos::new(1, 2)]))
        );

        write(&w.join("data/chunks.dat"), b"broken");
        assert!(matches!(
            forced_chunks(&FsSource, &l, &ow),
            Known::Unknown(UnknownReason::UnreadableFile { .. })
        ));
    }

    #[test]
    fn paths_by_layout() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        write(&r.join("bukkit.yml"), b"");
        region(&r.join("world/dimensions/minecraft/overworld"), "r.0.0.mca");
        level(&r.join("world"), vec![("DataVersion", V::Int(5023))]);
        let l = detect(r).unwrap();
        let ow = dim(&l, "overworld");
        assert_eq!(
            player_data_dir(&FsSource, &l),
            l.root.join("world/players/data")
        );
        assert_eq!(
            paper_world_config(&FsSource, &l, ow),
            Some(ow.dir.join("paper-world.yml"))
        );
        assert_eq!(
            paper_defaults_config(&FsSource, &l),
            Some(l.root.join("config/paper-world-defaults.yml"))
        );
    }
}
