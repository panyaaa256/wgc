//! The recognized layout of a world, and the files it points at: game rules,
//! forced chunks, player data and Paper configuration.
//!
//! Recognizing a layout needs a file system and happens elsewhere; this module
//! holds the result. `P` names files and folders.

use alloc::borrow::ToOwned;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;

use crate::nbt::{self, NbtError, RuleValue};
use crate::source::WorldSource;
use crate::types::{ChunkPos, DataVersion, DimensionId, FileKind, Known, RegionPos, UnknownReason};
use crate::versions::{DV_FILES_26_1, DV_MAX_KNOWN};

/// File name of the lock in each world folder. The game holds it while the
/// world is open; it is never copied to the output.
pub const LOCK_FILE: &str = "session.lock";

/// The four layouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LayoutKind {
    /// `region/`, `DIM-1/`, `DIM1/` in the world folder.
    VanillaLegacy,
    /// 26.1 and later: `dimensions/<ns>/<name>/`.
    Vanilla,
    /// Bukkit before 26.1: `<base>/`, `<base>_nether/DIM-1/`, `<base>_the_end/DIM1/`.
    BukkitLegacy,
    /// Bukkit 26.1 and later: `<base>/dimensions/<ns>/<name>/`.
    Bukkit,
}

impl LayoutKind {
    pub const fn is_bukkit(self) -> bool {
        matches!(self, LayoutKind::Bukkit | LayoutKind::BukkitLegacy)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            LayoutKind::VanillaLegacy => "vanilla_legacy",
            LayoutKind::Vanilla => "vanilla",
            LayoutKind::BukkitLegacy => "bukkit_legacy",
            LayoutKind::Bukkit => "bukkit",
        }
    }
}

/// One dimension of the world.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Dimension<P> {
    pub id: DimensionId,
    /// The folder that holds `region/`, `entities/` and `poi/`.
    pub dir: P,
    /// The world folder this dimension belongs to (where `session.lock` is).
    pub world_folder: P,
}

/// A recognized world. Every later step finds the world's files through this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldLayout<P> {
    /// The input path, canonicalized.
    pub root: P,
    pub kind: LayoutKind,
    /// Only for Bukkit layouts.
    pub base: Option<String>,
    /// Where `level.dat` and player data are.
    pub main_world_folder: P,
    /// Units of locking and output, sorted.
    pub world_folders: Vec<P>,
    /// Only for Bukkit layouts: the root.
    pub server_root: Option<P>,
    /// `Data.DataVersion` of the main `level.dat`.
    pub data_version: Option<DataVersion>,
    /// The mob spawning rule stored in the main `level.dat` (before 4772).
    pub level_spawn_mobs: RuleValue,
    /// Sorted by ID.
    pub dimensions: Vec<Dimension<P>>,
}

/// The files of one region triple. Each kind may be missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TripleFiles<P> {
    pub pos: RegionPos,
    pub region: Option<P>,
    pub entities: Option<P>,
    pub poi: Option<P>,
}

impl<P> TripleFiles<P> {
    pub const fn new(pos: RegionPos) -> Self {
        Self {
            pos,
            region: None,
            entities: None,
            poi: None,
        }
    }

    pub const fn get(&self, kind: FileKind) -> Option<&P> {
        match kind {
            FileKind::Region => self.region.as_ref(),
            FileKind::Entities => self.entities.as_ref(),
            FileKind::Poi => self.poi.as_ref(),
        }
    }

    pub const fn slot(&mut self, kind: FileKind) -> &mut Option<P> {
        match kind {
            FileKind::Region => &mut self.region,
            FileKind::Entities => &mut self.entities,
            FileKind::Poi => &mut self.poi,
        }
    }
}

/// The region triples and external chunk files of one dimension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DimensionFiles<P> {
    /// Sorted by position.
    pub triples: Vec<TripleFiles<P>>,
    pub region_externals: BTreeMap<ChunkPos, P>,
    pub entities_externals: BTreeMap<ChunkPos, P>,
    pub poi_externals: BTreeMap<ChunkPos, P>,
}

impl<P> Default for DimensionFiles<P> {
    fn default() -> Self {
        Self {
            triples: Vec::new(),
            region_externals: BTreeMap::new(),
            entities_externals: BTreeMap::new(),
            poi_externals: BTreeMap::new(),
        }
    }
}

impl<P> DimensionFiles<P> {
    /// External `.mcc` files of `kind`.
    pub const fn externals(&self, kind: FileKind) -> &BTreeMap<ChunkPos, P> {
        match kind {
            FileKind::Region => &self.region_externals,
            FileKind::Entities => &self.entities_externals,
            FileKind::Poi => &self.poi_externals,
        }
    }

    pub const fn externals_mut(&mut self, kind: FileKind) -> &mut BTreeMap<ChunkPos, P> {
        match kind {
            FileKind::Region => &mut self.region_externals,
            FileKind::Entities => &mut self.entities_externals,
            FileKind::Poi => &mut self.poi_externals,
        }
    }

    /// Chunks with an external file of `kind`, grouped by region.
    pub fn externals_by_region(&self, kind: FileKind) -> BTreeMap<RegionPos, BTreeSet<ChunkPos>> {
        let mut map: BTreeMap<RegionPos, BTreeSet<ChunkPos>> = BTreeMap::new();
        for pos in self.externals(kind).keys() {
            map.entry(pos.region()).or_default().insert(*pos);
        }
        map
    }
}

impl<P> WorldLayout<P> {
    fn known_version(&self) -> Result<(), UnknownReason<P>> {
        match self.data_version {
            Some(v) if v > DV_MAX_KNOWN => Err(UnknownReason::UnknownVersion(v)),
            _ => Ok(()),
        }
    }

    /// Whether the world uses the file locations of 26.1 and later.
    pub fn has_26_1_files(&self) -> bool {
        self.data_version.is_some_and(|v| v >= DV_FILES_26_1)
    }
}

/// Reads a gzip NBT file; `Ok(None)` if it does not exist.
fn read_optional<S: WorldSource, T>(
    source: &S,
    path: &S::Path,
    parse: impl FnOnce(&[u8]) -> Result<T, NbtError>,
) -> Result<Option<T>, UnknownReason<S::Path>> {
    let unreadable = || UnknownReason::UnreadableFile { path: path.clone() };
    match source.read_optional(path) {
        Ok(Some(bytes)) => nbt::gunzip(&bytes)
            .and_then(|nbt| parse(&nbt))
            .map(Some)
            .map_err(|_| unreadable()),
        Ok(None) => Ok(None),
        Err(_) => Err(unreadable()),
    }
}

/// Whether mob spawning is enabled for `dim` (game rule `spawn_mobs`, formerly `doMobSpawning`).
///
/// A rule that is not stored has its default value, enabled.
pub fn spawn_mobs<S: WorldSource>(
    source: &S,
    layout: &WorldLayout<S::Path>,
    dim: &Dimension<S::Path>,
) -> Known<bool, S::Path> {
    let game_rules = |folder: &S::Path| -> Result<_, UnknownReason<S::Path>> {
        let path = source.join(folder, "data/minecraft/game_rules.dat");
        let value = read_optional(source, &path, nbt::read_game_rules_file)?;
        Ok((path, value.unwrap_or(RuleValue::Absent)))
    };
    let main_level_rule = || {
        (
            source.join(&layout.main_world_folder, "level.dat"),
            layout.level_spawn_mobs,
        )
    };
    Known::from((|| {
        layout.known_version()?;
        let (path, value) = match layout.kind {
            LayoutKind::Bukkit => game_rules(&dim.dir)?,
            LayoutKind::BukkitLegacy if dim.world_folder != layout.main_world_folder => {
                let path = source.join(&dim.world_folder, "level.dat");
                match read_optional(source, &path, nbt::read_level_info)? {
                    Some(info) => (path, info.spawn_mobs),
                    None => return Err(UnknownReason::UnreadableFile { path }),
                }
            }
            LayoutKind::BukkitLegacy => main_level_rule(),
            LayoutKind::Vanilla | LayoutKind::VanillaLegacy if layout.has_26_1_files() => {
                game_rules(&layout.main_world_folder)?
            }
            LayoutKind::Vanilla | LayoutKind::VanillaLegacy => main_level_rule(),
        };
        match value {
            RuleValue::Absent => Ok(true),
            RuleValue::Value(v) => Ok(v),
            RuleValue::Unreadable => Err(UnknownReason::UnexpectedValue {
                path,
                key: "spawn_mobs".to_owned(),
            }),
        }
    })())
}

/// The `/forceload` chunks of `dim`.
pub fn forced_chunks<S: WorldSource>(
    source: &S,
    layout: &WorldLayout<S::Path>,
    dim: &Dimension<S::Path>,
) -> Known<BTreeSet<ChunkPos>, S::Path> {
    Known::from((|| {
        layout.known_version()?;
        let new = source.join(&dim.dir, "data/minecraft/chunk_tickets.dat");
        let old = source.join(&dim.dir, "data/chunks.dat");
        let candidates = if layout.has_26_1_files() {
            [new, old]
        } else {
            [old, new]
        };
        for path in candidates {
            if let Some(chunks) = read_optional(source, &path, nbt::read_forced_chunks)? {
                return Ok(chunks);
            }
        }
        Ok(BTreeSet::new())
    })())
}

/// The folder that holds `<uuid>.dat` player files.
pub fn player_data_dir<S: WorldSource>(source: &S, layout: &WorldLayout<S::Path>) -> S::Path {
    let relative = if layout.has_26_1_files() {
        "players/data"
    } else {
        "playerdata"
    };
    source.join(&layout.main_world_folder, relative)
}

/// `paper-world.yml` for `dim` (Bukkit layouts only).
pub fn paper_world_config<S: WorldSource>(
    source: &S,
    layout: &WorldLayout<S::Path>,
    dim: &Dimension<S::Path>,
) -> Option<S::Path> {
    match layout.kind {
        LayoutKind::BukkitLegacy => Some(source.join(&dim.world_folder, "paper-world.yml")),
        LayoutKind::Bukkit => Some(source.join(&dim.dir, "paper-world.yml")),
        LayoutKind::Vanilla | LayoutKind::VanillaLegacy => None,
    }
}

/// `config/paper-world-defaults.yml` of the server (Bukkit layouts only).
pub fn paper_defaults_config<S: WorldSource>(
    source: &S,
    layout: &WorldLayout<S::Path>,
) -> Option<S::Path> {
    layout
        .server_root
        .as_ref()
        .map(|root| source.join(root, "config/paper-world-defaults.yml"))
}
