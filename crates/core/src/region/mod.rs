//! Anvil region files (docs/spec/region-format.md).
//!
//! Parsing never fails: problems with the file structure are collected as
//! [`StructuralIssue`]s, and problems with a single chunk's data surface as
//! [`DecompressError`] when it is decompressed.

mod compression;
mod write;

use std::collections::BTreeSet;
use std::fmt;

use crate::types::{ChunkIndex, ChunkPos, FileKind, RegionPos};

pub use compression::{DecompressError, decompress};
pub use write::write_region;

/// Size of a sector in bytes.
pub const SECTOR_BYTES: usize = 4096;
/// Size of the header (location table + timestamps) in bytes.
pub const HEADER_BYTES: usize = 2 * SECTOR_BYTES;
/// Compression types at or above this value store the data in a `.mcc` file.
pub const EXTERNAL_FLAG: u8 = 128;

/// Parses a region file name `r.<x>.<z>.mca`.
pub fn parse_region_name(name: &str) -> Option<RegionPos> {
    let (x, z) = parse_pair(name, "r.", ".mca")?;
    Some(RegionPos::new(x, z))
}

/// Parses an external chunk file name `c.<x>.<z>.mcc`.
pub fn parse_external_name(name: &str) -> Option<ChunkPos> {
    let (x, z) = parse_pair(name, "c.", ".mcc")?;
    Some(ChunkPos::new(x, z))
}

/// File name of the external file for `pos`.
pub fn external_name(pos: ChunkPos) -> String {
    format!("c.{}.{}.mcc", pos.x, pos.z)
}

fn parse_pair(name: &str, prefix: &str, suffix: &str) -> Option<(i32, i32)> {
    let inner = name.strip_prefix(prefix)?.strip_suffix(suffix)?;
    let (x, z) = inner.split_once('.')?;
    Some((x.parse().ok()?, z.parse().ok()?))
}

/// Structural problem IDs S-1..S-8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StructuralId {
    /// File size is between 1 and 8191 bytes.
    S1,
    /// Only one of the sector offset and the sector count is zero.
    S2,
    /// The sector offset is below 2.
    S3,
    /// The payload extends past the end of the file.
    S4,
    /// The length field is zero.
    S5,
    /// The payload is longer than its sectors.
    S6,
    /// Two chunks share sectors.
    S7,
    /// The external `.mcc` file is missing.
    S8,
}

impl StructuralId {
    pub const fn as_str(self) -> &'static str {
        match self {
            StructuralId::S1 => "S-1",
            StructuralId::S2 => "S-2",
            StructuralId::S3 => "S-3",
            StructuralId::S4 => "S-4",
            StructuralId::S5 => "S-5",
            StructuralId::S6 => "S-6",
            StructuralId::S7 => "S-7",
            StructuralId::S8 => "S-8",
        }
    }
}

impl fmt::Display for StructuralId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A structural problem in one file of a region triple.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StructuralIssue {
    pub kind: FileKind,
    pub id: StructuralId,
    /// `None` when the problem concerns the whole file (S-1).
    pub index: Option<ChunkIndex>,
}

/// A problem with a single chunk's data.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContentError {
    /// C-1.
    #[error("cannot decompress chunk")]
    Decompress(#[from] DecompressError),
    /// C-2. Holds the NBT error once the `nbt` module exists (P2).
    #[error("cannot read chunk nbt: {0}")]
    Nbt(String),
}

/// A chunk slot whose payload lies inside the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub index: ChunkIndex,
    pub pos: ChunkPos,
    pub sector_offset: u32,
    pub sector_count: u8,
    pub timestamp: u32,
    /// The length field: compression byte plus data.
    pub length: u32,
    pub compression: u8,
}

impl Entry {
    /// Whether the data is stored in an external `.mcc` file.
    pub const fn is_external(&self) -> bool {
        self.compression >= EXTERNAL_FLAG
    }

    /// Size of the payload (length field, compression byte and data).
    pub const fn payload_len(&self) -> usize {
        4 + self.length as usize
    }

    /// Number of sectors the payload needs when packed.
    pub const fn packed_sectors(&self) -> usize {
        self.payload_len().div_ceil(SECTOR_BYTES)
    }

    fn payload_start(&self) -> usize {
        self.sector_offset as usize * SECTOR_BYTES
    }
}

/// A parsed region file. Owns its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionFile {
    kind: FileKind,
    pos: RegionPos,
    bytes: Vec<u8>,
    entries: Vec<Entry>,
    issues: Vec<StructuralIssue>,
}

impl RegionFile {
    /// Parses `bytes` as the region file of kind `kind` at `pos`.
    ///
    /// `external` lists the chunks that have a `.mcc` file next to the region file.
    pub fn parse(
        kind: FileKind,
        pos: RegionPos,
        bytes: Vec<u8>,
        external: &BTreeSet<ChunkPos>,
    ) -> Self {
        let mut file = Self {
            kind,
            pos,
            bytes,
            entries: Vec::new(),
            issues: Vec::new(),
        };
        file.scan(external);
        file
    }

    fn scan(&mut self, external: &BTreeSet<ChunkPos>) {
        let len = self.bytes.len();
        if len == 0 {
            return;
        }
        if len < HEADER_BYTES {
            self.issue(StructuralId::S1, None);
            return;
        }

        let mut ranges: Vec<(u32, u32, ChunkIndex)> = Vec::new();
        for index in ChunkIndex::all() {
            let i = index.get() as usize;
            let location = be_u32(&self.bytes, i * 4);
            if location == 0 {
                continue;
            }
            let sector_offset = location >> 8;
            let sector_count = (location & 0xFF) as u8;
            if sector_offset == 0 || sector_count == 0 {
                self.issue(StructuralId::S2, Some(index));
                continue;
            }
            if sector_offset < 2 {
                self.issue(StructuralId::S3, Some(index));
                continue;
            }
            ranges.push((
                sector_offset,
                sector_offset + u32::from(sector_count),
                index,
            ));

            let start = sector_offset as usize * SECTOR_BYTES;
            if start + 5 > len {
                self.issue(StructuralId::S4, Some(index));
                continue;
            }
            let length = be_u32(&self.bytes, start);
            if length == 0 {
                self.issue(StructuralId::S5, Some(index));
                continue;
            }
            if start + 4 + length as usize > len {
                self.issue(StructuralId::S4, Some(index));
                continue;
            }
            if 4 + length as usize > usize::from(sector_count) * SECTOR_BYTES {
                self.issue(StructuralId::S6, Some(index));
            }
            let pos = index.to_chunk_pos(self.pos);
            let entry = Entry {
                index,
                pos,
                sector_offset,
                sector_count,
                timestamp: be_u32(&self.bytes, SECTOR_BYTES + i * 4),
                length,
                compression: self.bytes[start + 4],
            };
            if entry.is_external() && !external.contains(&pos) {
                self.issue(StructuralId::S8, Some(index));
            }
            self.entries.push(entry);
        }

        ranges.sort_unstable();
        let mut overlapping = BTreeSet::new();
        for (i, a) in ranges.iter().enumerate() {
            for b in &ranges[i + 1..] {
                if b.0 >= a.1 {
                    break;
                }
                overlapping.insert(a.2);
                overlapping.insert(b.2);
            }
        }
        for index in overlapping {
            self.issue(StructuralId::S7, Some(index));
        }
        self.issues.sort();
    }

    fn issue(&mut self, id: StructuralId, index: Option<ChunkIndex>) {
        self.issues.push(StructuralIssue {
            kind: self.kind,
            id,
            index,
        });
    }

    pub fn kind(&self) -> FileKind {
        self.kind
    }

    pub fn pos(&self) -> RegionPos {
        self.pos
    }

    /// Size of the file in bytes.
    pub fn size(&self) -> u64 {
        self.bytes.len() as u64
    }

    /// Structural problems, sorted.
    pub fn issues(&self) -> &[StructuralIssue] {
        &self.issues
    }

    /// Chunks whose payload lies inside the file, by ascending index.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The entry at `index`, if its payload lies inside the file.
    pub fn entry(&self, index: ChunkIndex) -> Option<&Entry> {
        self.entries
            .binary_search_by_key(&index, |e| e.index)
            .ok()
            .map(|i| &self.entries[i])
    }

    /// The payload (length field, compression byte and data) of the chunk at `index`.
    pub fn payload(&self, index: ChunkIndex) -> Option<&[u8]> {
        let entry = self.entry(index)?;
        let start = entry.payload_start();
        Some(&self.bytes[start..start + entry.payload_len()])
    }
}

fn be_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(bytes[at..at + 4].try_into().expect("4 bytes"))
}

#[cfg(test)]
pub(crate) mod test_util {
    use super::*;

    /// A chunk to place in a synthetic region file.
    pub struct Chunk {
        pub index: u16,
        pub compression: u8,
        pub data: Vec<u8>,
        pub timestamp: u32,
    }

    pub fn chunk(index: u16, compression: u8, data: &[u8]) -> Chunk {
        Chunk {
            index,
            compression,
            data: data.to_vec(),
            timestamp: 1_700_000_000 + u32::from(index),
        }
    }

    /// Builds a well-formed region file, placing chunks in the given order.
    pub fn build(chunks: &[Chunk]) -> Vec<u8> {
        let mut out = vec![0u8; HEADER_BYTES];
        for c in chunks {
            let offset = out.len() / SECTOR_BYTES;
            let length = c.data.len() as u32 + 1;
            out.extend_from_slice(&length.to_be_bytes());
            out.push(c.compression);
            out.extend_from_slice(&c.data);
            out.resize(out.len().div_ceil(SECTOR_BYTES) * SECTOR_BYTES, 0);
            let count = out.len() / SECTOR_BYTES - offset;
            let i = c.index as usize;
            let location = ((offset as u32) << 8) | count as u32;
            out[i * 4..i * 4 + 4].copy_from_slice(&location.to_be_bytes());
            out[SECTOR_BYTES + i * 4..SECTOR_BYTES + i * 4 + 4]
                .copy_from_slice(&c.timestamp.to_be_bytes());
        }
        out
    }

    pub fn set_location(bytes: &mut [u8], index: u16, offset: u32, count: u8) {
        let i = index as usize;
        bytes[i * 4..i * 4 + 4].copy_from_slice(&((offset << 8) | u32::from(count)).to_be_bytes());
    }

    pub fn parse(bytes: Vec<u8>) -> RegionFile {
        RegionFile::parse(
            FileKind::Region,
            RegionPos::new(0, 0),
            bytes,
            &BTreeSet::new(),
        )
    }

    pub fn ids(file: &RegionFile) -> Vec<(StructuralId, Option<u16>)> {
        file.issues()
            .iter()
            .map(|i| (i.id, i.index.map(ChunkIndex::get)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::*;
    use super::*;

    #[test]
    fn names() {
        assert_eq!(parse_region_name("r.-1.2.mca"), Some(RegionPos::new(-1, 2)));
        assert_eq!(parse_region_name("r.00.00.mca"), Some(RegionPos::new(0, 0)));
        assert_eq!(parse_region_name("r.0.0.mca.bak"), None);
        assert_eq!(parse_region_name("r.0.mca"), None);
        assert_eq!(parse_region_name("r.a.0.mca"), None);
        assert_eq!(
            parse_external_name("c.-33.5.mcc"),
            Some(ChunkPos::new(-33, 5))
        );
        assert_eq!(external_name(ChunkPos::new(-33, 5)), "c.-33.5.mcc");
    }

    #[test]
    fn well_formed_file_has_no_issues() {
        let bytes = build(&[
            chunk(0, 2, b"aaa"),
            chunk(1023, 3, &[7; 5000]),
            chunk(40, 1, b"b"),
        ]);
        let file = parse(bytes);
        assert!(file.issues().is_empty());
        let indices: Vec<u16> = file.entries().iter().map(|e| e.index.get()).collect();
        assert_eq!(indices, [0, 40, 1023]);
        let e = file.entry(ChunkIndex::new(1023).unwrap()).unwrap();
        assert_eq!((e.sector_count, e.length, e.compression), (2, 5001, 3));
        assert_eq!(e.pos, ChunkPos::new(31, 31));
        assert_eq!(e.timestamp, 1_700_001_023);
        assert_eq!(
            &file.payload(ChunkIndex::new(0).unwrap()).unwrap()[4..],
            b"\x02aaa"
        );
        assert_eq!(file.payload(ChunkIndex::new(1).unwrap()), None);
    }

    #[test]
    fn empty_file_is_not_an_error() {
        let file = parse(Vec::new());
        assert!(file.issues().is_empty());
        assert!(file.entries().is_empty());
    }

    #[test]
    fn s1_short_header() {
        assert_eq!(ids(&parse(vec![0; 100])), [(StructuralId::S1, None)]);
        assert_eq!(
            ids(&parse(vec![0; HEADER_BYTES - 1])),
            [(StructuralId::S1, None)]
        );
    }

    #[test]
    fn s2_half_empty_location() {
        let mut bytes = build(&[]);
        set_location(&mut bytes, 5, 0, 1);
        set_location(&mut bytes, 6, 2, 0);
        assert_eq!(
            ids(&parse(bytes)),
            [(StructuralId::S2, Some(5)), (StructuralId::S2, Some(6))]
        );
    }

    #[test]
    fn s3_offset_in_header() {
        let mut bytes = build(&[]);
        set_location(&mut bytes, 7, 1, 1);
        assert_eq!(ids(&parse(bytes)), [(StructuralId::S3, Some(7))]);
    }

    #[test]
    fn s4_past_end_of_file() {
        let mut bytes = build(&[]);
        set_location(&mut bytes, 1, 9, 1);
        assert_eq!(ids(&parse(bytes)), [(StructuralId::S4, Some(1))]);

        let mut bytes = build(&[chunk(2, 2, &[1; 100])]);
        bytes.truncate(HEADER_BYTES + 50);
        assert_eq!(ids(&parse(bytes)), [(StructuralId::S4, Some(2))]);
    }

    #[test]
    fn short_last_sector_is_fine() {
        let mut bytes = build(&[chunk(2, 2, &[1; 100])]);
        bytes.truncate(HEADER_BYTES + 4 + 101);
        let file = parse(bytes);
        assert!(file.issues().is_empty());
        assert_eq!(file.entries().len(), 1);
    }

    #[test]
    fn s5_zero_length() {
        let mut bytes = build(&[chunk(3, 2, b"x")]);
        bytes[HEADER_BYTES..HEADER_BYTES + 4].copy_from_slice(&0u32.to_be_bytes());
        assert_eq!(ids(&parse(bytes)), [(StructuralId::S5, Some(3))]);
    }

    #[test]
    fn s6_payload_longer_than_sectors() {
        let mut bytes = build(&[chunk(4, 2, &[1; 5000])]);
        set_location(&mut bytes, 4, 2, 1);
        assert_eq!(ids(&parse(bytes)), [(StructuralId::S6, Some(4))]);
    }

    #[test]
    fn s7_shared_sectors() {
        let mut bytes = build(&[chunk(8, 2, b"x"), chunk(9, 2, b"y")]);
        set_location(&mut bytes, 9, 2, 1);
        assert_eq!(
            ids(&parse(bytes)),
            [(StructuralId::S7, Some(8)), (StructuralId::S7, Some(9))]
        );
    }

    #[test]
    fn s8_missing_external_file() {
        let bytes = build(&[chunk(10, 2 + EXTERNAL_FLAG, b"")]);
        assert_eq!(ids(&parse(bytes.clone())), [(StructuralId::S8, Some(10))]);

        let present = BTreeSet::from([ChunkPos::new(10, 0)]);
        let file = RegionFile::parse(FileKind::Region, RegionPos::new(0, 0), bytes, &present);
        assert!(file.issues().is_empty());
        assert!(file.entries()[0].is_external());
    }

    #[test]
    fn issues_carry_the_file_kind() {
        let file = RegionFile::parse(
            FileKind::Poi,
            RegionPos::new(0, 0),
            vec![1],
            &BTreeSet::new(),
        );
        assert_eq!(file.issues()[0].kind, FileKind::Poi);
    }
}
