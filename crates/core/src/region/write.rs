//! Repacking a region file.

use super::{HEADER_BYTES, RegionFile, SECTOR_BYTES};
use crate::types::ChunkIndex;

/// Why a region file cannot be repacked.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WriteError {
    #[error("no chunk at index {}", .0.get())]
    MissingEntry(ChunkIndex),
    #[error("chunk at index {} needs more than 255 sectors", .0.get())]
    TooManySectors(ChunkIndex),
}

/// Builds a new region file that contains only the chunks in `keep`.
///
/// Chunks are placed by ascending index from sector 2. Each payload is copied
/// byte for byte and padded to a sector boundary. Timestamps of kept chunks
/// are preserved; all other slots are zero. `.mcc` files are not touched.
///
/// # Errors
///
/// Fails if an index in `keep` has no entry in `source`, or if a payload
/// needs more than 255 sectors. The plan only rewrites files without
/// structural issues, so neither should happen there.
pub fn write_region(source: &RegionFile, keep: &[ChunkIndex]) -> Result<Vec<u8>, WriteError> {
    let mut keep = keep.to_vec();
    keep.sort_unstable();
    keep.dedup();

    let mut out = vec![0u8; HEADER_BYTES];
    for index in keep {
        let entry = source.entry(index).ok_or(WriteError::MissingEntry(index))?;
        let payload = source
            .payload(index)
            .ok_or(WriteError::MissingEntry(index))?;
        let offset = out.len() / SECTOR_BYTES;
        let count =
            u8::try_from(entry.packed_sectors()).map_err(|_| WriteError::TooManySectors(index))?;
        out.extend_from_slice(payload);
        out.resize(out.len().div_ceil(SECTOR_BYTES) * SECTOR_BYTES, 0);

        let i = index.get() as usize;
        let location = ((offset as u32) << 8) | u32::from(count);
        out[i * 4..i * 4 + 4].copy_from_slice(&location.to_be_bytes());
        out[SECTOR_BYTES + i * 4..SECTOR_BYTES + i * 4 + 4]
            .copy_from_slice(&entry.timestamp.to_be_bytes());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::super::test_util::*;
    use super::*;
    use crate::region::EXTERNAL_FLAG;
    use crate::types::{ChunkPos, FileKind, RegionPos};

    fn idx(i: u16) -> ChunkIndex {
        ChunkIndex::new(i).unwrap()
    }

    #[test]
    fn repacked_file_keeps_payloads_and_timestamps() {
        // Chunks placed out of index order, with a gap that repacking removes.
        let source = parse(build(&[
            chunk(900, 2, &[9; 9000]),
            chunk(3, 1, b"three"),
            chunk(500, 3, &[5; 4091]), // exactly one sector: 4 + 1 + 4091
            chunk(7, 2, b"dropped"),
        ]));
        assert!(source.issues().is_empty());

        let keep = [idx(900), idx(3), idx(500)];
        let bytes = write_region(&source, &keep).unwrap();
        let packed = parse(bytes.clone());
        assert!(packed.issues().is_empty());

        let expected: usize = HEADER_BYTES
            + keep
                .iter()
                .map(|&i| source.entry(i).unwrap().packed_sectors() * SECTOR_BYTES)
                .sum::<usize>();
        assert_eq!(bytes.len(), expected);

        let kept: Vec<u16> = packed.entries().iter().map(|e| e.index.get()).collect();
        assert_eq!(kept, [3, 500, 900]);
        for &i in &keep {
            assert_eq!(packed.payload(i), source.payload(i));
            assert_eq!(
                packed.entry(i).unwrap().timestamp,
                source.entry(i).unwrap().timestamp
            );
        }
        // Placed by ascending index from sector 2.
        assert_eq!(packed.entry(idx(3)).unwrap().sector_offset, 2);
        assert_eq!(packed.entry(idx(500)).unwrap().sector_offset, 3);
        assert_eq!(packed.entry(idx(900)).unwrap().sector_offset, 4);
        assert_eq!(packed.payload(idx(7)), None);
    }

    #[test]
    fn external_stub_is_copied() {
        let external = BTreeSet::from([ChunkPos::new(1, 0)]);
        let source = RegionFile::parse(
            FileKind::Entities,
            RegionPos::new(0, 0),
            build(&[chunk(1, 2 + EXTERNAL_FLAG, b"")]),
            &external,
        );
        let packed = RegionFile::parse(
            FileKind::Entities,
            RegionPos::new(0, 0),
            write_region(&source, &[idx(1)]).unwrap(),
            &external,
        );
        assert!(packed.issues().is_empty());
        assert_eq!(packed.payload(idx(1)), source.payload(idx(1)));
    }

    #[test]
    fn keeping_nothing_leaves_only_the_header() {
        let source = parse(build(&[chunk(1, 2, b"x")]));
        assert_eq!(write_region(&source, &[]).unwrap(), vec![0u8; HEADER_BYTES]);
    }

    #[test]
    fn missing_entry_is_an_error() {
        let source = parse(build(&[chunk(1, 2, b"x")]));
        assert_eq!(
            write_region(&source, &[idx(2)]),
            Err(WriteError::MissingEntry(idx(2)))
        );
    }
}
