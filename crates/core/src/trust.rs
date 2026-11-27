//! Checking that InhabitedTime can be trusted.
//!
//! GC assumes that a chunk whose InhabitedTime is 0 has never had a
//! non-spectator player within 128 blocks: vanilla adds to it every tick while
//! a player is that close. Each check looks for a reason the assumption may
//! not hold, and only in the dimensions that GC targets.
//!
//! Files that cannot be read never fail the check run; they make the check
//! "unknown" instead.

use alloc::borrow::ToOwned;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;

use crate::layout::{self, Dimension, LayoutKind, WorldLayout};
use crate::nbt::{self, PlayerDimension, PlayerSummary};
use crate::plan::{Inhabited, Plan, forceload_protection, is_targeted};
use crate::source::{Config, WorldSource};
use crate::types::{ChunkPos, DimensionId, GuardId, Known, UnknownReason};
use crate::versions::{DV_1_21_5, DV_FILES_26_1, DV_MAX_KNOWN};

/// Results of the trust checks, in ID order T-1..T-5 (T-3 is absent when
/// [`check_trust_before_plan`] was told to skip it). Each check is described
/// on its [`GuardId`] variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustReport<P> {
    pub checks: Vec<CheckResult<P>>,
}

impl<P> TrustReport<P> {
    /// Checks that failed or could not be confirmed: these stop `gc --out`.
    pub fn stops(&self) -> BTreeSet<GuardId> {
        self.checks
            .iter()
            .filter(|c| matches!(c.status, CheckStatus::Fail(_) | CheckStatus::Unknown(_)))
            .map(|c| c.id)
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckResult<P> {
    pub id: GuardId,
    pub status: CheckStatus<P>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckStatus<P> {
    Pass,
    Fail(Vec<TrustDetail<P>>),
    Unknown(UnknownReason<P>),
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum TrustDetail<P> {
    PaperFixedInhabitedTime {
        file: P,
        value: i64,
    },
    /// `world_folder` is the folder whose game rules disable spawning (a
    /// dimension folder on Bukkit 26.1 and later).
    MobSpawningDisabled {
        world_folder: P,
    },
    PlayerInZeroChunk {
        uuid: String,
        dimension: DimensionId,
        pos: ChunkPos,
    },
}

/// Runs T-1..T-5 against a plan.
pub fn check_trust<S: WorldSource>(source: &S, plan: &Plan<S::Path>) -> TrustReport<S::Path> {
    run_checks(
        source,
        &plan.layout,
        plan.gc_dims.as_ref(),
        &plan.protection_unknown,
        Some(|wanted: &BTreeSet<ChunkKey>| Ok(inhabited_in_plan(plan, wanted))),
    )
}

/// Runs T-1..T-5 without a plan, so that `gc --out` can stop before reading
/// every region file. T-5 reads the forced chunks directly, and T-3 looks up
/// the chunks players stand in through `inhabited_of`, which returns the
/// InhabitedTime of the chunks that exist.
///
/// Without `inhabited_of`, T-3 reads nothing and is left out of the report:
/// when `--force` allows it, it cannot stop the run, and the check against the
/// plan decides whether it was forced.
pub fn check_trust_before_plan<S: WorldSource>(
    source: &S,
    layout: &WorldLayout<S::Path>,
    gc_dims: Option<&BTreeSet<DimensionId>>,
    inhabited_of: Option<
        impl FnOnce(&BTreeSet<ChunkKey>) -> Result<InhabitedMap, UnknownReason<S::Path>>,
    >,
) -> TrustReport<S::Path> {
    let protection_unknown: BTreeSet<DimensionId> = layout
        .dimensions
        .iter()
        .filter(|d| {
            let protection = forceload_protection(source, layout, d, is_targeted(gc_dims, &d.id));
            matches!(protection, Known::Unknown(_))
        })
        .map(|d| d.id.clone())
        .collect();
    run_checks(source, layout, gc_dims, &protection_unknown, inhabited_of)
}

/// A chunk in a dimension.
pub type ChunkKey = (DimensionId, ChunkPos);

/// The InhabitedTime of the chunks T-3 looks up; chunks that do not exist are left out.
pub type InhabitedMap = BTreeMap<ChunkKey, Inhabited>;

fn run_checks<S: WorldSource>(
    source: &S,
    layout: &WorldLayout<S::Path>,
    gc_dims: Option<&BTreeSet<DimensionId>>,
    protection_unknown: &BTreeSet<DimensionId>,
    inhabited_of: Option<
        impl FnOnce(&BTreeSet<ChunkKey>) -> Result<InhabitedMap, UnknownReason<S::Path>>,
    >,
) -> TrustReport<S::Path> {
    let targeted: Vec<&Dimension<S::Path>> = layout
        .dimensions
        .iter()
        .filter(|d| is_targeted(gc_dims, &d.id))
        .collect();
    let t3 = inhabited_of.map(|inhabited_of| CheckResult {
        id: GuardId::T3,
        status: player_positions(source, layout, gc_dims, inhabited_of),
    });
    let checks = [
        Some(CheckResult {
            id: GuardId::T1,
            status: paper_config(source, layout, &targeted),
        }),
        Some(CheckResult {
            id: GuardId::T2,
            status: mob_spawning(source, layout, &targeted),
        }),
        t3,
        Some(CheckResult {
            id: GuardId::T4,
            status: version(layout),
        }),
        Some(CheckResult {
            id: GuardId::T5,
            status: forced_chunks(gc_dims, protection_unknown),
        }),
    ];
    TrustReport {
        checks: checks.into_iter().flatten().collect(),
    }
}

fn from_details<P>(details: Vec<TrustDetail<P>>) -> CheckStatus<P> {
    if details.is_empty() {
        CheckStatus::Pass
    } else {
        CheckStatus::Fail(details)
    }
}

// ---- T-1 -----------------------------------------------------------------

/// `fixed-chunk-inhabited-time` as found in one place.
#[derive(Clone, Copy)]
enum FixedTime {
    /// Not set here, or `default`: the next place decides.
    Inherit,
    /// A negative value: the feature is off.
    Off,
    On(i64),
}

/// Reads a configuration file; `Ok(None)` if it does not exist.
fn load_config<S: WorldSource>(
    source: &S,
    path: &S::Path,
) -> Result<Option<Config>, UnknownReason<S::Path>> {
    source
        .read_config(path)
        .map_err(|_| UnknownReason::UnreadableFile { path: path.clone() })
}

/// Looks up `keys` under `doc`. A missing key is `Ok(None)`; a node on the way
/// that is neither a mapping nor null is an unexpected value.
fn lookup<'a, P: Clone>(
    doc: &'a Config,
    keys: &[&str],
    path: &P,
) -> Result<Option<&'a Config>, UnknownReason<P>> {
    let mut node = doc;
    for key in keys {
        match node {
            Config::Map(map) => match map.get(*key) {
                Some(next) => node = next,
                None => return Ok(None),
            },
            Config::Null => return Ok(None),
            _ => {
                return Err(UnknownReason::UnexpectedValue {
                    path: path.clone(),
                    key: keys.join("."),
                });
            }
        }
    }
    Ok(Some(node))
}

fn fixed_time<P: Clone>(
    value: Option<&Config>,
    keys: &[&str],
    path: &P,
) -> Result<FixedTime, UnknownReason<P>> {
    match value {
        None | Some(Config::Null) => Ok(FixedTime::Inherit),
        Some(Config::String(s)) if s == "default" => Ok(FixedTime::Inherit),
        Some(Config::Integer(v)) if *v >= 0 => Ok(FixedTime::On(*v)),
        Some(Config::Integer(_)) => Ok(FixedTime::Off),
        Some(_) => Err(UnknownReason::UnexpectedValue {
            path: path.clone(),
            key: keys.join("."),
        }),
    }
}

/// The setting in a Paper 1.19+ file (`paper-world.yml`, `paper-world-defaults.yml`).
fn read_modern<S: WorldSource>(
    source: &S,
    path: &S::Path,
) -> Result<FixedTime, UnknownReason<S::Path>> {
    const KEYS: [&str; 2] = ["chunks", "fixed-chunk-inhabited-time"];
    match load_config(source, path)? {
        None => Ok(FixedTime::Inherit),
        Some(doc) => fixed_time(lookup(&doc, &KEYS, path)?, &KEYS, path),
    }
}

/// T-1, for Bukkit layouts: fails when `fixed-chunk-inhabited-time` is 0 or
/// more for a world that holds a targeted dimension, because Paper then pins
/// every chunk's InhabitedTime to that value. A per-world value overrides the
/// server default; `default`, a missing key or a missing file inherit it; a
/// negative value turns the feature off.
fn paper_config<S: WorldSource>(
    source: &S,
    layout: &WorldLayout<S::Path>,
    targeted: &[&Dimension<S::Path>],
) -> CheckStatus<S::Path> {
    let Some(server_root) = layout
        .server_root
        .as_ref()
        .filter(|_| layout.kind.is_bukkit())
    else {
        return CheckStatus::NotApplicable;
    };
    let result = (|| {
        let defaults = source.join(server_root, "config/paper-world-defaults.yml");
        let legacy = source.join(server_root, "paper.yml");
        let mut details = BTreeSet::new();
        // Paper 1.19 moved paper.yml to config/; paper.yml alone means an older Paper.
        if !source.exists(&defaults) && source.exists(&legacy) {
            let doc = load_config(source, &legacy)?.unwrap_or(Config::Null);
            let folders: BTreeSet<&S::Path> = targeted.iter().map(|d| &d.world_folder).collect();
            for folder in folders {
                let world = source.name(folder).unwrap_or_default();
                let own = ["world-settings", world, "fixed-chunk-inhabited-time"];
                let default = ["world-settings", "default", "fixed-chunk-inhabited-time"];
                let time = match fixed_time(lookup(&doc, &own, &legacy)?, &own, &legacy)? {
                    FixedTime::Inherit => {
                        fixed_time(lookup(&doc, &default, &legacy)?, &default, &legacy)?
                    }
                    other => other,
                };
                if let FixedTime::On(value) = time {
                    details.insert(TrustDetail::PaperFixedInhabitedTime {
                        file: legacy.clone(),
                        value,
                    });
                }
            }
            return Ok(details);
        }

        let default_time = read_modern(source, &defaults)?;
        let world_files: BTreeSet<S::Path> = targeted
            .iter()
            .filter_map(|d| layout::paper_world_config(source, layout, d))
            .collect();
        for file in &world_files {
            let found = match read_modern(source, file)? {
                FixedTime::Inherit => match default_time {
                    FixedTime::On(value) => Some((value, defaults.clone())),
                    _ => None,
                },
                FixedTime::On(value) => Some((value, file.clone())),
                FixedTime::Off => None,
            };
            if let Some((value, file)) = found {
                details.insert(TrustDetail::PaperFixedInhabitedTime { file, value });
            }
        }
        Ok(details)
    })();
    match result {
        Ok(details) => from_details(details.into_iter().collect()),
        Err(reason) => CheckStatus::Unknown(reason),
    }
}

// ---- T-2 -----------------------------------------------------------------

/// T-2, for Bukkit layouts from 1.21.5 on: fails when the mob spawning game
/// rule is off for a world that holds a targeted dimension, because Paper then
/// stops advancing InhabitedTime (PaperMC/Paper#12827). Other Bukkit servers
/// are not affected, but the files cannot tell them apart.
fn mob_spawning<S: WorldSource>(
    source: &S,
    layout: &WorldLayout<S::Path>,
    targeted: &[&Dimension<S::Path>],
) -> CheckStatus<S::Path> {
    let affected = layout.data_version.is_some_and(|v| v >= DV_1_21_5);
    if !layout.kind.is_bukkit() || !affected {
        return CheckStatus::NotApplicable;
    }
    let mut details = BTreeSet::new();
    let mut unknown = None;
    for dim in targeted {
        match layout::spawn_mobs(source, layout, dim) {
            Known::Value(true) => {}
            Known::Value(false) => {
                let folder = match layout.kind {
                    LayoutKind::Bukkit => dim.dir.clone(),
                    _ => dim.world_folder.clone(),
                };
                details.insert(TrustDetail::MobSpawningDisabled {
                    world_folder: folder,
                });
            }
            Known::Unknown(reason) => {
                unknown.get_or_insert(reason);
            }
        }
    }
    // A failure is more useful to report than an unknown; both stop `gc --out`.
    match (details.is_empty(), unknown) {
        (false, _) => CheckStatus::Fail(details.into_iter().collect()),
        (true, Some(reason)) => CheckStatus::Unknown(reason),
        (true, None) => CheckStatus::Pass,
    }
}

// ---- T-3 -----------------------------------------------------------------

const SPECTATOR: i32 = 3;

/// Reads every player: the player data folder and, before 26.1, the
/// singleplayer host in `level.dat`. Unreadable sources are an error.
fn read_players<S: WorldSource>(
    source: &S,
    layout: &WorldLayout<S::Path>,
) -> Result<Vec<(String, PlayerSummary)>, UnknownReason<S::Path>> {
    let unreadable = |path: &S::Path| UnknownReason::UnreadableFile { path: path.clone() };
    let mut players = Vec::new();
    let dir = layout::player_data_dir(source, layout);
    let mut files: Vec<S::Path> = source
        .list(&dir)
        .map_err(|_| unreadable(&dir))?
        .into_iter()
        .map(|entry| entry.path)
        .collect();
    files.sort();
    for path in files {
        let Some(stem) = source
            .name(&path)
            .and_then(|n| n.strip_suffix(".dat"))
            .filter(|stem| is_uuid(stem))
        else {
            continue;
        };
        let player = read_gzip(source, &path, nbt::read_player).ok_or_else(|| unreadable(&path))?;
        players.push((stem.to_owned(), player));
    }

    if layout.data_version.is_none_or(|v| v < DV_FILES_26_1) {
        let level = source.join(&layout.main_world_folder, "level.dat");
        let host =
            read_gzip(source, &level, nbt::read_level_player).ok_or_else(|| unreadable(&level))?;
        // The host usually also has a player file; do not count them twice.
        if let Some(host) = host
            && !players
                .iter()
                .any(|(_, p)| p.pos == host.pos && p.dimension == host.dimension)
        {
            players.push(("level.dat".to_owned(), host));
        }
    }
    Ok(players)
}

/// The dimension and chunk a player stands in, if it can be checked.
fn player_chunk(player: &PlayerSummary) -> Option<ChunkKey> {
    if player.game_type == Some(SPECTATOR) {
        return None;
    }
    let id = match player.dimension.as_ref()? {
        PlayerDimension::Legacy(0) => DimensionId::overworld(),
        PlayerDimension::Legacy(-1) => DimensionId::the_nether(),
        PlayerDimension::Legacy(1) => DimensionId::the_end(),
        PlayerDimension::Legacy(_) => return None,
        PlayerDimension::Named(name) => DimensionId::parse(name).ok()?,
    };
    let pos = player.pos?;
    Some((id, block_to_chunk(pos[0], pos[2])?))
}

/// T-3: fails when a non-spectator player in a targeted dimension stands in a
/// chunk whose InhabitedTime is 0, which the game would have advanced.
/// Players whose position cannot be read, whose chunk does not exist, or
/// whose chunk has no plain InhabitedTime value are skipped; if no player is
/// left to check, the result is unknown.
fn player_positions<S: WorldSource>(
    source: &S,
    layout: &WorldLayout<S::Path>,
    gc_dims: Option<&BTreeSet<DimensionId>>,
    inhabited_of: impl FnOnce(&BTreeSet<ChunkKey>) -> Result<InhabitedMap, UnknownReason<S::Path>>,
) -> CheckStatus<S::Path> {
    let players = match read_players(source, layout) {
        Ok(players) => players,
        Err(reason) => return CheckStatus::Unknown(reason),
    };
    let wanted: Vec<(String, ChunkKey)> = players
        .into_iter()
        .filter_map(|(uuid, p)| player_chunk(&p).map(|key| (uuid, key)))
        .filter(|(_, (id, _))| is_targeted(gc_dims, id))
        .collect();
    let keys: BTreeSet<ChunkKey> = wanted.iter().map(|(_, key)| key.clone()).collect();
    let inhabited = match inhabited_of(&keys) {
        Ok(inhabited) => inhabited,
        Err(reason) => return CheckStatus::Unknown(reason),
    };

    let mut checked = 0;
    let mut details = Vec::new();
    for (uuid, key) in wanted {
        match inhabited.get(&key) {
            Some(Inhabited::Value(0)) => {
                checked += 1;
                let (dimension, pos) = key;
                details.push(TrustDetail::PlayerInZeroChunk {
                    uuid,
                    dimension,
                    pos,
                });
            }
            Some(Inhabited::Value(_)) => checked += 1,
            // Missing chunk or unknown InhabitedTime.
            _ => {}
        }
    }
    if checked == 0 {
        return CheckStatus::Unknown(UnknownReason::NoPlayersChecked);
    }
    from_details(details)
}

fn inhabited_in_plan<P>(plan: &Plan<P>, wanted: &BTreeSet<ChunkKey>) -> InhabitedMap {
    let mut found = BTreeMap::new();
    for dim in &plan.dimensions {
        for t in &dim.triples {
            for c in &t.chunks {
                let key = (dim.dimension.id.clone(), c.pos);
                if wanted.contains(&key) {
                    found.insert(key, c.inhabited.clone());
                }
            }
        }
    }
    found
}

/// The chunk that holds a block coordinate, if it is a finite value inside
/// the range of chunk coordinates.
fn block_to_chunk(x: f64, z: f64) -> Option<ChunkPos> {
    Some(ChunkPos::new(
        floor_to_i32(x / 16.0)?,
        floor_to_i32(z / 16.0)?,
    ))
}

/// `v` rounded towards negative infinity, if that fits an `i32`.
fn floor_to_i32(v: f64) -> Option<i32> {
    // floor(v) is in i32's range exactly when v is in [MIN, MAX + 1); NaN fails both.
    if !(v >= f64::from(i32::MIN) && v < f64::from(i32::MAX) + 1.0) {
        return None;
    }
    // In range, `as` truncates towards zero; negative values with a
    // fractional part need one less.
    let truncated = v as i64;
    let floor = if (truncated as f64) > v {
        truncated - 1
    } else {
        truncated
    };
    i32::try_from(floor).ok()
}

fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

fn read_gzip<S: WorldSource, T>(
    source: &S,
    path: &S::Path,
    parse: impl FnOnce(&[u8]) -> Result<T, nbt::NbtError>,
) -> Option<T> {
    let bytes = source.read(path).ok()?;
    let nbt = nbt::gunzip(&bytes).ok()?;
    parse(&nbt).ok()
}

// ---- T-4, T-5 --------------------------------------------------------------

/// T-4: unknown when the world's DataVersion is newer than any version wgc
/// knows, since its InhabitedTime behavior has not been checked.
fn version<P>(layout: &WorldLayout<P>) -> CheckStatus<P> {
    match layout.data_version {
        Some(v) if v > DV_MAX_KNOWN => CheckStatus::Unknown(UnknownReason::UnknownVersion(v)),
        _ => CheckStatus::Pass,
    }
}

/// T-5: unknown when the forced chunks of a targeted dimension could not be
/// read, so chunks around them cannot be protected from deletion.
fn forced_chunks<P>(
    gc_dims: Option<&BTreeSet<DimensionId>>,
    unknown: &BTreeSet<DimensionId>,
) -> CheckStatus<P> {
    let unknown: BTreeSet<DimensionId> = unknown
        .iter()
        .filter(|id| is_targeted(gc_dims, id))
        .cloned()
        .collect();
    if unknown.is_empty() {
        CheckStatus::Pass
    } else {
        CheckStatus::Unknown(UnknownReason::ForcedChunksUnknown(unknown))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_to_chunk_floors() {
        assert_eq!(block_to_chunk(-0.5, 15.9), Some(ChunkPos::new(-1, 0)));
        assert_eq!(block_to_chunk(16.0, -16.0), Some(ChunkPos::new(1, -1)));
        assert_eq!(block_to_chunk(f64::NAN, 0.0), None);
    }

    #[test]
    fn floor_to_i32_matches_floor() {
        for v in [
            0.0, -0.0, 0.5, -0.5, 1.0, -1.0, 15.9, -16.0, -16.1, 1e9, -1e9,
        ] {
            assert_eq!(floor_to_i32(v), Some(v.floor() as i32), "{v}");
        }
        let max = f64::from(i32::MAX);
        let min = f64::from(i32::MIN);
        assert_eq!(floor_to_i32(max + 0.5), Some(i32::MAX));
        assert_eq!(floor_to_i32(max + 1.0), None);
        assert_eq!(floor_to_i32(min), Some(i32::MIN));
        assert_eq!(floor_to_i32(min - 0.5), None);
        for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(floor_to_i32(v), None);
        }
    }
}
