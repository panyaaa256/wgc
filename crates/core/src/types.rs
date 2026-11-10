//! Basic types shared by every module (docs/spec/types.md).

use std::fmt;
use std::str::FromStr;

/// Region coordinates (`r.<x>.<z>.mca`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RegionPos {
    pub x: i32,
    pub z: i32,
}

impl RegionPos {
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }
}

/// Absolute chunk coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChunkPos {
    pub x: i32,
    pub z: i32,
}

impl ChunkPos {
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    /// Unpacks the game's `long` chunk position: x in the low 32 bits, z in the high 32 bits.
    pub const fn from_packed(v: i64) -> Self {
        Self {
            x: v as i32,
            z: (v >> 32) as i32,
        }
    }

    /// Packs into the game's `long` form; inverse of [`ChunkPos::from_packed`].
    #[cfg(test)]
    pub(crate) const fn to_packed(self) -> i64 {
        (self.x as u32 as i64) | ((self.z as i64) << 32)
    }

    /// The region that contains this chunk.
    pub const fn region(self) -> RegionPos {
        RegionPos {
            x: self.x >> 5,
            z: self.z >> 5,
        }
    }

    /// Index of this chunk in its region file.
    pub const fn index(self) -> ChunkIndex {
        ChunkIndex(((self.x & 31) + (self.z & 31) * 32) as u16)
    }
}

/// Index of a chunk in a region file: `0..1024`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChunkIndex(u16);

impl ChunkIndex {
    /// Number of chunk slots in a region file.
    pub const COUNT: u16 = 1024;

    /// Returns `None` when `i` is not below 1024.
    pub const fn new(i: u16) -> Option<Self> {
        if i < Self::COUNT { Some(Self(i)) } else { None }
    }

    pub const fn get(self) -> u16 {
        self.0
    }

    /// Absolute position of this slot in `region`.
    pub const fn to_chunk_pos(self, region: RegionPos) -> ChunkPos {
        ChunkPos {
            x: region.x * 32 + (self.0 & 31) as i32,
            z: region.z * 32 + (self.0 >> 5) as i32,
        }
    }

    /// All 1024 indices in ascending order.
    pub fn all() -> impl Iterator<Item = ChunkIndex> {
        (0..Self::COUNT).map(ChunkIndex)
    }
}

/// A Minecraft data version (`DataVersion`). Constants live in [`crate::versions`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DataVersion(pub i32);

/// A dimension ID such as `minecraft:the_nether`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DimensionId(String);

/// Why a string is not a valid dimension ID.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid dimension id {0:?}")]
pub struct DimensionIdError(pub String);

impl DimensionId {
    /// Parses `<namespace>:<path>`. A bare `overworld`, `the_nether` or `the_end`
    /// gets the `minecraft:` namespace.
    ///
    /// Namespaces may contain `[a-z0-9_.-]`, paths additionally `/`.
    pub fn parse(s: &str) -> Result<Self, DimensionIdError> {
        let full = match s {
            "overworld" | "the_nether" | "the_end" => format!("minecraft:{s}"),
            _ => s.to_owned(),
        };
        let Some((namespace, path)) = full.split_once(':') else {
            return Err(DimensionIdError(s.to_owned()));
        };
        let ns_ok = |c: char| matches!(c, 'a'..='z' | '0'..='9' | '_' | '.' | '-');
        let path_ok = |c: char| ns_ok(c) || c == '/';
        if namespace.is_empty()
            || path.is_empty()
            || !namespace.chars().all(ns_ok)
            || !path.chars().all(path_ok)
        {
            return Err(DimensionIdError(s.to_owned()));
        }
        Ok(Self(full))
    }

    pub fn overworld() -> Self {
        Self("minecraft:overworld".to_owned())
    }

    pub fn the_nether() -> Self {
        Self("minecraft:the_nether".to_owned())
    }

    pub fn the_end() -> Self {
        Self("minecraft:the_end".to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DimensionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for DimensionId {
    type Err = DimensionIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// The three files of a region triple.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FileKind {
    Region,
    Entities,
    Poi,
}

impl FileKind {
    pub const ALL: [FileKind; 3] = [FileKind::Region, FileKind::Entities, FileKind::Poi];

    /// Folder name inside a dimension folder.
    pub const fn dir_name(self) -> &'static str {
        match self {
            FileKind::Region => "region",
            FileKind::Entities => "entities",
            FileKind::Poi => "poi",
        }
    }
}

/// A stop that `--force` can continue past: the trust checks and `session.lock` missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GuardId {
    T1,
    T2,
    T3,
    T4,
    T5,
    L1,
}

/// Why a string is not a known [`GuardId`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown check id {0:?}")]
pub struct GuardIdError(pub String);

impl GuardId {
    pub const ALL: [GuardId; 6] = [
        GuardId::T1,
        GuardId::T2,
        GuardId::T3,
        GuardId::T4,
        GuardId::T5,
        GuardId::L1,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            GuardId::T1 => "T-1",
            GuardId::T2 => "T-2",
            GuardId::T3 => "T-3",
            GuardId::T4 => "T-4",
            GuardId::T5 => "T-5",
            GuardId::L1 => "L-1",
        }
    }
}

impl fmt::Display for GuardId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for GuardId {
    type Err = GuardIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        GuardId::ALL
            .into_iter()
            .find(|id| id.as_str() == s)
            .ok_or_else(|| GuardIdError(s.to_owned()))
    }
}

/// A value that was read, or why it could not be confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Known<T> {
    Value(T),
    Unknown(UnknownReason),
}

impl<T> From<Result<T, UnknownReason>> for Known<T> {
    fn from(result: Result<T, UnknownReason>) -> Self {
        match result {
            Ok(v) => Known::Value(v),
            Err(reason) => Known::Unknown(reason),
        }
    }
}

/// Why something could not be confirmed (docs/spec/trust.md).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum UnknownReason {
    UnreadableFile {
        path: std::path::PathBuf,
    },
    UnexpectedValue {
        path: std::path::PathBuf,
        key: String,
    },
    UnknownVersion(DataVersion),
    NoPlayersChecked,
    ForcedChunksUnknown(std::collections::BTreeSet<DimensionId>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_packed_restores_negative_coordinates() {
        let pack = |x: i32, z: i32| ChunkPos::new(x, z).to_packed();
        for (x, z) in [
            (0, 0),
            (-1, 2),
            (3, -4),
            (i32::MIN, i32::MAX),
            (-30_000_000, 30_000_000),
        ] {
            assert_eq!(ChunkPos::from_packed(pack(x, z)), ChunkPos::new(x, z));
        }
    }

    #[test]
    fn chunk_index_range() {
        assert!(ChunkIndex::new(1023).is_some());
        assert_eq!(ChunkIndex::new(1024), None);
    }

    #[test]
    fn index_and_to_chunk_pos_round_trip() {
        for pos in [
            ChunkPos::new(0, 0),
            ChunkPos::new(-1, -1),
            ChunkPos::new(33, -65),
            ChunkPos::new(-32, 31),
        ] {
            assert_eq!(pos.index().to_chunk_pos(pos.region()), pos);
        }
        assert_eq!(ChunkPos::new(-1, -1).region(), RegionPos::new(-1, -1));
        assert_eq!(ChunkPos::new(-1, -1).index().get(), 1023);
    }

    #[test]
    fn dimension_id_parse() {
        assert_eq!(
            DimensionId::parse("the_end").unwrap(),
            DimensionId::the_end()
        );
        assert_eq!(
            DimensionId::parse("minecraft:overworld").unwrap(),
            DimensionId::overworld()
        );
        assert_eq!(
            DimensionId::parse("my_pack:sky/islands").unwrap().as_str(),
            "my_pack:sky/islands"
        );
        for bad in ["", "nether", ":x", "x:", "Upper:case", "a:b:c", "a b:c"] {
            assert!(DimensionId::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn guard_id_text() {
        assert_eq!(GuardId::T1.to_string(), "T-1");
        assert_eq!("L-1".parse::<GuardId>(), Ok(GuardId::L1));
        assert!("T1".parse::<GuardId>().is_err());
    }
}
