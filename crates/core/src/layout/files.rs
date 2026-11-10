//! Enumerating region triples (docs/spec/world-layout.md "リージョン三つ組の列挙").

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::{Dimension, LayoutError, read_dir_opt};
use crate::region::{parse_external_name, parse_region_name};
use crate::types::{ChunkPos, FileKind, RegionPos};

/// The files of one region triple. Each kind may be missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TripleFiles {
    pub pos: RegionPos,
    pub region: Option<PathBuf>,
    pub entities: Option<PathBuf>,
    pub poi: Option<PathBuf>,
}

impl TripleFiles {
    pub fn get(&self, kind: FileKind) -> Option<&Path> {
        match kind {
            FileKind::Region => self.region.as_deref(),
            FileKind::Entities => self.entities.as_deref(),
            FileKind::Poi => self.poi.as_deref(),
        }
    }

    fn slot(&mut self, kind: FileKind) -> &mut Option<PathBuf> {
        match kind {
            FileKind::Region => &mut self.region,
            FileKind::Entities => &mut self.entities,
            FileKind::Poi => &mut self.poi,
        }
    }
}

/// The region triples and external chunk files of one dimension.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DimensionFiles {
    /// Sorted by position.
    pub triples: Vec<TripleFiles>,
    pub region_externals: BTreeMap<ChunkPos, PathBuf>,
    pub entities_externals: BTreeMap<ChunkPos, PathBuf>,
    pub poi_externals: BTreeMap<ChunkPos, PathBuf>,
}

impl DimensionFiles {
    /// External `.mcc` files of `kind`.
    pub fn externals(&self, kind: FileKind) -> &BTreeMap<ChunkPos, PathBuf> {
        match kind {
            FileKind::Region => &self.region_externals,
            FileKind::Entities => &self.entities_externals,
            FileKind::Poi => &self.poi_externals,
        }
    }

    /// Chunks of `region` that have an external file of `kind`.
    pub fn externals_in(&self, kind: FileKind, region: RegionPos) -> BTreeSet<ChunkPos> {
        self.externals(kind)
            .keys()
            .copied()
            .filter(|pos| pos.region() == region)
            .collect()
    }

    fn externals_mut(&mut self, kind: FileKind) -> &mut BTreeMap<ChunkPos, PathBuf> {
        match kind {
            FileKind::Region => &mut self.region_externals,
            FileKind::Entities => &mut self.entities_externals,
            FileKind::Poi => &mut self.poi_externals,
        }
    }
}

/// Lists the region triples of `dim`.
///
/// Two names for the same coordinate in one folder (`r.0.0.mca` and
/// `r.00.00.mca`) are an error: which one the game reads cannot be decided.
pub fn list_dimension_files(dim: &Dimension) -> Result<DimensionFiles, LayoutError> {
    let mut triples: BTreeMap<RegionPos, TripleFiles> = BTreeMap::new();
    let mut files = DimensionFiles::default();
    for kind in FileKind::ALL {
        let dir = dim.dir.join(kind.dir_name());
        let mut entries = read_dir_opt(&dir)?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let path = entry.path();
            if let Some(pos) = parse_region_name(&name) {
                if !path.is_file() {
                    continue;
                }
                let triple = triples.entry(pos).or_insert_with(|| TripleFiles {
                    pos,
                    region: None,
                    entities: None,
                    poi: None,
                });
                insert_unique(triple.slot(kind), path)?;
            } else if let Some(pos) = parse_external_name(&name) {
                if !path.is_file() {
                    continue;
                }
                let map = files.externals_mut(kind);
                if let Some(first) = map.get(&pos) {
                    return Err(LayoutError::AmbiguousRegion {
                        first: first.clone(),
                        second: path,
                    });
                }
                map.insert(pos, path);
            }
        }
    }
    files.triples = triples.into_values().collect();
    Ok(files)
}

fn insert_unique(slot: &mut Option<PathBuf>, path: PathBuf) -> Result<(), LayoutError> {
    if let Some(first) = slot {
        return Err(LayoutError::AmbiguousRegion {
            first: first.clone(),
            second: path,
        });
    }
    *slot = Some(path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::test_util::*;
    use super::*;
    use crate::types::DimensionId;

    fn dimension(dir: &Path) -> Dimension {
        Dimension {
            id: DimensionId::overworld(),
            dir: dir.to_path_buf(),
            world_folder: dir.to_path_buf(),
        }
    }

    #[test]
    fn triples_are_the_union_of_kinds() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        write(&d.join("region/r.0.0.mca"), b"");
        write(&d.join("region/r.-1.0.mca"), &[0; 8192]);
        write(&d.join("entities/r.0.0.mca"), &[0; 8192]);
        write(&d.join("poi/r.5.5.mca"), &[0; 8192]);
        write(&d.join("region/c.-1.3.mcc"), b"x");
        write(&d.join("region/r.0.0.mca.bak"), b"");
        std::fs::create_dir_all(d.join("region/r.9.9.mca")).unwrap(); // a directory, not a file

        let files = list_dimension_files(&dimension(d)).unwrap();
        let positions: Vec<RegionPos> = files.triples.iter().map(|t| t.pos).collect();
        assert_eq!(
            positions,
            [
                RegionPos::new(-1, 0),
                RegionPos::new(0, 0),
                RegionPos::new(5, 5)
            ]
        );
        let origin = &files.triples[1];
        assert!(origin.region.is_some() && origin.entities.is_some() && origin.poi.is_none());
        assert_eq!(
            files.externals_in(FileKind::Region, RegionPos::new(-1, 0)),
            BTreeSet::from([ChunkPos::new(-1, 3)])
        );
        assert!(
            files
                .externals_in(FileKind::Region, RegionPos::new(0, 0))
                .is_empty()
        );
    }

    #[test]
    fn two_names_for_one_region_are_an_error() {
        let t = tempfile::tempdir().unwrap();
        write(&t.path().join("region/r.0.0.mca"), b"");
        write(&t.path().join("region/r.00.00.mca"), b"");
        assert!(matches!(
            list_dimension_files(&dimension(t.path())),
            Err(LayoutError::AmbiguousRegion { .. })
        ));
    }

    #[test]
    fn missing_folders_are_empty() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(
            list_dimension_files(&dimension(t.path())).unwrap(),
            DimensionFiles::default()
        );
    }
}
