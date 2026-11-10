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

/// `chunks.fixed-chunk-inhabited-time` in one YAML file.
enum FixedTime {
    /// File or key missing, or `default`: use the server defaults.
    Inherit,
    /// A negative value: the feature is off.
    Off,
    On(i64),
}

fn read_fixed_time(path: &Path) -> Result<FixedTime, UnknownReason> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(FixedTime::Inherit),
        Err(_) => {
            return Err(UnknownReason::UnreadableFile {
                path: path.to_path_buf(),
            });
        }
    };
    let docs = YamlLoader::load_from_str(&text).map_err(|_| UnknownReason::UnreadableFile {
        path: path.to_path_buf(),
    })?;
    let Some(doc) = docs.first() else {
        return Ok(FixedTime::Inherit);
    };
    match &doc["chunks"]["fixed-chunk-inhabited-time"] {
        Yaml::BadValue | Yaml::Null => Ok(FixedTime::Inherit),
        Yaml::String(s) if s == "default" => Ok(FixedTime::Inherit),
        Yaml::Integer(v) if *v >= 0 => Ok(FixedTime::On(*v)),
        Yaml::Integer(_) => Ok(FixedTime::Off),
        _ => Err(UnknownReason::UnexpectedValue {
            path: path.to_path_buf(),
            key: "chunks.fixed-chunk-inhabited-time".to_owned(),
        }),
    }
}

fn paper_config(plan: &Plan, targeted: &[&Dimension]) -> CheckStatus {
    let layout = &plan.layout;
    if !layout.kind.is_bukkit() {
        return CheckStatus::NotApplicable;
    }
    let world_files: BTreeSet<PathBuf> = targeted
        .iter()
        .filter_map(|d| layout::paper_world_config(layout, d))
        .collect();
    let defaults = layout::paper_defaults_config(layout);
    let result = (|| {
        let default_time = match &defaults {
            Some(path) => read_fixed_time(path)?,
            None => FixedTime::Inherit,
        };
        let mut details = Vec::new();
        for file in &world_files {
            let (value, source) = match read_fixed_time(file)? {
                FixedTime::Inherit => match default_time {
                    FixedTime::On(v) => (Some(v), defaults.clone().unwrap_or_default()),
                    _ => (None, PathBuf::new()),
                },
                FixedTime::On(v) => (Some(v), file.clone()),
                FixedTime::Off => (None, PathBuf::new()),
            };
            if let Some(value) = value {
                details.push(TrustDetail::PaperFixedInhabitedTime {
                    file: source,
                    value,
                });
            }
        }
        // With no world file to inspect, the defaults decide.
        if world_files.is_empty()
            && let (FixedTime::On(value), Some(file)) = (default_time, defaults.clone())
        {
            details.push(TrustDetail::PaperFixedInhabitedTime { file, value });
        }
        details.sort();
        details.dedup();
        Ok(details)
    })();
    match result {
        Ok(details) => from_details(details),
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
            Known::Unknown(reason) => return CheckStatus::Unknown(reason),
        }
    }
    from_details(details.into_iter().collect())
}

// ---- T-3 -----------------------------------------------------------------

const SPECTATOR: i32 = 3;

fn player_positions(plan: &Plan) -> CheckStatus {
    let layout = &plan.layout;
    // InhabitedTime of every chunk in targeted dimensions.
    let mut chunks: BTreeMap<(&DimensionId, ChunkPos), &Inhabited> = BTreeMap::new();
    for dim in &plan.dimensions {
        if !plan.is_targeted(&dim.dimension.id) {
            continue;
        }
        for t in &dim.triples {
            for c in &t.chunks {
                chunks.insert((&dim.dimension.id, c.pos), &c.inhabited);
            }
        }
    }

    let mut players: Vec<(String, PlayerSummary)> = Vec::new();
    let dir = layout::player_data_dir(layout);
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .map(|entries| entries.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
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
        if let Some(player) = read_gzip(&path, nbt::read_player) {
            players.push((stem.to_owned(), player));
        }
    }
    let has_level_player = layout.data_version.is_none_or(|v| v < DV_FILES_26_1);
    if has_level_player
        && let Some(Some(player)) = read_gzip(
            &layout.main_world_folder.join("level.dat"),
            nbt::read_level_player,
        )
    {
        players.push(("level.dat".to_owned(), player));
    }

    let mut checked = 0;
    let mut details = Vec::new();
    for (uuid, player) in players {
        if player.game_type == Some(SPECTATOR) {
            continue;
        }
        let (Some(pos), Some(dimension)) = (player.pos, player.dimension) else {
            continue;
        };
        let id = match dimension {
            PlayerDimension::Legacy(0) => DimensionId::overworld(),
            PlayerDimension::Legacy(-1) => DimensionId::the_nether(),
            PlayerDimension::Legacy(1) => DimensionId::the_end(),
            PlayerDimension::Legacy(_) => continue,
            PlayerDimension::Named(name) => match DimensionId::parse(&name) {
                Ok(id) => id,
                Err(_) => continue,
            },
        };
        let Some(chunk) = block_to_chunk(pos[0], pos[2]) else {
            continue;
        };
        match chunks.get(&(&id, chunk)) {
            Some(Inhabited::Value(0)) => {
                checked += 1;
                details.push(TrustDetail::PlayerInZeroChunk {
                    uuid,
                    dimension: id,
                    pos: chunk,
                });
            }
            Some(Inhabited::Value(_)) => checked += 1,
            // Missing chunk, unknown InhabitedTime, or a dimension not targeted.
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
