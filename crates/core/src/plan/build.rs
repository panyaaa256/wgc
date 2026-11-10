//! Building a [`Plan`] (docs/spec/plan.md "処理の流れ").

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use rayon::prelude::*;

use super::*;
use crate::layout::{self, DimensionFiles, TripleFiles};
use crate::nbt::{ChunkSummary, read_chunk_summary};
use crate::region::{RegionFile, decompress};
use crate::types::{Known, Observer, Progress};

/// Reads the world and decides what to delete and how to write every file.
pub fn build_plan(
    layout: &WorldLayout,
    options: &PlanOptions,
    observer: &dyn Observer,
    cancel: &AtomicBool,
) -> Result<Plan, PlanError> {
    // Enumerate first so that progress has a total.
    let mut listed = Vec::new();
    for dim in &layout.dimensions {
        listed.push((dim, layout::list_dimension_files(dim)?));
    }
    let total: u64 = listed.iter().map(|(_, f)| f.triples.len() as u64).sum();
    // Counting and reporting under one lock keeps `done` increasing for the observer.
    let done = Mutex::new(0u64);
    observer.progress(Progress::Reading { done: 0, total });

    let mut dimensions = Vec::new();
    let mut protection_unknown = BTreeSet::new();
    for (dim, files) in &listed {
        let externals = [FileKind::Region, FileKind::Entities, FileKind::Poi]
            .map(|kind| externals_by_region(files, kind));
        // Stage A: per triple, in parallel.
        let mut analyses: Vec<Analysis> = files
            .triples
            .par_iter()
            .map(|triple| {
                if cancel.load(Ordering::Relaxed) {
                    return Err(PlanError::Cancelled);
                }
                let analysis = analyze(triple, &externals)?;
                let mut n = done.lock().unwrap_or_else(PoisonError::into_inner);
                *n += 1;
                observer.progress(Progress::Reading { done: *n, total });
                Ok(analysis)
            })
            .collect::<Result<_, _>>()?;
        analyses.sort_by_key(|a| a.pos);

        // Stage B: protections and decisions for the whole dimension.
        let is_targeted = super::is_targeted(options.gc_dims.as_ref(), &dim.id);
        let forced = if is_targeted {
            match layout::forced_chunks(layout, dim) {
                Known::Value(chunks) => chunks,
                Known::Unknown(_) => {
                    protection_unknown.insert(dim.id.clone());
                    BTreeSet::new()
                }
            }
        } else {
            BTreeSet::new()
        };
        dimensions.push(decide(dim, analyses, is_targeted, &forced));
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(PlanError::Cancelled);
    }

    let (omitted_other_files, dat_old_without_dat) = player_files(layout)?;
    let estimate = estimate(layout, &listed, &dimensions, &omitted_other_files)?;
    Ok(Plan {
        layout: layout.clone(),
        gc_dims: options.gc_dims.clone(),
        dimensions,
        omitted_other_files,
        protection_unknown,
        dat_old_without_dat,
        estimate,
    })
}

/// Result of stage A for one triple.
struct Analysis {
    pos: RegionPos,
    issues: Vec<StructuralIssue>,
    region: Option<FileInfo>,
    entities: Option<FileInfo>,
    poi: Option<FileInfo>,
    chunks: ChunkSummaries,
}

fn read(path: &Path) -> Result<Vec<u8>, PlanError> {
    fs::read(path).map_err(|source| PlanError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// `.mcc` files of `kind` grouped by region.
type Externals = BTreeMap<RegionPos, BTreeMap<ChunkPos, PathBuf>>;

fn externals_by_region(files: &DimensionFiles, kind: FileKind) -> Externals {
    let mut map = Externals::new();
    for (pos, path) in files.externals(kind) {
        map.entry(pos.region())
            .or_default()
            .insert(*pos, path.clone());
    }
    map
}

fn analyze(triple: &TripleFiles, externals_of: &[Externals; 3]) -> Result<Analysis, PlanError> {
    let mut analysis = Analysis {
        pos: triple.pos,
        issues: Vec::new(),
        region: None,
        entities: None,
        poi: None,
        chunks: Vec::new(),
    };
    for kind in FileKind::ALL {
        let Some(path) = triple.get(kind) else {
            continue;
        };
        let empty = BTreeMap::new();
        let externals = externals_of[kind as usize]
            .get(&triple.pos)
            .unwrap_or(&empty);
        let present: BTreeSet<ChunkPos> = externals.keys().copied().collect();
        let file = RegionFile::parse(kind, triple.pos, read(path)?, &present);
        analysis.issues.extend_from_slice(file.issues());

        if kind == FileKind::Region {
            analysis.chunks = chunk_summaries(&file, externals, |_| true)?;
        }

        let mut ext = Vec::new();
        for (pos, path) in externals {
            let size = fs::metadata(path)
                .map_err(|source| PlanError::Io {
                    path: path.to_path_buf(),
                    source,
                })?
                .len();
            ext.push(ExternalFile {
                pos: *pos,
                path: path.to_path_buf(),
                size,
            });
        }
        let info = FileInfo {
            path: path.to_path_buf(),
            size: file.size(),
            entries: file
                .entries()
                .iter()
                .map(|e| EntryInfo {
                    index: e.index,
                    length: e.length,
                    external: e.is_external(),
                })
                .collect(),
            externals: ext,
        };
        match kind {
            FileKind::Region => analysis.region = Some(info),
            FileKind::Entities => analysis.entities = Some(info),
            FileKind::Poi => analysis.poi = Some(info),
        }
    }
    analysis.issues.sort();
    Ok(analysis)
}

type ChunkSummaries = Vec<(ChunkIndex, ChunkPos, Result<ChunkSummary, ContentError>)>;

/// Reads the summary of every chunk of a parsed region file that `wanted` accepts.
fn chunk_summaries(
    file: &RegionFile,
    externals: &BTreeMap<ChunkPos, PathBuf>,
    wanted: impl Fn(ChunkPos) -> bool,
) -> Result<ChunkSummaries, PlanError> {
    let mut chunks = Vec::new();
    for entry in file.entries().iter().filter(|e| wanted(e.pos)) {
        // Every entry has a payload; `RegionFile::parse` lists no other.
        let Some(payload) = file.payload(entry.index) else {
            continue;
        };
        let external = match externals.get(&entry.pos) {
            Some(path) if entry.is_external() => Some(read(path)?),
            _ => None,
        };
        let summary = decompress(payload, external.as_deref())
            .map_err(ContentError::from)
            .and_then(|nbt| read_chunk_summary(&nbt).map_err(ContentError::from));
        chunks.push((entry.index, entry.pos, summary));
    }
    Ok(chunks)
}

/// The InhabitedTime of `chunks` in `dim`, read exactly as [`build_plan`]
/// reads it but only from the region files that hold them. Chunks that do not
/// exist are left out.
pub(crate) fn read_inhabited(
    dim: &Dimension,
    chunks: &BTreeSet<ChunkPos>,
) -> Result<BTreeMap<ChunkPos, Inhabited>, PlanError> {
    let regions: BTreeSet<RegionPos> = chunks.iter().map(|c| c.region()).collect();
    if regions.is_empty() {
        return Ok(BTreeMap::new());
    }
    let files = layout::list_dimension_files(dim)?;
    let externals = externals_by_region(&files, FileKind::Region);
    let empty = BTreeMap::new();
    let mut found = BTreeMap::new();
    for triple in files.triples.iter().filter(|t| regions.contains(&t.pos)) {
        let Some(path) = &triple.region else {
            continue;
        };
        let externals = externals.get(&triple.pos).unwrap_or(&empty);
        let present: BTreeSet<ChunkPos> = externals.keys().copied().collect();
        let file = RegionFile::parse(FileKind::Region, triple.pos, read(path)?, &present);
        for (_, pos, summary) in chunk_summaries(&file, externals, |pos| chunks.contains(&pos))? {
            found.insert(pos, inhabited(&summary));
        }
    }
    Ok(found)
}

fn inhabited(summary: &Result<ChunkSummary, ContentError>) -> Inhabited {
    match summary {
        Err(e) => Inhabited::Unreadable(e.clone()),
        Ok(s) => match s.inhabited_time {
            None => Inhabited::Missing,
            Some(v) if v < 0 => Inhabited::Negative(v),
            Some(v) => Inhabited::Value(v),
        },
    }
}

/// Chebyshev distance of the forceload protection around each forced chunk.
const FORCELOAD_MARGIN: i32 = 2;

fn decide(
    dim: &Dimension,
    analyses: Vec<Analysis>,
    is_targeted: bool,
    forced: &BTreeSet<ChunkPos>,
) -> DimensionPlan {
    // C: candidates.
    let mut candidates = BTreeSet::new();
    for a in &analyses {
        for (_, pos, summary) in &a.chunks {
            if is_targeted && a.issues.is_empty() && inhabited(summary) == Inhabited::Value(0) {
                candidates.insert(*pos);
            }
        }
    }
    // F: forced chunks and their margin.
    let mut forceload = BTreeSet::new();
    for c in forced {
        for dx in -FORCELOAD_MARGIN..=FORCELOAD_MARGIN {
            for dz in -FORCELOAD_MARGIN..=FORCELOAD_MARGIN {
                if let (Some(x), Some(z)) = (c.x.checked_add(dx), c.z.checked_add(dz)) {
                    forceload.insert(ChunkPos::new(x, z));
                }
            }
        }
    }
    // R: structure starts referenced by chunks that are kept for any reason
    // other than a structure reference (not in C, or kept by F). One pass.
    let mut referenced = BTreeSet::new();
    for a in &analyses {
        for (_, pos, summary) in &a.chunks {
            if let Ok(s) = summary
                && (!candidates.contains(pos) || forceload.contains(pos))
            {
                referenced.extend(s.structure_refs.iter().copied());
            }
        }
    }

    let mut deleted = BTreeSet::new();
    let mut triples = Vec::new();
    for a in analyses {
        let verbatim = !a.issues.is_empty();
        let chunks = a
            .chunks
            .into_iter()
            .map(|(index, pos, summary)| {
                let inhabited = inhabited(&summary);
                let decision = if verbatim {
                    Decision::Keep(KeepReason::TripleVerbatim)
                } else if !is_targeted {
                    Decision::Keep(KeepReason::NotTargeted)
                } else {
                    match &inhabited {
                        Inhabited::Unreadable(_) => Decision::Keep(KeepReason::ContentError),
                        Inhabited::Missing | Inhabited::Negative(_) => {
                            Decision::Keep(KeepReason::InhabitedUnknown)
                        }
                        Inhabited::Value(v) if *v > 0 => Decision::Keep(KeepReason::Inhabited),
                        Inhabited::Value(_) if forceload.contains(&pos) => {
                            Decision::Keep(KeepReason::Forceload)
                        }
                        Inhabited::Value(_) if referenced.contains(&pos) => {
                            Decision::Keep(KeepReason::StructureRef)
                        }
                        Inhabited::Value(_) => Decision::Delete,
                    }
                };
                if decision == Decision::Delete {
                    deleted.insert(pos);
                }
                let (data_version, has_blending_data) = match &summary {
                    Ok(s) => (s.data_version, s.has_blending_data),
                    Err(_) => (None, false),
                };
                ChunkDecision {
                    index,
                    pos,
                    inhabited,
                    data_version,
                    has_blending_data,
                    decision,
                }
            })
            .collect();
        triples.push(TriplePlan {
            pos: a.pos,
            issues: a.issues,
            region: a.region,
            entities: a.entities,
            poi: a.poi,
            chunks,
        });
    }
    let boundary_warnings = boundary(&triples, &deleted);
    DimensionPlan {
        dimension: dim.clone(),
        triples,
        boundary_warnings,
    }
}

/// Kept chunks with `blending_data` next to a deleted chunk, grouped by 8-connectivity.
fn boundary(triples: &[TriplePlan], deleted: &BTreeSet<ChunkPos>) -> Vec<BoundaryCluster> {
    let neighbors4 = |p: ChunkPos| {
        [(1, 0), (-1, 0), (0, 1), (0, -1)]
            .into_iter()
            .filter_map(move |(dx, dz)| {
                Some(ChunkPos::new(p.x.checked_add(dx)?, p.z.checked_add(dz)?))
            })
    };
    let mut marked: BTreeSet<ChunkPos> = triples
        .iter()
        .flat_map(|t| &t.chunks)
        .filter(|c| c.has_blending_data && c.decision != Decision::Delete)
        .filter(|c| neighbors4(c.pos).any(|n| deleted.contains(&n)))
        .map(|c| c.pos)
        .collect();

    let mut clusters = Vec::new();
    while let Some(start) = marked.pop_first() {
        let mut cluster = BoundaryCluster {
            chunks: 0,
            min: start,
            max: start,
        };
        let mut queue = VecDeque::from([start]);
        while let Some(p) = queue.pop_front() {
            cluster.chunks += 1;
            cluster.min = ChunkPos::new(cluster.min.x.min(p.x), cluster.min.z.min(p.z));
            cluster.max = ChunkPos::new(cluster.max.x.max(p.x), cluster.max.z.max(p.z));
            for dx in -1..=1 {
                for dz in -1..=1 {
                    if let (Some(x), Some(z)) = (p.x.checked_add(dx), p.z.checked_add(dz))
                        && marked.remove(&ChunkPos::new(x, z))
                    {
                        queue.push_back(ChunkPos::new(x, z));
                    }
                }
            }
        }
        clusters.push(cluster);
    }
    clusters.sort();
    clusters
}

/// Player temp files to omit, and `.dat_old` files without a `.dat`.
fn player_files(layout: &WorldLayout) -> Result<(Vec<OmittedFile>, Vec<PathBuf>), PlanError> {
    let dir = layout::player_data_dir(layout);
    let mut omitted = Vec::new();
    let mut dat_old = Vec::new();
    for entry in layout::read_dir_opt(&dir)? {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if is_player_temp_file(&name) {
            omitted.push(OmittedFile {
                path,
                reason: OmitReason::PlayerTempFile,
            });
        } else if let Some(stem) = name.strip_suffix(".dat_old")
            && !dir.join(format!("{stem}.dat")).exists()
        {
            dat_old.push(path);
        }
    }
    omitted.sort_by(|a, b| a.path.cmp(&b.path));
    dat_old.sort();
    Ok((omitted, dat_old))
}

/// `^[0-9a-f-]{36}-\d+\.dat$`
fn is_player_temp_file(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".dat") else {
        return false;
    };
    if stem.len() < 38 || !stem.is_char_boundary(36) {
        return false;
    }
    let (uuid, rest) = stem.split_at(36);
    let Some(digits) = rest.strip_prefix('-') else {
        return false;
    };
    uuid.bytes()
        .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f' | b'-'))
        && !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
}

fn estimate(
    layout: &WorldLayout,
    listed: &[(&Dimension, DimensionFiles)],
    dimensions: &[DimensionPlan],
    omitted: &[OmittedFile],
) -> Result<Estimate, PlanError> {
    let mut est = Estimate::default();
    for dim in dimensions {
        let mut sizes = KindSizes::default();
        for t in &dim.triples {
            sizes.region += t.sizes(FileKind::Region);
            sizes.entities += t.sizes(FileKind::Entities);
            sizes.poi += t.sizes(FileKind::Poi);
        }
        est.total_output_bytes += sizes.region.after + sizes.entities.after + sizes.poi.after;
        est.dimensions.insert(dim.dimension.id.clone(), sizes);
    }

    // Everything else under the world folders is copied as it is.
    // Files the plan accounts for: triple files and the .mcc files attached to
    // them. A .mcc without its region file stays an ordinary file.
    let mut managed: BTreeSet<&Path> = BTreeSet::new();
    for (_, files) in listed {
        for t in &files.triples {
            managed.extend(FileKind::ALL.into_iter().filter_map(|k| t.get(k)));
        }
    }
    for dim in dimensions {
        for t in &dim.triples {
            for kind in FileKind::ALL {
                if let Some(file) = t.file(kind) {
                    managed.extend(file.externals.iter().map(|x| x.path.as_path()));
                }
            }
        }
    }
    managed.extend(omitted.iter().map(|o| o.path.as_path()));
    let lock_files: BTreeSet<PathBuf> = layout
        .world_folders
        .iter()
        .map(|f| f.join(crate::lock::LOCK_FILE))
        .collect();
    let walked = layout::world_files(layout)?;
    est.other_files_bytes = walked
        .files
        .iter()
        .filter(|f| !managed.contains(f.path.as_path()))
        .filter(|f| !lock_files.contains(&f.path))
        .map(|f| f.size)
        .sum();
    est.total_output_bytes += est.other_files_bytes;
    Ok(est)
}

#[cfg(test)]
mod tests;
