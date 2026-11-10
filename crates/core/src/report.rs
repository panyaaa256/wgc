//! Reports and their JSON form (docs/spec/scan.md).
//!
//! The report types are separate from the plan so that the JSON schema can
//! stay stable while the plan changes. Paths are relative to the input path
//! with `/` separators; a path that is not UTF-8 makes the report fail.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::AtomicBool;

use serde::Serialize;

use crate::layout::WorldLayout;
use crate::lock::{LockError, SessionLocks};
use crate::plan::{
    Decision, EntryFate, FileAction, Inhabited, KeepReason, OmitReason, Plan, PlanError,
    PlanOptions, RemoveReason, build_plan,
};
use crate::region::{ContentError, StructuralId};
use crate::trust::{CheckStatus, TrustDetail, TrustReport, check_trust};
use crate::types::{ChunkPos, FileKind, GuardId, Observer, UnknownReason, Warning};

/// Why a report could not be produced.
#[derive(Debug, thiserror::Error)]
pub enum ReportError {
    #[error("path is not valid utf-8: {0}")]
    NonUtf8Path(PathBuf),
    #[error(transparent)]
    Plan(#[from] PlanError),
}

/// State of `session.lock` while the report was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LockStatus {
    Acquired,
    InUse,
    Missing,
    NotWritable,
    Error,
}

impl LockStatus {
    pub fn from_result(result: &Result<SessionLocks, LockError>) -> Self {
        match result {
            Ok(_) => LockStatus::Acquired,
            Err(LockError::InUse { .. } | LockError::HeldByThisProcess { .. }) => LockStatus::InUse,
            Err(LockError::Missing { .. }) => LockStatus::Missing,
            Err(LockError::NotWritable { .. }) => LockStatus::NotWritable,
            Err(LockError::Io { .. }) => LockStatus::Error,
        }
    }
}

/// Fixed notices shown with a report (docs/spec/cli.md "固定の注意文").
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Notice {
    PregeneratedChunks,
    BoundaryGeneral,
    RunOfficialOptimize,
    NoTrash,
}

/// Everything `wgc scan` reports (`wgc.scan/1`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScanReport {
    pub schema: &'static str,
    pub tool_version: &'static str,
    pub layout: LayoutReport,
    pub world_data_version: Option<i32>,
    pub lock: LockStatus,
    pub trust: Vec<TrustItem>,
    pub other_files: OtherFiles,
    pub notices: Vec<Notice>,
    pub dimensions: Vec<DimensionReport>,
}

impl ScanReport {
    /// Whether any trust check failed or could not be confirmed (would stop `gc --out`).
    pub fn has_stops(&self) -> bool {
        self.trust
            .iter()
            .any(|t| matches!(t.status, TrustStatus::Fail | TrustStatus::Unknown))
    }
}

/// The `wgc gc` report (`wgc.gc/1`): the scan report plus what `gc` did.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GcReport {
    #[serde(flatten)]
    pub scan: ScanReport,
    pub mode: GcMode,
    /// Canonical absolute path of the output, for `output`.
    pub out: Option<String>,
    pub forced: Vec<GuardId>,
    pub dimensions_targeted: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GcMode {
    Estimate,
    Output,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LayoutReport {
    pub kind: &'static str,
    pub base: Option<String>,
    pub world_folders: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TrustItem {
    pub id: GuardId,
    pub status: TrustStatus,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<DetailJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<ReasonJson>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustStatus {
    Pass,
    Fail,
    Unknown,
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DetailJson {
    PaperFixedInhabitedTime {
        file: String,
        value: i64,
    },
    MobSpawningDisabled {
        world_folder: String,
    },
    PlayerInZeroChunk {
        uuid: String,
        dimension: String,
        pos: [i32; 2],
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReasonJson {
    UnreadableFile { path: String },
    UnexpectedValue { path: String, key: String },
    UnknownVersion { data_version: i32 },
    NoPlayersChecked,
    ForcedChunksUnknown { dimensions: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OtherFiles {
    pub omitted: Vec<OmittedJson>,
    pub dat_old_without_dat: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OmittedJson {
    pub path: String,
    pub reason: OmitJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OmitJson {
    EmptyFile,
    NoEntriesLeft,
    PlayerTempFile,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DimensionReport {
    pub id: String,
    pub dir: String,
    pub size: KindBytes,
    pub chunks: ChunkCounts,
    pub inhabited_time: InhabitedBuckets,
    pub gc: GcSummary,
    pub integrity: Integrity,
    pub orphans: OrphanCounts,
    pub empty_files: Vec<String>,
    pub boundary_warnings: Vec<BoundaryJson>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct KindBytes {
    pub region: u64,
    pub entities: u64,
    pub poi: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct ChunkCounts {
    pub total: u64,
    pub older_data_version: u64,
}

/// InhabitedTime buckets in ticks (docs/spec/scan.md "InhabitedTime の区分").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct InhabitedBuckets {
    pub zero: u64,
    pub missing: u64,
    pub negative: u64,
    pub unreadable: u64,
    pub lt_1s: u64,
    pub lt_10s: u64,
    pub lt_1m: u64,
    pub lt_10m: u64,
    pub lt_1h: u64,
    pub lt_10h: u64,
    pub ge_10h: u64,
}

impl InhabitedBuckets {
    pub fn add(&mut self, inhabited: &Inhabited) {
        let slot = match inhabited {
            Inhabited::Missing => &mut self.missing,
            Inhabited::Negative(_) => &mut self.negative,
            Inhabited::Unreadable(_) => &mut self.unreadable,
            Inhabited::Value(0) => &mut self.zero,
            Inhabited::Value(t) => match *t {
                1..20 => &mut self.lt_1s,
                20..200 => &mut self.lt_10s,
                200..1_200 => &mut self.lt_1m,
                1_200..12_000 => &mut self.lt_10m,
                12_000..72_000 => &mut self.lt_1h,
                72_000..720_000 => &mut self.lt_10h,
                _ => &mut self.ge_10h,
            },
        };
        *slot += 1;
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GcSummary {
    pub delete: u64,
    pub protected: Protected,
    pub size_after: KindBytes,
    pub emptied_files: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct Protected {
    pub forceload: u64,
    pub structure_ref: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Integrity {
    pub structural: Vec<StructuralJson>,
    pub content: Vec<ContentJson>,
    pub verbatim_triples: Vec<[i32; 2]>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StructuralJson {
    pub path: String,
    pub id: StructuralId,
    pub index: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContentJson {
    pub pos: [i32; 2],
    pub kind: ContentKind,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Decompress,
    Nbt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct OrphanCounts {
    pub entities: u64,
    pub poi: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BoundaryJson {
    pub chunks: u32,
    pub min: [i32; 2],
    pub max: [i32; 2],
}

/// Locks the world for a read-only report if possible; otherwise warns
/// ([`Warning::LockUnavailable`]) and continues. Keep the returned locks alive while reading.
pub fn try_lock_for_report(
    layout: &WorldLayout,
    observer: &dyn Observer,
) -> (LockStatus, Option<SessionLocks>) {
    let locks = SessionLocks::try_acquire_all(layout);
    let lock = LockStatus::from_result(&locks);
    match locks {
        Ok(held) => (lock, Some(held)),
        Err(e) => {
            observer.warn(Warning::LockUnavailable(e));
            (lock, None)
        }
    }
}

/// Runs `wgc scan`: locks if possible, plans every dimension and checks trust.
pub fn run_scan(
    layout: &WorldLayout,
    observer: &dyn Observer,
    cancel: &AtomicBool,
) -> Result<ScanReport, ReportError> {
    let (lock, _held) = try_lock_for_report(layout, observer);
    let plan = build_plan(layout, &PlanOptions::default(), observer, cancel)?;
    let trust = check_trust(&plan);
    scan_report(&plan, &trust, lock)
}

/// Builds the scan report for a plan made without `--dimension`.
pub fn scan_report(
    plan: &Plan,
    trust: &TrustReport,
    lock: LockStatus,
) -> Result<ScanReport, ReportError> {
    let root = &plan.layout.root;
    let rel = |p: &Path| rel_path(root, p);
    let rels = |ps: &[PathBuf]| ps.iter().map(|p| rel(p)).collect::<Result<Vec<_>, _>>();

    let mut dimensions = Vec::new();
    let mut any_deleted = false;
    let mut any_older = false;
    for dim in &plan.dimensions {
        let r = dimension_report(plan, dim, &rel)?;
        any_deleted |= r.gc.delete > 0;
        any_older |= r.chunks.older_data_version > 0;
        dimensions.push(r);
    }

    let mut notices = vec![Notice::PregeneratedChunks];
    if any_deleted {
        notices.push(Notice::BoundaryGeneral);
    }
    if any_older {
        notices.push(Notice::RunOfficialOptimize);
    }

    Ok(ScanReport {
        schema: "wgc.scan/1",
        tool_version: crate::TOOL_VERSION,
        layout: LayoutReport {
            kind: plan.layout.kind.as_str(),
            base: plan.layout.base.clone(),
            world_folders: rels(&plan.layout.world_folders)?,
        },
        world_data_version: plan.layout.data_version.map(|v| v.0),
        lock,
        trust: trust
            .checks
            .iter()
            .map(|c| trust_item(c.id, &c.status, &rel))
            .collect::<Result<_, _>>()?,
        other_files: OtherFiles {
            omitted: plan
                .omitted_other_files
                .iter()
                .map(|o| {
                    Ok(OmittedJson {
                        path: rel(&o.path)?,
                        reason: omit_reason(o.reason),
                    })
                })
                .collect::<Result<_, ReportError>>()?,
            dat_old_without_dat: rels(&plan.dat_old_without_dat)?,
        },
        notices,
        dimensions,
    })
}

fn dimension_report(
    plan: &Plan,
    dim: &crate::plan::DimensionPlan,
    rel: &dyn Fn(&Path) -> Result<String, ReportError>,
) -> Result<DimensionReport, ReportError> {
    let world_version = plan.layout.data_version;
    let sizes = plan
        .estimate
        .dimensions
        .get(&dim.dimension.id)
        .copied()
        .unwrap_or_default();
    let mut chunks = ChunkCounts::default();
    let mut buckets = InhabitedBuckets::default();
    let mut gc = GcSummary {
        delete: 0,
        protected: Protected::default(),
        size_after: KindBytes {
            region: sizes.region.after,
            entities: sizes.entities.after,
            poi: sizes.poi.after,
        },
        emptied_files: Vec::new(),
    };
    let mut integrity = Integrity {
        structural: Vec::new(),
        content: Vec::new(),
        verbatim_triples: Vec::new(),
    };
    let mut orphans = OrphanCounts::default();
    let mut empty_files = Vec::new();

    for t in &dim.triples {
        if t.is_verbatim() {
            integrity.verbatim_triples.push([t.pos.x, t.pos.z]);
        }
        for issue in &t.issues {
            let path = &t
                .file(issue.kind)
                .expect("a structural issue belongs to a file of the triple")
                .path;
            integrity.structural.push(StructuralJson {
                path: rel(path)?,
                id: issue.id,
                index: issue.index.map(|i| i.get()),
            });
        }
        for c in &t.chunks {
            chunks.total += 1;
            if let (Some(v), Some(world)) = (c.data_version, world_version)
                && v < world
            {
                chunks.older_data_version += 1;
            }
            buckets.add(&c.inhabited);
            match c.decision {
                Decision::Delete => gc.delete += 1,
                Decision::Keep(KeepReason::Forceload) => gc.protected.forceload += 1,
                Decision::Keep(KeepReason::StructureRef) => gc.protected.structure_ref += 1,
                Decision::Keep(_) => {}
            }
            if let Inhabited::Unreadable(e) = &c.inhabited {
                integrity.content.push(ContentJson {
                    pos: pos(c.pos),
                    kind: match e {
                        ContentError::Decompress(_) => ContentKind::Decompress,
                        ContentError::Nbt(_) => ContentKind::Nbt,
                    },
                    detail: e.to_string(),
                });
            }
        }
        for kind in FileKind::ALL {
            let Some(file) = t.file(kind) else {
                continue;
            };
            match t.action(kind) {
                Some(FileAction::Omit(OmitReason::EmptyFile)) => empty_files.push(rel(&file.path)?),
                Some(FileAction::Omit(OmitReason::NoEntriesLeft)) => {
                    gc.emptied_files.push(rel(&file.path)?)
                }
                _ => {}
            }
            if kind != FileKind::Region {
                let n = file
                    .entries
                    .iter()
                    .filter(|e| {
                        t.entry_fate(kind, e.index) == EntryFate::Remove(RemoveReason::Orphan)
                    })
                    .count() as u64;
                match kind {
                    FileKind::Entities => orphans.entities += n,
                    _ => orphans.poi += n,
                }
            }
        }
    }

    Ok(DimensionReport {
        id: dim.dimension.id.to_string(),
        dir: rel(&dim.dimension.dir)?,
        size: KindBytes {
            region: sizes.region.before,
            entities: sizes.entities.before,
            poi: sizes.poi.before,
        },
        chunks,
        inhabited_time: buckets,
        gc,
        integrity,
        orphans,
        empty_files,
        boundary_warnings: dim
            .boundary_warnings
            .iter()
            .map(|b| BoundaryJson {
                chunks: b.chunks,
                min: pos(b.min),
                max: pos(b.max),
            })
            .collect(),
    })
}

/// Turns a scan report into the `wgc gc` report.
///
/// Pass `out` only after the output was written and verified: the report
/// then says `output` and carries the `no_trash` notice. `out` is
/// canonicalized (made absolute if it cannot be resolved).
pub fn gc_report(
    mut scan: ScanReport,
    out: Option<&Path>,
    forced: &BTreeSet<GuardId>,
    plan: &Plan,
) -> Result<GcReport, ReportError> {
    scan.schema = "wgc.gc/1";
    let out = out
        .map(|p| {
            let full = std::fs::canonicalize(p)
                .or_else(|_| std::path::absolute(p))
                .unwrap_or_else(|_| p.to_path_buf());
            full.to_str()
                .map(str::to_owned)
                .ok_or(ReportError::NonUtf8Path(full.clone()))
        })
        .transpose()?;
    if out.is_some() {
        scan.notices.push(Notice::NoTrash);
    }
    Ok(GcReport {
        scan,
        mode: if out.is_some() {
            GcMode::Output
        } else {
            GcMode::Estimate
        },
        out,
        forced: forced.iter().copied().collect(),
        dimensions_targeted: plan
            .gc_dims
            .as_ref()
            .map(|dims| dims.iter().map(ToString::to_string).collect()),
    })
}

fn pos(p: ChunkPos) -> [i32; 2] {
    [p.x, p.z]
}

fn omit_reason(r: OmitReason) -> OmitJson {
    match r {
        OmitReason::EmptyFile => OmitJson::EmptyFile,
        OmitReason::NoEntriesLeft => OmitJson::NoEntriesLeft,
        OmitReason::PlayerTempFile => OmitJson::PlayerTempFile,
    }
}

fn trust_item(
    id: GuardId,
    status: &CheckStatus,
    rel: &dyn Fn(&Path) -> Result<String, ReportError>,
) -> Result<TrustItem, ReportError> {
    let (name, details, reason) = match status {
        CheckStatus::Pass => (TrustStatus::Pass, Vec::new(), None),
        CheckStatus::NotApplicable => (TrustStatus::NotApplicable, Vec::new(), None),
        CheckStatus::Fail(details) => (
            TrustStatus::Fail,
            details
                .iter()
                .map(|d| {
                    Ok(match d {
                        TrustDetail::PaperFixedInhabitedTime { file, value } => {
                            DetailJson::PaperFixedInhabitedTime {
                                file: rel(file)?,
                                value: *value,
                            }
                        }
                        TrustDetail::MobSpawningDisabled { world_folder } => {
                            DetailJson::MobSpawningDisabled {
                                world_folder: rel(world_folder)?,
                            }
                        }
                        TrustDetail::PlayerInZeroChunk {
                            uuid,
                            dimension,
                            pos: p,
                        } => DetailJson::PlayerInZeroChunk {
                            uuid: uuid.clone(),
                            dimension: dimension.to_string(),
                            pos: pos(*p),
                        },
                    })
                })
                .collect::<Result<_, ReportError>>()?,
            None,
        ),
        CheckStatus::Unknown(reason) => (
            TrustStatus::Unknown,
            Vec::new(),
            Some(match reason {
                UnknownReason::UnreadableFile { path } => {
                    ReasonJson::UnreadableFile { path: rel(path)? }
                }
                UnknownReason::UnexpectedValue { path, key } => ReasonJson::UnexpectedValue {
                    path: rel(path)?,
                    key: key.clone(),
                },
                UnknownReason::UnknownVersion(v) => {
                    ReasonJson::UnknownVersion { data_version: v.0 }
                }
                UnknownReason::NoPlayersChecked => ReasonJson::NoPlayersChecked,
                UnknownReason::ForcedChunksUnknown(dims) => ReasonJson::ForcedChunksUnknown {
                    dimensions: dims.iter().map(ToString::to_string).collect(),
                },
            }),
        ),
    };
    Ok(TrustItem {
        id,
        status: name,
        details,
        reason,
    })
}

/// `path` relative to `root`, with `/` separators. `root` itself is `"."`.
/// Paths outside `root` (e.g. the server defaults of a world folder input)
/// are given as they are.
pub fn rel_path(root: &Path, path: &Path) -> Result<String, ReportError> {
    let non_utf8 = || ReportError::NonUtf8Path(path.to_path_buf());
    let Ok(relative) = path.strip_prefix(root) else {
        return path.to_str().map(str::to_owned).ok_or_else(non_utf8);
    };
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(s) => parts.push(s.to_str().ok_or_else(non_utf8)?),
            Component::CurDir => {}
            // A path relative to root has no prefix, root or parent components.
            other => parts.push(other.as_os_str().to_str().ok_or_else(non_utf8)?),
        }
    }
    if parts.is_empty() {
        return Ok(".".to_owned());
    }
    Ok(parts.join("/"))
}

#[cfg(test)]
mod tests;
