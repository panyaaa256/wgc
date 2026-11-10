//! Reading the few NBT fields the tool needs (docs/spec/chunk-nbt.md).
//!
//! nanonbt is used only inside this module. Only the fields listed in the
//! spec are read; everything else is skipped with `nanonbt::skip_nbt`. A tag
//! with an unexpected type is treated like a missing tag.

use std::collections::BTreeSet;

use nanonbt::{Read, SliceReader, Tag};

use std::io::Read as _;

use crate::types::{ChunkPos, DataVersion};
use crate::versions::{DV_CHUNK_ROOT, DV_GAMERULE_REGISTRY};

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
            (Some(b"Level"), Tag::Compound) => {
                for_each_entry(r, |n, t, r| level.visit(n, t, r, b"Structures"))?;
            }
            _ => return root.visit(name, tag, r, b"structures"),
        }
        Ok(true)
    })?;

    let modern = matches!(data_version, Some(v) if v >= DV_CHUNK_ROOT);
    let fields = if modern { root } else { level };
    Ok(ChunkSummary {
        data_version,
        inhabited_time: fields.inhabited_time,
        // blending_data only exists in the root format.
        has_blending_data: has_blending_data && modern,
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
                // Like the game, the last occurrence of a duplicated key wins.
                self.structure_refs.clear();
                for_each_entry(r, |name, tag, r| {
                    if name == Some(b"References") && tag == Tag::Compound {
                        self.structure_refs.clear();
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

/// `References`: structure id -> LongArray of packed chunk positions.
fn read_references(r: &mut SliceReader<'_>, out: &mut BTreeSet<ChunkPos>) -> Result<(), NbtError> {
    for_each_entry(r, |_, tag, r| {
        if tag != Tag::LongArray {
            return Ok(false);
        }
        for _ in 0..nanonbt::read_count(r)? {
            out.insert(ChunkPos::from_packed(r.read_i64_be()?));
        }
        Ok(true)
    })
}

/// Largest decompressed size accepted for `level.dat` and saved data files.
const MAX_GZIP_BYTES: u64 = 64 * 1024 * 1024;

/// Decompresses a gzip file such as `level.dat` or a saved data file.
pub fn gunzip(bytes: &[u8]) -> Result<Vec<u8>, NbtError> {
    let mut out = Vec::new();
    flate2::read::MultiGzDecoder::new(bytes)
        .take(MAX_GZIP_BYTES + 1)
        .read_to_end(&mut out)
        .map_err(|_| NbtError::Malformed)?;
    if out.len() as u64 > MAX_GZIP_BYTES {
        return Err(NbtError::Malformed);
    }
    Ok(out)
}

/// The value of a boolean game rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleValue {
    /// Not stored: the rule has its default value.
    Absent,
    Value(bool),
    /// Stored with an unexpected type or value.
    Unreadable,
}

/// Fields of `level.dat` the tool needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LevelInfo {
    pub data_version: Option<DataVersion>,
    /// `doMobSpawning` / `minecraft:spawn_mobs` when stored in `level.dat` (before 4772).
    pub spawn_mobs: RuleValue,
}

/// Reads [`LevelInfo`] from decompressed `level.dat` NBT.
pub fn read_level_info(nbt: &[u8]) -> Result<LevelInfo, NbtError> {
    let mut reader = root_compound(nbt)?;
    let mut data_version = None;
    let mut legacy = RuleValue::Absent; // Data.GameRules.doMobSpawning (String)
    let mut registry = RuleValue::Absent; // Data.game_rules."minecraft:spawn_mobs" (Byte)
    for_each_entry(&mut reader, |name, tag, r| {
        if name != Some(b"Data") || tag != Tag::Compound {
            return Ok(false);
        }
        for_each_entry(r, |name, tag, r| {
            match (name, tag) {
                (Some(b"DataVersion"), Tag::Int) => {
                    data_version = Some(DataVersion(r.read_i32_be()?))
                }
                (Some(b"GameRules"), Tag::Compound) => {
                    for_each_entry(r, |name, tag, r| {
                        if name != Some(b"doMobSpawning") {
                            return Ok(false);
                        }
                        legacy = if tag == Tag::String {
                            match read_string_bytes(r)?.as_slice() {
                                b"true" => RuleValue::Value(true),
                                b"false" => RuleValue::Value(false),
                                _ => RuleValue::Unreadable,
                            }
                        } else {
                            RuleValue::Unreadable
                        };
                        Ok(tag == Tag::String)
                    })?;
                }
                (Some(b"game_rules"), Tag::Compound) => registry = read_spawn_mobs(r)?,
                _ => return Ok(false),
            }
            Ok(true)
        })?;
        Ok(true)
    })?;
    let spawn_mobs = match data_version {
        Some(v) if v >= DV_GAMERULE_REGISTRY => registry,
        _ => legacy,
    };
    Ok(LevelInfo {
        data_version,
        spawn_mobs,
    })
}

/// Reads `minecraft:spawn_mobs` from a decompressed `game_rules.dat` (4772 and later).
pub fn read_game_rules_file(nbt: &[u8]) -> Result<RuleValue, NbtError> {
    let mut reader = root_compound(nbt)?;
    let mut value = RuleValue::Absent;
    for_each_entry(&mut reader, |name, tag, r| {
        if name == Some(b"data") && tag == Tag::Compound {
            value = read_spawn_mobs(r)?;
            Ok(true)
        } else {
            Ok(false)
        }
    })?;
    Ok(value)
}

/// Reads `minecraft:spawn_mobs` (or `spawn_mobs`) as a Byte from a game rules compound.
fn read_spawn_mobs(r: &mut SliceReader<'_>) -> Result<RuleValue, NbtError> {
    let mut value = RuleValue::Absent;
    for_each_entry(r, |name, tag, r| {
        if !matches!(name, Some(b"minecraft:spawn_mobs" | b"spawn_mobs")) {
            return Ok(false);
        }
        value = if tag == Tag::Byte {
            match r.read_u8()? {
                0 => RuleValue::Value(false),
                1 => RuleValue::Value(true),
                _ => RuleValue::Unreadable,
            }
        } else {
            RuleValue::Unreadable
        };
        Ok(tag == Tag::Byte)
    })?;
    Ok(value)
}

/// Reads the `/forceload` chunks from a decompressed `chunks.dat` or `chunk_tickets.dat`.
///
/// Handles `data.Forced` (LongArray, before 4297) and `data.tickets` (4297 and
/// later, `minecraft:forced` tickets only). A malformed ticket is an error so
/// that the caller treats the protection as unknown.
pub fn read_forced_chunks(nbt: &[u8]) -> Result<BTreeSet<ChunkPos>, NbtError> {
    let mut reader = root_compound(nbt)?;
    let mut out = BTreeSet::new();
    for_each_entry(&mut reader, |name, tag, r| {
        if name != Some(b"data") || tag != Tag::Compound {
            return Ok(false);
        }
        for_each_entry(r, |name, tag, r| match (name, tag) {
            (Some(b"Forced"), Tag::LongArray) => {
                for _ in 0..nanonbt::read_count(r)? {
                    out.insert(ChunkPos::from_packed(r.read_i64_be()?));
                }
                Ok(true)
            }
            (Some(b"tickets"), Tag::List) => {
                let element = Tag::from_u8(r.read_u8()?)?;
                let count = nanonbt::read_count(r)?;
                if count > 0 && element != Tag::Compound {
                    return Err(NbtError::Malformed);
                }
                for _ in 0..count {
                    if let Some(pos) = read_forced_ticket(r)? {
                        out.insert(pos);
                    }
                }
                Ok(true)
            }
            (Some(b"Forced" | b"tickets"), _) => Err(NbtError::Malformed),
            _ => Ok(false),
        })?;
        Ok(true)
    })?;
    Ok(out)
}

/// One ticket compound; returns its position if it is a `minecraft:forced` ticket.
fn read_forced_ticket(r: &mut SliceReader<'_>) -> Result<Option<ChunkPos>, NbtError> {
    let mut ty = None; // Some(true) for a `minecraft:forced` ticket
    let mut pos = None;
    for_each_entry(r, |name, tag, r| match (name, tag) {
        (Some(b"type"), Tag::String) => {
            ty = Some(read_string_bytes(r)? == b"minecraft:forced");
            Ok(true)
        }
        (Some(b"chunk_pos"), Tag::IntArray) => {
            if nanonbt::read_count(r)? != 2 {
                return Err(NbtError::Malformed);
            }
            pos = Some(ChunkPos::new(r.read_i32_be()?, r.read_i32_be()?));
            Ok(true)
        }
        (Some(b"chunk_pos"), Tag::Long) => {
            pos = Some(ChunkPos::from_packed(r.read_i64_be()?));
            Ok(true)
        }
        (Some(b"type" | b"chunk_pos"), _) => Err(NbtError::Malformed),
        _ => Ok(false),
    })?;
    match (ty, pos) {
        (Some(true), Some(pos)) => Ok(Some(pos)),
        (Some(false), Some(_)) => Ok(None),
        // A ticket without a type or position cannot be classified.
        _ => Err(NbtError::Malformed),
    }
}

/// The dimension field of a player: an old numeric ID or a namespaced ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerDimension {
    Legacy(i32),
    Named(String),
}

/// Fields of a player the trust check needs. Unreadable fields are `None`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlayerSummary {
    pub pos: Option<[f64; 3]>,
    pub dimension: Option<PlayerDimension>,
    pub game_type: Option<i32>,
}

/// Reads a decompressed `<uuid>.dat` player file.
pub fn read_player(nbt: &[u8]) -> Result<PlayerSummary, NbtError> {
    let mut reader = root_compound(nbt)?;
    read_player_fields(&mut reader)
}

/// Reads `Data.Player` from decompressed `level.dat` (DataVersion < 4772).
pub fn read_level_player(nbt: &[u8]) -> Result<Option<PlayerSummary>, NbtError> {
    let mut reader = root_compound(nbt)?;
    let mut player = None;
    for_each_entry(&mut reader, |name, tag, r| {
        if name != Some(b"Data") || tag != Tag::Compound {
            return Ok(false);
        }
        for_each_entry(r, |name, tag, r| {
            if name == Some(b"Player") && tag == Tag::Compound {
                player = Some(read_player_fields(r)?);
                Ok(true)
            } else {
                Ok(false)
            }
        })?;
        Ok(true)
    })?;
    Ok(player)
}

fn read_player_fields(r: &mut SliceReader<'_>) -> Result<PlayerSummary, NbtError> {
    let mut p = PlayerSummary::default();
    for_each_entry(r, |name, tag, r| {
        match (name, tag) {
            (Some(b"Pos"), Tag::List) => {
                let element = Tag::from_u8(r.read_u8()?)?;
                let count = nanonbt::read_count(r)?;
                if element == Tag::Double && count == 3 {
                    p.pos = Some([r.read_f64_be()?, r.read_f64_be()?, r.read_f64_be()?]);
                } else {
                    // Not a position we understand: skip the elements.
                    for _ in 0..count {
                        nanonbt::skip_nbt(r, element)?;
                    }
                }
            }
            (Some(b"Dimension"), Tag::Int) => {
                p.dimension = Some(PlayerDimension::Legacy(r.read_i32_be()?))
            }
            (Some(b"Dimension"), Tag::String) => {
                let bytes = read_string_bytes(r)?;
                p.dimension = String::from_utf8(bytes).ok().map(PlayerDimension::Named);
            }
            (Some(b"playerGameType"), Tag::Int) => p.game_type = Some(r.read_i32_be()?),
            _ => return Ok(false),
        }
        Ok(true)
    })?;
    Ok(p)
}

/// Reads a string payload as raw bytes (not decoded).
fn read_string_bytes(r: &mut SliceReader<'_>) -> Result<Vec<u8>, NbtError> {
    let len = r.read_u16_be()? as usize;
    let mut bytes = vec![0; len];
    r.read_exact(&mut bytes)?;
    Ok(bytes)
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

/// Longest compound key the tool matches (`minecraft:spawn_mobs`).
const MAX_NAME: usize = 32;

/// Walks the entries of a compound. `visit` returns `true` when it consumed
/// the value; otherwise the value is skipped. Names longer than [`MAX_NAME`]
/// are passed as `None` (they match no key the tool looks for).
///
/// Key names are compared as raw bytes and never validated, so whether a
/// chunk is readable does not depend on the encoding of keys it ignores.
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
        let len = r.read_u16_be()? as usize;
        let name = if len <= MAX_NAME {
            r.read_exact(&mut buf[..len])?;
            Some(&buf[..len])
        } else {
            r.skip(len)?;
            None
        };
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
        DoubleList(Vec<f64>),
        CompoundList(Vec<Vec<(&'static str, V)>>),
        Compound(Vec<(&'static str, V)>),
    }

    fn tag(v: &V) -> u8 {
        match v {
            V::Byte(_) => 1,
            V::Int(_) => 3,
            V::Long(_) => 4,
            V::Str(_) => 8,
            V::List(_) | V::DoubleList(_) | V::CompoundList(_) => 9,
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
            V::DoubleList(items) => {
                out.push(6);
                out.extend_from_slice(&(items.len() as i32).to_be_bytes());
                for v in items {
                    out.extend_from_slice(&v.to_be_bytes());
                }
            }
            V::CompoundList(items) => {
                out.push(10);
                out.extend_from_slice(&(items.len() as i32).to_be_bytes());
                for entries in items {
                    for (k, v) in entries {
                        out.push(tag(v));
                        name(out, k);
                        payload(out, v);
                    }
                    out.push(0);
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
        crate::types::ChunkPos::new(x, z).to_packed()
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
    fn blending_data_only_counts_in_the_root_format() {
        let blending = || V::Compound(vec![("min_section", V::Int(-4))]);
        let old = read_chunk_summary(&doc(vec![
            ("DataVersion", V::Int(2800)),
            ("blending_data", blending()),
        ]));
        assert!(!old.unwrap().has_blending_data);
        let none = read_chunk_summary(&doc(vec![("blending_data", blending())]));
        assert!(!none.unwrap().has_blending_data);
    }

    #[test]
    fn duplicated_structures_keep_the_last_one() {
        let refs = |p: i64| {
            V::Compound(vec![(
                "References",
                V::Compound(vec![("a", V::LongArray(vec![p]))]),
            )])
        };
        let s = read_chunk_summary(&doc(vec![
            ("DataVersion", V::Int(4000)),
            ("structures", refs(pack(1, 1))),
            ("structures", refs(pack(2, 2))),
        ]))
        .unwrap();
        assert_eq!(s.structure_refs, BTreeSet::from([ChunkPos::new(2, 2)]));
    }

    #[test]
    fn key_encoding_is_not_validated() {
        // 0xFF is not valid modified UTF-8; an ignored key must not make the chunk unreadable.
        let mut nbt = doc(vec![("DataVersion", V::Int(4000)), ("xPos", V::Int(1))]);
        let at = nbt.windows(4).position(|w| w == b"xPos").unwrap();
        nbt[at] = 0xFF;
        assert!(read_chunk_summary(&nbt).is_ok());
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

    fn data(entries: Vec<(&'static str, V)>) -> Vec<u8> {
        doc(vec![("Data", V::Compound(entries))])
    }

    #[test]
    fn level_info_game_rule_formats() {
        let legacy = |v: &'static str| {
            data(vec![
                ("DataVersion", V::Int(4325)),
                ("GameRules", V::Compound(vec![("doMobSpawning", V::Str(v))])),
            ])
        };
        let info = read_level_info(&legacy("false")).unwrap();
        assert_eq!(info.data_version, Some(DataVersion(4325)));
        assert_eq!(info.spawn_mobs, RuleValue::Value(false));
        assert_eq!(
            read_level_info(&legacy("true")).unwrap().spawn_mobs,
            RuleValue::Value(true)
        );
        assert_eq!(
            read_level_info(&legacy("maybe")).unwrap().spawn_mobs,
            RuleValue::Unreadable
        );

        let registry = data(vec![
            ("DataVersion", V::Int(4671)),
            (
                "game_rules",
                V::Compound(vec![("minecraft:spawn_mobs", V::Byte(0))]),
            ),
        ]);
        assert_eq!(
            read_level_info(&registry).unwrap().spawn_mobs,
            RuleValue::Value(false)
        );

        let absent = data(vec![
            ("DataVersion", V::Int(4671)),
            ("game_rules", V::Compound(vec![])),
        ]);
        assert_eq!(
            read_level_info(&absent).unwrap().spawn_mobs,
            RuleValue::Absent
        );
        let mistyped = data(vec![
            ("DataVersion", V::Int(4671)),
            (
                "game_rules",
                V::Compound(vec![("minecraft:spawn_mobs", V::Str("false"))]),
            ),
        ]);
        assert_eq!(
            read_level_info(&mistyped).unwrap().spawn_mobs,
            RuleValue::Unreadable
        );
    }

    #[test]
    fn game_rules_file() {
        let file = doc(vec![
            ("DataVersion", V::Int(5023)),
            (
                "data",
                V::Compound(vec![("minecraft:spawn_mobs", V::Byte(1))]),
            ),
        ]);
        assert_eq!(read_game_rules_file(&file).unwrap(), RuleValue::Value(true));
        let empty = doc(vec![("data", V::Compound(vec![]))]);
        assert_eq!(read_game_rules_file(&empty).unwrap(), RuleValue::Absent);
    }

    #[test]
    fn forced_chunk_formats() {
        let forced = doc(vec![(
            "data",
            V::Compound(vec![("Forced", V::LongArray(vec![pack(-1, 2)]))]),
        )]);
        assert_eq!(
            read_forced_chunks(&forced).unwrap(),
            BTreeSet::from([ChunkPos::new(-1, 2)])
        );

        let ticket = |ty: &'static str, pos: V| {
            vec![
                ("type", V::Str(ty)),
                ("level", V::Int(31)),
                ("ticks_left", V::Long(0)),
                ("chunk_pos", pos),
            ]
        };
        let tickets = doc(vec![(
            "data",
            V::Compound(vec![(
                "tickets",
                V::CompoundList(vec![
                    ticket("minecraft:forced", V::Long(pack(3, -4))),
                    ticket("minecraft:forced", V::IntArray(vec![-5, 6])),
                    ticket("minecraft:portal", V::IntArray(vec![7, 7])),
                ]),
            )]),
        )]);
        assert_eq!(
            read_forced_chunks(&tickets).unwrap(),
            BTreeSet::from([ChunkPos::new(3, -4), ChunkPos::new(-5, 6)])
        );

        let bad = doc(vec![(
            "data",
            V::Compound(vec![(
                "tickets",
                V::CompoundList(vec![ticket("minecraft:forced", V::IntArray(vec![1]))]),
            )]),
        )]);
        assert_eq!(read_forced_chunks(&bad), Err(NbtError::Malformed));
        let mistyped = doc(vec![(
            "data",
            V::Compound(vec![(
                "tickets",
                V::CompoundList(vec![vec![("type", V::Int(1)), ("chunk_pos", V::Long(0))]]),
            )]),
        )]);
        assert_eq!(read_forced_chunks(&mistyped), Err(NbtError::Malformed));
        let untyped = doc(vec![(
            "data",
            V::Compound(vec![(
                "tickets",
                V::CompoundList(vec![vec![("chunk_pos", V::Long(0))]]),
            )]),
        )]);
        assert_eq!(read_forced_chunks(&untyped), Err(NbtError::Malformed));
        let empty = doc(vec![("data", V::Compound(vec![]))]);
        assert!(read_forced_chunks(&empty).unwrap().is_empty());
    }

    #[test]
    fn gunzip_round_trip() {
        use std::io::Write;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(b"level").unwrap();
        assert_eq!(gunzip(&gz.finish().unwrap()).unwrap(), b"level");
        assert_eq!(gunzip(b"not gzip"), Err(NbtError::Malformed));
    }

    #[test]
    fn player_fields() {
        let nbt = doc(vec![
            ("Dimension", V::Str("minecraft:the_nether")),
            ("playerGameType", V::Int(3)),
            ("Pos", V::DoubleList(vec![-17.5, 64.0, 32.25])),
        ]);
        let p = read_player(&nbt).unwrap();
        assert_eq!(p.pos, Some([-17.5, 64.0, 32.25]));
        assert_eq!(
            p.dimension,
            Some(PlayerDimension::Named("minecraft:the_nether".into()))
        );
        assert_eq!(p.game_type, Some(3));

        let legacy = doc(vec![(
            "Data",
            V::Compound(vec![(
                "Player",
                V::Compound(vec![("Dimension", V::Int(-1))]),
            )]),
        )]);
        let p = read_level_player(&legacy).unwrap().unwrap();
        assert_eq!(p.dimension, Some(PlayerDimension::Legacy(-1)));
        assert_eq!(p.pos, None);
        assert_eq!(
            read_level_player(&doc(vec![("Data", V::Compound(vec![]))])).unwrap(),
            None
        );
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
