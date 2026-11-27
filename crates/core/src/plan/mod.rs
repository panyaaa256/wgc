//! The plan: what to delete and how to write each file.
//!
//! The only source of truth for keeping a chunk is [`ChunkDecision::decision`];
//! file actions and entry fates are derived from it by [`TriplePlan`]'s methods.
//! `P` names files and folders.

mod build;

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use crate::layout::{Dimension, WorldLayout};
use crate::region::{
    ContentError, HEADER_BYTES, RegionFile, SECTOR_BYTES, StructuralIssue, packed_sectors,
};
use crate::types::{
    ChunkIndex, ChunkPos, DataVersion, DimensionId, FileKind, Known, PerKind, RegionPos,
};

pub use build::{
    Analysis, Externals, PlayerFiles, analyze_triple, decide, estimate, forceload_protection,
    group_externals, player_files, read_inhabited,
};

/// Options for building a [`Plan`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlanOptions {
    /// Delete chunks only in these dimensions. `None` means all.
    pub gc_dims: Option<BTreeSet<DimensionId>>,
}

/// The InhabitedTime of a chunk as the plan sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inhabited {
    /// Zero or more ticks.
    Value(i64),
    /// The tag is missing or has another type.
    Missing,
    /// A negative value; treated as unknown.
    Negative(i64),
    Unreadable(ContentError),
}

/// Why a chunk is kept. When several reasons apply, the first variant listed is recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KeepReason {
    TripleVerbatim,
    NotTargeted,
    ContentError,
    InhabitedUnknown,
    Inhabited,
    Forceload,
    StructureRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Decision {
    Delete,
    Keep(KeepReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileAction {
    Verbatim,
    Rewrite { keep: Vec<ChunkIndex> },
    Omit(OmitReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OmitReason {
    EmptyFile,
    NoEntriesLeft,
    PlayerTempFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EntryFate {
    Keep,
    Remove(RemoveReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RemoveReason {
    GcDeleted,
    Orphan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExternalAction {
    Copy,
    Omit,
}

/// One region-format file of a triple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInfo<P> {
    pub path: P,
    pub size: u64,
    /// Sorted by index.
    pub entries: Vec<EntryInfo>,
    /// `.mcc` files of this kind in this region, sorted by position.
    pub externals: Vec<ExternalFile<P>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryInfo {
    pub index: ChunkIndex,
    /// The length field of the payload.
    pub length: u32,
    pub external: bool,
}

impl EntryInfo {
    fn packed_bytes(&self) -> u64 {
        (packed_sectors(self.length) * SECTOR_BYTES) as u64
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalFile<P> {
    pub pos: ChunkPos,
    pub path: P,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkDecision {
    pub index: ChunkIndex,
    pub pos: ChunkPos,
    pub inhabited: Inhabited,
    pub data_version: Option<DataVersion>,
    pub has_blending_data: bool,
    pub decision: Decision,
}

/// The plan for one region triple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriplePlan<P> {
    pub pos: RegionPos,
    /// Not empty means the whole triple is copied verbatim.
    pub issues: Vec<StructuralIssue>,
    pub files: PerKind<Option<FileInfo<P>>>,
    /// Chunks of the region file, sorted by index.
    pub chunks: Vec<ChunkDecision>,
}

impl<P> TriplePlan<P> {
    pub fn is_verbatim(&self) -> bool {
        !self.issues.is_empty()
    }

    pub fn file(&self, kind: FileKind) -> Option<&FileInfo<P>> {
        self.files[kind].as_ref()
    }

    fn region_decision(&self, index: ChunkIndex) -> Option<Decision> {
        self.chunks
            .binary_search_by_key(&index, |c| c.index)
            .ok()
            .map(|i| self.chunks[i].decision)
    }

    /// Parses `bytes` as the file of `kind` of this triple, with the `.mcc`
    /// files the plan found next to it.
    pub fn parse_file(&self, kind: FileKind, bytes: Vec<u8>) -> RegionFile {
        let externals: BTreeSet<ChunkPos> = self
            .file(kind)
            .map(|f| f.externals.iter().map(|x| x.pos).collect())
            .unwrap_or_default();
        RegionFile::parse(kind, self.pos, bytes, &externals)
    }

    /// What happens to the entry at `index` in the file of `kind`.
    pub fn entry_fate(&self, kind: FileKind, index: ChunkIndex) -> EntryFate {
        if self.is_verbatim() {
            return EntryFate::Keep;
        }
        match (kind, self.region_decision(index)) {
            (_, Some(Decision::Delete)) => EntryFate::Remove(RemoveReason::GcDeleted),
            (_, Some(Decision::Keep(_))) => EntryFate::Keep,
            // The region file can be read and has no chunk here.
            (FileKind::Entities | FileKind::Poi, None) => EntryFate::Remove(RemoveReason::Orphan),
            (FileKind::Region, None) => EntryFate::Keep,
        }
    }

    /// How the file of `kind` is written. `None` if the triple has no such file.
    pub fn action(&self, kind: FileKind) -> Option<FileAction> {
        let file = self.file(kind)?;
        if file.size == 0 {
            return Some(FileAction::Omit(OmitReason::EmptyFile));
        }
        if self.is_verbatim() {
            return Some(FileAction::Verbatim);
        }
        let keep: Vec<ChunkIndex> = file
            .entries
            .iter()
            .filter(|e| self.entry_fate(kind, e.index) == EntryFate::Keep)
            .map(|e| e.index)
            .collect();
        Some(if keep.len() == file.entries.len() {
            FileAction::Verbatim
        } else if keep.is_empty() {
            FileAction::Omit(OmitReason::NoEntriesLeft)
        } else {
            FileAction::Rewrite { keep }
        })
    }

    /// Whether the `.mcc` file of `kind` for `pos` is copied.
    pub fn external_action(&self, kind: FileKind, pos: ChunkPos) -> ExternalAction {
        let referenced_by_removed = self.file(kind).is_some_and(|file| {
            file.entries.iter().any(|e| {
                e.external
                    && e.index == pos.index()
                    && pos.region() == self.pos
                    && self.entry_fate(kind, e.index) != EntryFate::Keep
            })
        });
        if referenced_by_removed && !self.is_verbatim() {
            ExternalAction::Omit
        } else {
            ExternalAction::Copy
        }
    }

    /// Size of the file of `kind` and its `.mcc` files now and after writing.
    pub fn sizes(&self, kind: FileKind) -> Sizes {
        let Some(file) = self.file(kind) else {
            return Sizes::default();
        };
        let externals_before: u64 = file.externals.iter().map(|x| x.size).sum();
        let externals_after: u64 = file
            .externals
            .iter()
            .filter(|x| self.external_action(kind, x.pos) == ExternalAction::Copy)
            .map(|x| x.size)
            .sum();
        let after = match self.action(kind) {
            Some(FileAction::Verbatim) => file.size,
            Some(FileAction::Rewrite { keep }) => {
                HEADER_BYTES as u64
                    + file
                        .entries
                        .iter()
                        .filter(|e| keep.binary_search(&e.index).is_ok())
                        .map(EntryInfo::packed_bytes)
                        .sum::<u64>()
            }
            Some(FileAction::Omit(_)) | None => 0,
        };
        Sizes {
            before: file.size + externals_before,
            after: after + externals_after,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OmittedFile<P> {
    pub path: P,
    pub reason: OmitReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct BoundaryCluster {
    pub chunks: u32,
    pub min: ChunkPos,
    pub max: ChunkPos,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DimensionPlan<P> {
    pub dimension: Dimension<P>,
    /// Sorted by position.
    pub triples: Vec<TriplePlan<P>>,
    pub boundary_warnings: Vec<BoundaryCluster>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sizes {
    pub before: u64,
    pub after: u64,
}

impl core::ops::AddAssign for Sizes {
    fn add_assign(&mut self, o: Sizes) {
        self.before += o.before;
        self.after += o.after;
    }
}

pub type KindSizes = PerKind<Sizes>;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Estimate {
    pub dimensions: BTreeMap<DimensionId, KindSizes>,
    /// Files outside the region folders that are copied as they are.
    pub other_files_bytes: u64,
    /// Everything written to the output.
    pub total_output_bytes: u64,
}

/// The plan for a whole world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan<P> {
    pub layout: WorldLayout<P>,
    pub gc_dims: Option<BTreeSet<DimensionId>>,
    /// Sorted by dimension ID.
    pub dimensions: Vec<DimensionPlan<P>>,
    /// Sorted by path.
    pub omitted_other_files: Vec<OmittedFile<P>>,
    /// Dimensions whose forced chunks could not be read.
    pub protection_unknown: BTreeSet<DimensionId>,
    pub dat_old_without_dat: Vec<P>,
    pub estimate: Estimate,
}

impl<P> Plan<P> {
    /// Whether GC deletes chunks in `dim`.
    pub fn is_targeted(&self, dim: &DimensionId) -> bool {
        is_targeted(self.gc_dims.as_ref(), dim)
    }
}

pub fn is_targeted(gc_dims: Option<&BTreeSet<DimensionId>>, dim: &DimensionId) -> bool {
    gc_dims.is_none_or(|dims| dims.contains(dim))
}
