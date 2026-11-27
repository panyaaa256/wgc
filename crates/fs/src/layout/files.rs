//! Enumerating region triples.

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::{Dimension, LayoutError, read_dir_opt};
use crate::region::{parse_external_name, parse_region_name};
use crate::types::{FileKind, RegionPos};

/// The files of one region triple. Each kind may be missing.
pub type TripleFiles = wgc_core::layout::TripleFiles<PathBuf>;

/// The region triples and external chunk files of one dimension.
pub type DimensionFiles = wgc_core::layout::DimensionFiles<PathBuf>;

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
            let Some(name) = entry.file_name().into_string().ok() else {
                continue;
            };
            let path = entry.path();
            if let Some(pos) = parse_region_name(&name) {
                if !path.is_file() {
                    continue;
                }
                let triple = triples.entry(pos).or_insert_with(|| TripleFiles::new(pos));
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
    use std::path::Path;

    use super::*;
    use crate::types::{ChunkPos, DimensionId};

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
        let externals: Vec<ChunkPos> = files.externals(FileKind::Region).keys().copied().collect();
        assert_eq!(externals, [ChunkPos::new(-1, 3)]);
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
