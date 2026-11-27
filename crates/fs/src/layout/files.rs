//! Enumerating region triples.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::path::{Path, PathBuf};

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
            let region = parse_region_name(&name);
            let external = parse_external_name(&name);
            let path = entry.path();
            if (region.is_none() && external.is_none()) || !path.is_file() {
                continue;
            }
            if let Some(pos) = region {
                let triple = triples.entry(pos).or_insert_with(|| TripleFiles::new(pos));
                match &mut triple.files[kind] {
                    Some(first) => return Err(ambiguous(first, path)),
                    slot => *slot = Some(path),
                }
            } else if let Some(pos) = external {
                match files.externals[kind].entry(pos) {
                    Entry::Occupied(first) => return Err(ambiguous(first.get(), path)),
                    Entry::Vacant(slot) => {
                        slot.insert(path);
                    }
                }
            }
        }
    }
    files.triples = triples.into_values().collect();
    Ok(files)
}

fn ambiguous(first: &Path, second: PathBuf) -> LayoutError {
    LayoutError::AmbiguousRegion {
        first: first.to_path_buf(),
        second,
    }
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
        let present = origin.files.as_ref().map(Option::is_some);
        assert_eq!(present.into_iter().collect::<Vec<_>>(), [true, true, false]);
        let externals: Vec<ChunkPos> = files.externals.region.keys().copied().collect();
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
