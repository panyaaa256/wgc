//! Data versions the tool depends on.

use crate::types::DataVersion;

/// 20w14a: `session.lock` is held with an OS lock from here on.
pub const DV_SESSION_LOCK_OS: DataVersion = DataVersion(2524);
/// 21w43a: chunk NBT fields moved from `Level` to the root.
pub const DV_CHUNK_ROOT: DataVersion = DataVersion(2844);
/// 1.21.5 snapshot: forced chunks stored as `tickets` instead of `Forced`.
pub const DV_TICKETS: DataVersion = DataVersion(4297);
/// 1.21.5: first version affected by PaperMC/Paper#12827, where Paper stops
/// increasing `InhabitedTime` while mob spawning is disabled (trust check `T-2`).
pub const DV_1_21_5: DataVersion = DataVersion(4325);
/// `GameRules` became `game_rules` with namespaced keys and typed values.
pub const DV_GAMERULE_REGISTRY: DataVersion = DataVersion(4658);
/// 26.1 snapshot: dimensions, player data and saved data moved.
pub const DV_FILES_26_1: DataVersion = DataVersion(4772);
/// Newest version whose save format the tool knows (26.3).
pub const DV_MAX_KNOWN: DataVersion = DataVersion(5023);
