//! Every message the tool prints, in one place (English only).

use wgc_core::layout::LayoutError;
use wgc_core::lock::LockError;
use wgc_core::output::{ChangedFile, OutputError, VerifyError, VerifyFailure};
use wgc_core::plan::PlanError;
use wgc_core::report::{DetailJson, Notice, ReasonJson, ReportError};
use wgc_core::types::{GuardId, Warning};

use crate::ui::ListCounts;

pub fn notice(n: Notice) -> &'static str {
    match n {
        Notice::PregeneratedChunks => {
            "Pre-generated chunks are also deleted. On a pre-generated world, this undoes the pre-generation."
        }
        Notice::BoundaryGeneral => {
            "Deleted chunks are regenerated with the current version and world generation settings when they are next loaded. Terrain may not line up where it meets older chunks."
        }
        Notice::RunOfficialOptimize => {
            "Consider running the game's Optimize World (--forceUpgrade)."
        }
        Notice::NoTrash => {
            "The original world was not modified. There is no trash: once you delete the original world, it cannot be restored. Keep it until you have checked the new one."
        }
    }
}

pub fn older_chunks(n: u64) -> String {
    format!(
        "{n} chunks are in an older format. {}",
        notice(Notice::RunOfficialOptimize)
    )
}

pub const VERBOSE_HINT: &str = "(run with --verbose for details)";

fn count(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The counts of the lists the default summary leaves out; `None` when there is nothing.
pub fn also_found(counts: &ListCounts, temp_files: u64) -> Option<String> {
    let parts: Vec<String> = [
        (counts.integrity, "integrity problem", "integrity problems"),
        (counts.empty_files, "empty file", "empty files"),
        (
            counts.orphans,
            "entity/POI entry without a chunk",
            "entity/POI entries without a chunk",
        ),
        (
            temp_files,
            "leftover player data temp file",
            "leftover player data temp files",
        ),
        (
            counts.seams,
            "seam with old terrain",
            "seams with old terrain",
        ),
    ]
    .into_iter()
    .filter(|(n, ..)| *n > 0)
    .map(|(n, one, many)| count(n, one, many))
    .collect();
    (!parts.is_empty()).then(|| format!("Also found: {}", parts.join(", ")))
}

pub fn temp_files_not_copied(n: u64) -> String {
    format!(
        "Also not copied: {}",
        count(
            n,
            "leftover player data temp file",
            "leftover player data temp files"
        )
    )
}

pub fn check_name(id: GuardId) -> &'static str {
    match id {
        GuardId::T1 => "Paper fixed-chunk-inhabited-time",
        GuardId::T2 => "mob spawning (PaperMC/Paper#12827)",
        GuardId::T3 => "player positions",
        GuardId::T4 => "known data version",
        GuardId::T5 => "forced chunks readable",
        GuardId::L1 => "session.lock present",
    }
}

pub fn detail(d: &DetailJson) -> String {
    match d {
        DetailJson::PaperFixedInhabitedTime { file, value } => {
            format!(
                "{file}: fixed-chunk-inhabited-time is {value}, so every chunk has the same InhabitedTime"
            )
        }
        DetailJson::MobSpawningDisabled { world_folder } => {
            format!(
                "{world_folder}: mob spawning is disabled; Paper then does not count InhabitedTime"
            )
        }
        DetailJson::PlayerInZeroChunk {
            uuid,
            dimension,
            pos,
        } => {
            format!(
                "player {uuid} is in chunk [{}, {}] of {dimension}, whose InhabitedTime is 0",
                pos[0], pos[1]
            )
        }
    }
}

pub fn reason(r: &ReasonJson) -> String {
    match r {
        ReasonJson::UnreadableFile { path } => format!("cannot read {path}"),
        ReasonJson::UnexpectedValue { path, key } => format!("{path}: unexpected value for {key}"),
        ReasonJson::UnknownVersion { data_version } => {
            format!("data version {data_version} is newer than this version of wgc knows")
        }
        ReasonJson::NoPlayersChecked => "no player position could be checked".to_owned(),
        ReasonJson::ForcedChunksUnknown { dimensions } => {
            format!("cannot read the forced chunks of {}", dimensions.join(", "))
        }
    }
}

pub fn warning(w: &Warning) -> String {
    match w {
        Warning::LockUnavailable(e) => match e {
            LockError::InUse { .. } | LockError::HeldByThisProcess { .. } => {
                "The world is in use. Results may be inaccurate.".to_owned()
            }
            LockError::Missing { path } => format!(
                "{} is missing, so a running game cannot be detected. Results may be inaccurate.",
                path.display()
            ),
            LockError::NotWritable { path } => format!(
                "Cannot open {} for writing, so the world could not be locked. Results may be inaccurate.",
                path.display()
            ),
            LockError::Io { path, source } => format!(
                "Cannot lock {} ({source}). Results may be inaccurate.",
                path.display()
            ),
        },
        Warning::LockSkipped => "session.lock is missing: continuing without locking the world (--force).".to_owned(),
        Warning::PreLockVersion(v) => format!(
            "The world was last saved by an old version (data version {}). A running game cannot be detected.",
            v.0
        ),
        Warning::OwnerNotPreserved => {
            "Output files are owned by the current user, not by the original owner. Check that the server user can write them.".to_owned()
        }
        Warning::SymlinkFollowed(p) => format!("Followed symbolic link {}; the output has its contents.", p.display()),
        Warning::SpecialFileSkipped(p) => format!("Skipped {} (not a regular file, or a broken link).", p.display()),
        Warning::Forced(id) => format!(
            "Ignoring check {id} ({}) because of --force.{}",
            check_name(*id),
            if *id == GuardId::T5 { " Forced chunks are NOT protected." } else { "" }
        ),
    }
}

fn lock(e: &LockError) -> String {
    match e {
        LockError::InUse { .. } | LockError::HeldByThisProcess { .. } => {
            "The world is in use. Stop the server or game and try again.".to_owned()
        }
        LockError::NotWritable { path } => format!(
            "Cannot open {} for writing. Run wgc as the user that runs the server.",
            path.display()
        ),
        LockError::Missing { path } => format!("{} does not exist.", path.display()),
        LockError::Io { path, source } => format!("Cannot lock {}: {source}", path.display()),
    }
}

pub fn layout(e: &LayoutError, command: &str) -> String {
    match e {
        LayoutError::LooksLikeVanillaServer { suggested } => format!(
            "This looks like a vanilla server. Pass the world folder instead: wgc {command} {}",
            suggested.display()
        ),
        LayoutError::LooksLikeBukkitWorldFolder => {
            "This looks like a world folder of a Bukkit-based server. Pass the server root folder instead.".to_owned()
        }
        LayoutError::NotAWorld { path } => format!("No Minecraft world found at {}.", path.display()),
        LayoutError::AmbiguousRegion { first, second } => format!(
            "{} and {} name the same region; remove one of them.",
            first.display(),
            second.display()
        ),
        LayoutError::DuplicateDimension { first, second } => format!(
            "{} and {} are the same dimension; remove one of them.",
            first.display(),
            second.display()
        ),
        LayoutError::MissingLevelDat { path } => format!("{} not found.", path.display()),
        LayoutError::UnreadableLevelDat { path, source } => format!("Cannot read {}: {source}.", path.display()),
        LayoutError::SymlinkLoop { path } => format!("Symbolic link loop at {}.", path.display()),
        LayoutError::Io { path, source } => format!("Cannot access {}: {source}.", path.display()),
    }
}

pub fn plan(e: &PlanError, command: &str) -> String {
    match e {
        PlanError::Io { path, source } => format!("Cannot read {}: {source}.", path.display()),
        PlanError::Layout(l) => layout(l, command),
        PlanError::Cancelled => "Interrupted.".to_owned(),
    }
}

pub fn report(e: &ReportError, command: &str) -> String {
    match e {
        ReportError::NonUtf8Path(p) => format!("{} is not a valid UTF-8 path.", p.display()),
        ReportError::Plan(p) => plan(p, command),
    }
}

pub fn output(e: &OutputError) -> String {
    match e {
        OutputError::OutputNotEmpty { path } => format!(
            "{} exists and is not empty. Choose a new or empty folder.",
            path.display()
        ),
        OutputError::InvalidOutputPath { path } => format!(
            "{}: '..' is not allowed in the part that does not exist yet.",
            path.display()
        ),
        OutputError::OutputOverlapsSource { out, world } => format!(
            "{} overlaps the world at {}. Choose a folder outside the world.",
            out.display(),
            world.display()
        ),
        OutputError::Lock(l) => lock(l),
        OutputError::Stopped(ids) => {
            let mut text = "Stopped. Nothing was written.".to_owned();
            for id in ids {
                text.push_str(&format!(
                    "\n  {id} {}: {}",
                    check_name(*id),
                    stop_advice(*id)
                ));
            }
            let list: Vec<&str> = ids.iter().map(|i| i.as_str()).collect();
            text.push_str(&format!(
                "\nRun `wgc scan` for details. To continue anyway, run again with --force={}.",
                list.join(",")
            ));
            text
        }
        OutputError::InsufficientSpace { needed, available } => format!(
            "Not enough free space: {} needed, {} available.",
            crate::ui::bytes(*needed),
            crate::ui::bytes(*available)
        ),
        OutputError::Layout(l) => layout(l, "gc"),
        OutputError::Plan(p) => plan(p, "gc"),
        OutputError::Verification(failures) => {
            let mut text = "The output failed verification; it is incomplete. Please report this with .wgc/failure.json.".to_owned();
            for VerifyFailure {
                check,
                path,
                detail,
            } in failures.iter().take(10)
            {
                text.push_str(&format!("\n  {check} {}: {detail}", path.display()));
            }
            if failures.len() > 10 {
                text.push_str(&format!("\n  ... and {} more", failures.len() - 10));
            }
            text
        }
        OutputError::Io { path, source } => format!("Cannot access {}: {source}.", path.display()),
        OutputError::Cancelled => {
            "Interrupted. The output is incomplete (no complete.json).".to_owned()
        }
    }
}

pub fn verify(e: &VerifyError) -> String {
    match e {
        VerifyError::Incomplete { .. } => "The output is incomplete.".to_owned(),
        VerifyError::SourceMismatch { recorded, given } => format!(
            "The output was made from {}, not {}.",
            recorded.display(),
            given.display()
        ),
        VerifyError::Changed(changed) => {
            let mut text =
                "The world has changed since the output was created. Swapping it in would lose those changes.".to_owned();
            for c in changed.iter().take(10) {
                let (kind, path) = match c {
                    ChangedFile::Added(p) => ("added", p),
                    ChangedFile::Removed(p) => ("removed", p),
                    ChangedFile::Modified(p) => ("modified", p),
                };
                text.push_str(&format!("\n  {kind}: {path}"));
            }
            if changed.len() > 10 {
                text.push_str(&format!("\n  ... and {} more", changed.len() - 10));
            }
            text
        }
        VerifyError::InUse { .. } => "The world is in use.".to_owned(),
        VerifyError::Io { path, source } => format!("Cannot access {}: {source}.", path.display()),
    }
}

/// What ignoring a stop means.
fn stop_advice(id: GuardId) -> &'static str {
    match id {
        GuardId::T1 | GuardId::T2 | GuardId::T3 => {
            "InhabitedTime may be unreliable, so visited chunks could be deleted. Use --force only if you are sure it is reliable."
        }
        GuardId::T4 => {
            "this world was saved by a newer version than wgc knows; its files may have changed format."
        }
        GuardId::T5 => "the forced chunks could not be read; with --force they are NOT protected.",
        GuardId::L1 => {
            "session.lock is missing, so a running game cannot be detected; make sure the game is stopped."
        }
    }
}

pub const READY: &str = "Ready to swap.";
pub const INTERRUPTED: &str = "Interrupted.";
pub const NOTHING_WRITTEN: &str =
    "Nothing was written. Run again with --out <DIR> to write the result.";
pub const DONE: &str = "Done:";

pub fn unknown_dimension(id: &str) -> String {
    format!("This world has no dimension {id}.")
}

pub fn forced_output(ids: &[String]) -> String {
    format!(
        "This output was created with --force (ignored checks: {}).",
        ids.join(", ")
    )
}
