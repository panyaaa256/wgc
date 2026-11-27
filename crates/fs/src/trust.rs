//! Checking that InhabitedTime can be trusted.
//!
//! GC assumes that a chunk whose InhabitedTime is 0 has never had a
//! non-spectator player within 128 blocks: vanilla adds to it every tick while
//! a player is that close. Each check looks for a reason the assumption may
//! not hold, and only in the dimensions that GC targets.
//!
//! Files that cannot be read never fail the check run; they make the check
//! "unknown" instead.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use yaml_rust2::{Yaml, YamlLoader};

use crate::layout::{self, Dimension, LayoutKind, WorldLayout};
use crate::nbt::{self, PlayerDimension, PlayerSummary};
use crate::plan::{Inhabited, Plan, PlanError, is_targeted, read_inhabited};
use crate::types::{ChunkPos, DimensionId, GuardId, Known, UnknownReason};
use crate::versions::{DV_1_21_5, DV_FILES_26_1, DV_MAX_KNOWN};

/// Results of the trust checks, in ID order T-1..T-5 (T-3 is absent when
/// [`check_trust_before_plan`] was told to skip it). Each check is described
/// on its [`GuardId`] variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustReport {
    pub checks: Vec<CheckResult>,
}

impl TrustReport {
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
pub struct CheckResult {
    pub id: GuardId,
    pub status: CheckStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckStatus {
    Pass,
    Fail(Vec<TrustDetail>),
    Unknown(UnknownReason),
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum TrustDetail {
    PaperFixedInhabitedTime {
        file: PathBuf,
        value: i64,
    },
    /// `world_folder` is the folder whose game rules disable spawning (a
    /// dimension folder on Bukkit 26.1 and later).
    MobSpawningDisabled {
        world_folder: PathBuf,
    },
    PlayerInZeroChunk {
        uuid: String,
        dimension: DimensionId,
        pos: ChunkPos,
    },
}

/// Runs T-1..T-5 against a plan.
pub fn check_trust(plan: &Plan) -> TrustReport {
    run_checks(
        &plan.layout,
        plan.gc_dims.as_ref(),
        plan.protection_unknown.clone(),
        Some(|wanted: &BTreeSet<ChunkKey>| Ok(inhabited_in_plan(plan, wanted))),
    )
}

/// Runs T-1..T-5 without a plan, so that `gc --out` can stop before reading
/// every region file. T-3 reads only the region files that hold the chunks
/// players stand in, and T-5 reads the forced chunks directly.
///
/// With `skip_t3`, T-3 reads nothing and is left out of the report: when
/// `--force` allows it, it cannot stop the run, and the check against the plan
/// decides whether it was forced.
pub fn check_trust_before_plan(
    layout: &WorldLayout,
    gc_dims: Option<&BTreeSet<DimensionId>>,
    skip_t3: bool,
) -> TrustReport {
    let protection_unknown = layout
        .dimensions
        .iter()
        .filter(|d| is_targeted(gc_dims, &d.id))
        .filter(|d| matches!(layout::forced_chunks(layout, d), Known::Unknown(_)))
        .map(|d| d.id.clone())
        .collect();
    let inhabited_of =
        (!skip_t3).then_some(|wanted: &BTreeSet<ChunkKey>| read_inhabited_of(layout, wanted));
    run_checks(layout, gc_dims, protection_unknown, inhabited_of)
}

/// A chunk in a dimension.
type ChunkKey = (DimensionId, ChunkPos);

/// The InhabitedTime of the chunks T-3 looks up; chunks that do not exist are left out.
type InhabitedMap = BTreeMap<ChunkKey, Inhabited>;

fn run_checks(
    layout: &WorldLayout,
    gc_dims: Option<&BTreeSet<DimensionId>>,
    protection_unknown: BTreeSet<DimensionId>,
    inhabited_of: Option<impl FnOnce(&BTreeSet<ChunkKey>) -> Result<InhabitedMap, UnknownReason>>,
) -> TrustReport {
    let targeted: Vec<&Dimension> = layout
        .dimensions
        .iter()
        .filter(|d| is_targeted(gc_dims, &d.id))
        .collect();
    let t3 = inhabited_of.map(|inhabited_of| CheckResult {
        id: GuardId::T3,
        status: player_positions(layout, gc_dims, inhabited_of),
    });
    let checks = [
        Some(CheckResult {
            id: GuardId::T1,
            status: paper_config(layout, &targeted),
        }),
        Some(CheckResult {
            id: GuardId::T2,
            status: mob_spawning(layout, &targeted),
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

fn from_details(details: Vec<TrustDetail>) -> CheckStatus {
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

/// Parses a YAML file; `Ok(None)` if it does not exist. An empty file is `Null`.
fn load_yaml(path: &Path) -> Result<Option<Yaml>, UnknownReason> {
    let unreadable = || UnknownReason::UnreadableFile {
        path: path.to_path_buf(),
    };
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(unreadable()),
    };
    let docs = YamlLoader::load_from_str(&text).map_err(|_| unreadable())?;
    Ok(Some(docs.into_iter().next().unwrap_or(Yaml::Null)))
}

/// Looks up `keys` under `doc`. A missing key is `Ok(None)`; a node on the way
/// that is neither a mapping nor null is an unexpected value.
fn lookup<'a>(
    doc: &'a Yaml,
    keys: &[&str],
    path: &Path,
) -> Result<Option<&'a Yaml>, UnknownReason> {
    let mut node = doc;
    for key in keys {
        match node {
            Yaml::Hash(map) => match map.get(&Yaml::String((*key).to_owned())) {
                Some(next) => node = next,
                None => return Ok(None),
            },
            Yaml::Null => return Ok(None),
            _ => {
                return Err(UnknownReason::UnexpectedValue {
                    path: path.to_path_buf(),
                    key: keys.join("."),
                });
            }
        }
    }
    Ok(Some(node))
}

fn fixed_time(
    value: Option<&Yaml>,
    keys: &[&str],
    path: &Path,
) -> Result<FixedTime, UnknownReason> {
    match value {
        None | Some(Yaml::Null) => Ok(FixedTime::Inherit),
        Some(Yaml::String(s)) if s == "default" => Ok(FixedTime::Inherit),
        Some(Yaml::Integer(v)) if *v >= 0 => Ok(FixedTime::On(*v)),
        Some(Yaml::Integer(_)) => Ok(FixedTime::Off),
        Some(_) => Err(UnknownReason::UnexpectedValue {
            path: path.to_path_buf(),
            key: keys.join("."),
        }),
    }
}

/// The setting in a Paper 1.19+ file (`paper-world.yml`, `paper-world-defaults.yml`).
fn read_modern(path: &Path) -> Result<FixedTime, UnknownReason> {
    const KEYS: [&str; 2] = ["chunks", "fixed-chunk-inhabited-time"];
    match load_yaml(path)? {
        None => Ok(FixedTime::Inherit),
        Some(doc) => fixed_time(lookup(&doc, &KEYS, path)?, &KEYS, path),
    }
}

/// T-1, for Bukkit layouts: fails when `fixed-chunk-inhabited-time` is 0 or
/// more for a world that holds a targeted dimension, because Paper then pins
/// every chunk's InhabitedTime to that value. A per-world value overrides the
/// server default; `default`, a missing key or a missing file inherit it; a
/// negative value turns the feature off.
fn paper_config(layout: &WorldLayout, targeted: &[&Dimension]) -> CheckStatus {
    let Some(server_root) = layout
        .server_root
        .as_ref()
        .filter(|_| layout.kind.is_bukkit())
    else {
        return CheckStatus::NotApplicable;
    };
    let result = (|| {
        let defaults = server_root.join("config/paper-world-defaults.yml");
        let legacy = server_root.join("paper.yml");
        let mut details = BTreeSet::new();
        // Paper 1.19 moved paper.yml to config/; paper.yml alone means an older Paper.
        if !defaults.exists() && legacy.exists() {
            let doc = load_yaml(&legacy)?.unwrap_or(Yaml::Null);
            let folders: BTreeSet<&PathBuf> = targeted.iter().map(|d| &d.world_folder).collect();
            for folder in folders {
                let world = folder
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();
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

        let default_time = read_modern(&defaults)?;
        let world_files: BTreeSet<PathBuf> = targeted
            .iter()
            .filter_map(|d| layout::paper_world_config(layout, d))
            .collect();
        for file in &world_files {
            let found = match read_modern(file)? {
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
fn mob_spawning(layout: &WorldLayout, targeted: &[&Dimension]) -> CheckStatus {
    let affected = layout.data_version.is_some_and(|v| v >= DV_1_21_5);
    if !layout.kind.is_bukkit() || !affected {
        return CheckStatus::NotApplicable;
    }
    let mut details = BTreeSet::new();
    let mut unknown = None;
    for dim in targeted {
        match layout::spawn_mobs(layout, dim) {
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
fn read_players(layout: &WorldLayout) -> Result<Vec<(String, PlayerSummary)>, UnknownReason> {
    let unreadable = |path: &Path| UnknownReason::UnreadableFile {
        path: path.to_path_buf(),
    };
    let mut players = Vec::new();
    let dir = layout::player_data_dir(layout);
    let mut files = Vec::new();
    match fs::read_dir(&dir) {
        Ok(entries) => {
            for entry in entries {
                files.push(entry.map_err(|_| unreadable(&dir))?.path());
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(unreadable(&dir)),
    }
    files.sort();
    for path in files {
        let Some(stem) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".dat"))
            .filter(|stem| is_uuid(stem))
        else {
            continue;
        };
        let player = read_gzip(&path, nbt::read_player).ok_or_else(|| unreadable(&path))?;
        players.push((stem.to_owned(), player));
    }

    if layout.data_version.is_none_or(|v| v < DV_FILES_26_1) {
        let level = layout.main_world_folder.join("level.dat");
        let host = read_gzip(&level, nbt::read_level_player).ok_or_else(|| unreadable(&level))?;
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
fn player_positions(
    layout: &WorldLayout,
    gc_dims: Option<&BTreeSet<DimensionId>>,
    inhabited_of: impl FnOnce(&BTreeSet<ChunkKey>) -> Result<InhabitedMap, UnknownReason>,
) -> CheckStatus {
    let players = match read_players(layout) {
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

fn inhabited_in_plan(plan: &Plan, wanted: &BTreeSet<ChunkKey>) -> InhabitedMap {
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

/// Reads the chunks from the region files, one dimension at a time.
fn read_inhabited_of(
    layout: &WorldLayout,
    wanted: &BTreeSet<ChunkKey>,
) -> Result<InhabitedMap, UnknownReason> {
    let mut found = BTreeMap::new();
    for dim in &layout.dimensions {
        let chunks: BTreeSet<ChunkPos> = wanted
            .iter()
            .filter(|(id, _)| *id == dim.id)
            .map(|(_, pos)| *pos)
            .collect();
        let inhabited =
            read_inhabited(dim, &chunks).map_err(|e| UnknownReason::UnreadableFile {
                path: match e {
                    PlanError::Io { path, .. } => path,
                    _ => dim.dir.clone(),
                },
            })?;
        found.extend(
            inhabited
                .into_iter()
                .map(|(pos, inhabited)| ((dim.id.clone(), pos), inhabited)),
        );
    }
    Ok(found)
}

fn block_to_chunk(x: f64, z: f64) -> Option<ChunkPos> {
    let to_chunk = |v: f64| {
        let c = (v / 16.0).floor();
        (c.is_finite() && c >= f64::from(i32::MIN) && c <= f64::from(i32::MAX)).then_some(c as i32)
    };
    Some(ChunkPos::new(to_chunk(x)?, to_chunk(z)?))
}

fn is_uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

fn read_gzip<T>(path: &Path, parse: impl FnOnce(&[u8]) -> Result<T, nbt::NbtError>) -> Option<T> {
    let bytes = fs::read(path).ok()?;
    let nbt = nbt::gunzip(&bytes).ok()?;
    parse(&nbt).ok()
}

// ---- T-4, T-5 --------------------------------------------------------------

/// T-4: unknown when the world's DataVersion is newer than any version wgc
/// knows, since its InhabitedTime behavior has not been checked.
fn version(layout: &WorldLayout) -> CheckStatus {
    match layout.data_version {
        Some(v) if v > DV_MAX_KNOWN => CheckStatus::Unknown(UnknownReason::UnknownVersion(v)),
        _ => CheckStatus::Pass,
    }
}

/// T-5: unknown when the forced chunks of a targeted dimension could not be
/// read, so chunks around them cannot be protected from deletion.
fn forced_chunks(
    gc_dims: Option<&BTreeSet<DimensionId>>,
    mut unknown: BTreeSet<DimensionId>,
) -> CheckStatus {
    unknown.retain(|id| is_targeted(gc_dims, id));
    if unknown.is_empty() {
        CheckStatus::Pass
    } else {
        CheckStatus::Unknown(UnknownReason::ForcedChunksUnknown(unknown))
    }
}

#[cfg(test)]
mod tests;
