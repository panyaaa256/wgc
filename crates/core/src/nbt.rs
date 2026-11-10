//! Reading the few NBT fields the tool needs (docs/spec/chunk-nbt.md).
//!
//! nanonbt is used only inside this module. Only the fields listed in the
//! spec are read; everything else is skipped with `nanonbt::skip_nbt`. A tag
//! with an unexpected type is treated like a missing tag.

use std::collections::BTreeSet;

use nanonbt::route::LongArray;
use nanonbt::{DeserializeNbtArray, Read, SliceReader, Tag};

use crate::types::{ChunkPos, DataVersion};
use crate::versions::DV_CHUNK_ROOT;

/// Why bytes cannot be read as NBT (content error C-2).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NbtError {
    #[error("nbt data ends early")]
    Truncated,
    #[error("unknown nbt tag type {0}")]
    InvalidTagType(u8),
    #[error("nbt string is not valid modified utf-8")]
    InvalidString,
    #[error("nbt root is not a compound")]
    RootNotCompound,
    #[error("nbt data is malformed")]
    Malformed,
}

impl From<nanonbt::Error> for NbtError {
    fn from(e: nanonbt::Error) -> Self {
        match e {
            nanonbt::Error::UnexpectedEof => NbtError::Truncated,
            nanonbt::Error::UnknownTag(t) => NbtError::InvalidTagType(t),
            nanonbt::Error::InvalidUtf8 => NbtError::InvalidString,
            _ => NbtError::Malformed,
        }
    }
}

/// The fields of a chunk that the plan needs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChunkSummary {
    pub data_version: Option<DataVersion>,
    /// `None` when the tag is missing or not a `Long`. Negative values are kept.
    pub inhabited_time: Option<i64>,
    pub has_blending_data: bool,
    /// Chunks holding the starts of structures this chunk is part of.
    pub structure_refs: BTreeSet<ChunkPos>,
}

/// Reads a [`ChunkSummary`] from decompressed chunk NBT.
pub fn read_chunk_summary(nbt: &[u8]) -> Result<ChunkSummary, NbtError> {
    let mut reader = root_compound(nbt)?;
    let mut root = Fields::default();
    let mut level = Fields::default();
    let mut data_version = None;
    let mut has_blending_data = false;

    for_each_entry(&mut reader, |name, tag, r| {
        match (name, tag) {
            (Some(b"DataVersion"), Tag::Int) => data_version = Some(DataVersion(r.read_i32_be()?)),
            (Some(b"blending_data"), Tag::Compound) => {
                has_blending_data = true;
                nanonbt::skip_nbt(r, tag)?;
            }
            (Some(b"Level"), Tag::Compound) => read_fields(r, &mut level, b"Structures")?,
            _ => return root.visit(name, tag, r, b"structures"),
        }
        Ok(true)
    })?;

    let fields = match data_version {
        Some(v) if v >= DV_CHUNK_ROOT => root,
        _ => level,
    };
    Ok(ChunkSummary {
        data_version,
        inhabited_time: fields.inhabited_time,
        has_blending_data,
        structure_refs: fields.structure_refs,
    })
}

/// Fields that live at the root (DataVersion ≥ 2844) or under `Level`.
#[derive(Default)]
struct Fields {
    inhabited_time: Option<i64>,
    structure_refs: BTreeSet<ChunkPos>,
}

impl Fields {
    /// Handles `name` if it is one of these fields; returns whether it was consumed.
    fn visit(
        &mut self,
        name: Option<&[u8]>,
        tag: Tag,
        r: &mut SliceReader<'_>,
        structures_key: &[u8],
    ) -> Result<bool, NbtError> {
        match (name, tag) {
            (Some(b"InhabitedTime"), Tag::Long) => self.inhabited_time = Some(r.read_i64_be()?),
            (Some(n), Tag::Compound) if n == structures_key => {
                for_each_entry(r, |name, tag, r| {
                    if name == Some(b"References") && tag == Tag::Compound {
                        read_references(r, &mut self.structure_refs)?;
                        Ok(true)
                    } else {
                        Ok(false)
                    }
                })?;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}

fn read_fields(
    r: &mut SliceReader<'_>,
    fields: &mut Fields,
    structures_key: &[u8],
) -> Result<(), NbtError> {
    for_each_entry(r, |name, tag, r| fields.visit(name, tag, r, structures_key))
}

/// `References`: structure id -> LongArray of packed chunk positions.
fn read_references(r: &mut SliceReader<'_>, out: &mut BTreeSet<ChunkPos>) -> Result<(), NbtError> {
    for_each_entry(r, |_, tag, r| {
        if tag != Tag::LongArray {
            return Ok(false);
        }
        let values = <Vec<i64> as DeserializeNbtArray<LongArray>>::deserialize_nbt_array(r)?;
        out.extend(values.into_iter().map(ChunkPos::from_packed));
        Ok(true)
    })
}

/// Checks the root header and returns a reader positioned at the root compound's payload.
fn root_compound(nbt: &[u8]) -> Result<SliceReader<'_>, NbtError> {
    let mut reader = SliceReader::new(nbt);
    let tag = reader.read_u8()?;
    if tag != Tag::Compound.as_u8() {
        return Err(if tag <= Tag::LongArray.as_u8() {
            NbtError::RootNotCompound
        } else {
            NbtError::InvalidTagType(tag)
        });
    }
    let name_len = reader.read_u16_be()? as usize;
    reader.skip(name_len)?;
    Ok(reader)
}

/// Longest compound key the tool matches (`InhabitedTime`, `blending_data`).
const MAX_NAME: usize = 16;

/// Walks the entries of a compound. `visit` returns `true` when it consumed
/// the value; otherwise the value is skipped. Names longer than [`MAX_NAME`]
/// are passed as `None` (they match no key the tool looks for).
fn for_each_entry<'a>(
    r: &mut SliceReader<'a>,
    mut visit: impl FnMut(Option<&[u8]>, Tag, &mut SliceReader<'a>) -> Result<bool, NbtError>,
) -> Result<(), NbtError> {
    let mut buf = [0u8; MAX_NAME];
    loop {
        let tag = Tag::from_u8(r.read_u8()?)?;
        if tag == Tag::End {
            return Ok(());
        }
        let name = nanonbt::read_name_into(r, &mut buf)?.map(|len| &buf[..len]);
        let consumed = visit(name, tag, r)?;
        if !consumed {
            nanonbt::skip_nbt(r, tag)?;
        }
    }
}

#[cfg(test)]
pub(crate) mod test_util {
    //! A tiny NBT writer for tests.

    pub enum V {
        Byte(i8),
        Int(i32),
        Long(i64),
        Str(&'static str),
        LongArray(Vec<i64>),
        IntArray(Vec<i32>),
        List(Vec<i32>),
        Compound(Vec<(&'static str, V)>),
    }

    fn tag(v: &V) -> u8 {
        match v {
            V::Byte(_) => 1,
            V::Int(_) => 3,
            V::Long(_) => 4,
            V::Str(_) => 8,
            V::List(_) => 9,
            V::Compound(_) => 10,
            V::IntArray(_) => 11,
            V::LongArray(_) => 12,
        }
    }

    fn name(out: &mut Vec<u8>, s: &str) {
        out.extend_from_slice(&(s.len() as u16).to_be_bytes());
        out.extend_from_slice(s.as_bytes());
    }

    fn payload(out: &mut Vec<u8>, v: &V) {
        match v {
            V::Byte(b) => out.push(*b as u8),
            V::Int(i) => out.extend_from_slice(&i.to_be_bytes()),
            V::Long(l) => out.extend_from_slice(&l.to_be_bytes()),
            V::Str(s) => name(out, s),
            V::List(items) => {
                out.push(3);
                out.extend_from_slice(&(items.len() as i32).to_be_bytes());
                for i in items {
                    out.extend_from_slice(&i.to_be_bytes());
                }
            }
            V::IntArray(items) => {
                out.extend_from_slice(&(items.len() as i32).to_be_bytes());
                for i in items {
                    out.extend_from_slice(&i.to_be_bytes());
                }
            }
            V::LongArray(items) => {
                out.extend_from_slice(&(items.len() as i32).to_be_bytes());
                for l in items {
                    out.extend_from_slice(&l.to_be_bytes());
                }
            }
            V::Compound(entries) => {
                for (k, v) in entries {
                    out.push(tag(v));
                    name(out, k);
                    payload(out, v);
                }
                out.push(0);
            }
        }
    }

    /// A complete document with an empty root name.
    pub fn doc(root: Vec<(&'static str, V)>) -> Vec<u8> {
        let mut out = vec![10];
        name(&mut out, "");
        payload(&mut out, &V::Compound(root));
        out
    }

    pub fn pack(x: i32, z: i32) -> i64 {
        (x as u32 as i64) | ((z as i64) << 32)
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::*;
    use super::*;

    fn big_sections() -> V {
        V::List((0..4000).collect())
    }

    #[test]
    fn modern_root_fields() {
        let nbt = doc(vec![
            ("sections", big_sections()),
            ("DataVersion", V::Int(4325)),
            ("Status", V::Str("minecraft:full")),
            ("InhabitedTime", V::Long(1234)),
            (
                "blending_data",
                V::Compound(vec![("min_section", V::Int(-4))]),
            ),
            (
                "structures",
                V::Compound(vec![
                    ("starts", V::Compound(vec![])),
                    (
                        "References",
                        V::Compound(vec![
                            (
                                "minecraft:village_plains",
                                V::LongArray(vec![pack(-1, 2), pack(3, -4)]),
                            ),
                            ("minecraft:mineshaft", V::LongArray(vec![pack(-1, 2)])),
                        ]),
                    ),
                ]),
            ),
        ]);
        let s = read_chunk_summary(&nbt).unwrap();
        assert_eq!(s.data_version, Some(DataVersion(4325)));
        assert_eq!(s.inhabited_time, Some(1234));
        assert!(s.has_blending_data);
        assert_eq!(
            s.structure_refs,
            BTreeSet::from([ChunkPos::new(-1, 2), ChunkPos::new(3, -4)])
        );
    }

    fn legacy_level() -> V {
        V::Compound(vec![
            ("Sections", big_sections()),
            ("InhabitedTime", V::Long(77)),
            (
                "Structures",
                V::Compound(vec![(
                    "References",
                    V::Compound(vec![("Village", V::LongArray(vec![pack(5, 6)]))]),
                )]),
            ),
        ])
    }

    #[test]
    fn legacy_level_fields() {
        for root in [
            vec![("DataVersion", V::Int(1343)), ("Level", legacy_level())],
            vec![("Level", legacy_level())], // pre-1.9: no DataVersion
        ] {
            let s = read_chunk_summary(&doc(root)).unwrap();
            assert_eq!(s.inhabited_time, Some(77));
            assert_eq!(s.structure_refs, BTreeSet::from([ChunkPos::new(5, 6)]));
            assert!(!s.has_blending_data);
        }
    }

    #[test]
    fn version_decides_which_side_is_used() {
        // Modern version: a stale Level is ignored.
        let s = read_chunk_summary(&doc(vec![
            ("DataVersion", V::Int(3000)),
            ("Level", V::Compound(vec![("InhabitedTime", V::Long(5))])),
        ]))
        .unwrap();
        assert_eq!(s.inhabited_time, None);
        // Old version: a root InhabitedTime is ignored.
        let s = read_chunk_summary(&doc(vec![
            ("DataVersion", V::Int(2000)),
            ("InhabitedTime", V::Long(5)),
        ]))
        .unwrap();
        assert_eq!(s.inhabited_time, None);
    }

    #[test]
    fn missing_or_mistyped_inhabited_time_is_none() {
        let s = read_chunk_summary(&doc(vec![("DataVersion", V::Int(4000))])).unwrap();
        assert_eq!(s.inhabited_time, None);
        let s = read_chunk_summary(&doc(vec![
            ("DataVersion", V::Int(4000)),
            ("InhabitedTime", V::Int(9)),
        ]))
        .unwrap();
        assert_eq!(s.inhabited_time, None);
        let s = read_chunk_summary(&doc(vec![
            ("DataVersion", V::Int(4000)),
            ("InhabitedTime", V::Long(-3)),
        ]))
        .unwrap();
        assert_eq!(s.inhabited_time, Some(-3));
    }

    #[test]
    fn mistyped_structure_data_is_ignored() {
        let s = read_chunk_summary(&doc(vec![
            ("DataVersion", V::Int(4000)),
            (
                "structures",
                V::Compound(vec![(
                    "References",
                    V::Compound(vec![("x", V::IntArray(vec![1, 2])), ("y", V::Byte(1))]),
                )]),
            ),
        ]))
        .unwrap();
        assert!(s.structure_refs.is_empty());
    }

    #[test]
    fn errors() {
        assert_eq!(read_chunk_summary(&[]), Err(NbtError::Truncated));
        assert_eq!(
            read_chunk_summary(&[3, 0, 0, 0, 0, 0, 1]),
            Err(NbtError::RootNotCompound)
        );
        assert_eq!(read_chunk_summary(&[99]), Err(NbtError::InvalidTagType(99)));
        let mut nbt = doc(vec![("DataVersion", V::Int(1))]);
        nbt.pop(); // drop the root End tag
        assert_eq!(read_chunk_summary(&nbt), Err(NbtError::Truncated));
    }

    /// Random corruption of a valid document must never panic.
    #[test]
    fn fuzz_never_panics() {
        let base = doc(vec![
            ("DataVersion", V::Int(4325)),
            ("InhabitedTime", V::Long(10)),
            ("sections", V::List((0..50).collect())),
            (
                "structures",
                V::Compound(vec![(
                    "References",
                    V::Compound(vec![("a", V::LongArray(vec![1, 2, 3]))]),
                )]),
            ),
            ("Level", V::Compound(vec![("InhabitedTime", V::Long(1))])),
        ]);
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..20_000 {
            let mut bytes = base.clone();
            for _ in 0..1 + next() % 4 {
                let i = (next() as usize) % bytes.len();
                bytes[i] = next() as u8;
            }
            if next() % 4 == 0 {
                bytes.truncate((next() as usize) % bytes.len());
            }
            let _ = read_chunk_summary(&bytes);
        }
    }
}
