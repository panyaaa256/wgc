//! Checking that InhabitedTime can be trusted (docs/spec/trust.md).
//!
//! Files that cannot be read never fail the check run; they make the check
//! "unknown" instead.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use yaml_rust2::{Yaml, YamlLoader};

use crate::layout::{self, Dimension, LayoutKind};
use crate::nbt::{self, PlayerDimension, PlayerSummary};
use crate::plan::{Inhabited, Plan};
use crate::types::{ChunkPos, DimensionId, GuardId, Known, UnknownReason};
use crate::versions::{DV_1_21_5, DV_FILES_26_1, DV_MAX_KNOWN};

/// Results of all trust checks, in ID order T-1..T-5.
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
    let targeted: Vec<&Dimension> = plan
        .layout
        .dimensions
        .iter()
        .filter(|d| plan.is_targeted(&d.id))
        .collect();
    TrustReport {
        checks: vec![
            CheckResult {
                id: GuardId::T1,
                status: paper_config(plan, &targeted),
            },
            CheckResult {
                id: GuardId::T2,
                status: mob_spawning(plan, &targeted),
            },
            CheckResult {
                id: GuardId::T3,
                status: player_positions(plan),
            },
            CheckResult {
                id: GuardId::T4,
                status: version(plan),
            },
            CheckResult {
                id: GuardId::T5,
                status: forced_chunks(plan),
            },
        ],
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

fn paper_config(plan: &Plan, targeted: &[&Dimension]) -> CheckStatus {
    let layout = &plan.layout;
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

fn mob_spawning(plan: &Plan, targeted: &[&Dimension]) -> CheckStatus {
    let layout = &plan.layout;
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
fn read_players(
    layout: &crate::layout::WorldLayout,
) -> Result<Vec<(String, PlayerSummary)>, UnknownReason> {
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
fn player_chunk(player: &PlayerSummary) -> Option<(DimensionId, ChunkPos)> {
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

fn player_positions(plan: &Plan) -> CheckStatus {
    let players = match read_players(&plan.layout) {
        Ok(players) => players,
        Err(reason) => return CheckStatus::Unknown(reason),
    };
    let wanted: Vec<(String, DimensionId, ChunkPos)> = players
        .into_iter()
        .filter_map(|(uuid, p)| player_chunk(&p).map(|(id, pos)| (uuid, id, pos)))
        .filter(|(_, id, _)| plan.is_targeted(id))
        .collect();
    let keys: BTreeSet<(&DimensionId, ChunkPos)> =
        wanted.iter().map(|(_, id, pos)| (id, *pos)).collect();

    // Look up only the chunks players stand in.
    let mut inhabited: BTreeMap<(&DimensionId, ChunkPos), &Inhabited> = BTreeMap::new();
    for dim in &plan.dimensions {
        for t in &dim.triples {
            for c in &t.chunks {
                if keys.contains(&(&dim.dimension.id, c.pos)) {
                    inhabited.insert((&dim.dimension.id, c.pos), &c.inhabited);
                }
            }
        }
    }

    let mut checked = 0;
    let mut details = Vec::new();
    for (uuid, id, pos) in &wanted {
        match inhabited.get(&(id, *pos)) {
            Some(Inhabited::Value(0)) => {
                checked += 1;
                details.push(TrustDetail::PlayerInZeroChunk {
                    uuid: uuid.clone(),
                    dimension: id.clone(),
                    pos: *pos,
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

fn version(plan: &Plan) -> CheckStatus {
    match plan.layout.data_version {
        Some(v) if v > DV_MAX_KNOWN => CheckStatus::Unknown(UnknownReason::UnknownVersion(v)),
        _ => CheckStatus::Pass,
    }
}

fn forced_chunks(plan: &Plan) -> CheckStatus {
    let unknown: BTreeSet<DimensionId> = plan
        .protection_unknown
        .iter()
        .filter(|id| plan.is_targeted(id))
        .cloned()
        .collect();
    if unknown.is_empty() {
        CheckStatus::Pass
    } else {
        CheckStatus::Unknown(UnknownReason::ForcedChunksUnknown(unknown))
    }
}

#[cfg(test)]
mod tests;
