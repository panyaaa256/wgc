//! Building the parts of a [`Plan`] from the world's files.

use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::format;
use alloc::vec::Vec;

use super::*;
use crate::layout::{self, DimensionFiles, TripleFiles};
use crate::nbt::{ChunkSummary, read_chunk_summary};
use crate::region::{Decompressor, RegionFile};
use crate::source::WorldSource;

/// What one region triple holds, before any decision. Built by
/// [`analyze_triple`], one triple at a time and in any order; [`decide`]
/// combines them.
pub struct Analysis<P> {
    pos: RegionPos,
    issues: Vec<StructuralIssue>,
    files: PerKind<Option<FileInfo<P>>>,
    chunks: ChunkSummaries,
}

/// `.mcc` files of one kind, grouped by region.
pub type Externals<P> = BTreeMap<RegionPos, BTreeMap<ChunkPos, P>>;

/// The `.mcc` files of `kind` in `files`, grouped by region.
pub fn group_externals<P: Clone>(files: &DimensionFiles<P>, kind: FileKind) -> Externals<P> {
    let mut map = Externals::new();
    for (pos, path) in &files.externals[kind] {
        map.entry(pos.region())
            .or_default()
            .insert(*pos, path.clone());
    }
    map
}

/// Reads and parses the files of one triple. `externals_of` holds the `.mcc`
/// files of each [`FileKind`].
pub fn analyze_triple<S: WorldSource>(
    source: &S,
    triple: &TripleFiles<S::Path>,
    externals_of: &PerKind<Externals<S::Path>>,
) -> Result<Analysis<S::Path>, S::Error> {
    let mut analysis = Analysis {
        pos: triple.pos,
        issues: Vec::new(),
        files: PerKind::default(),
        chunks: Vec::new(),
    };
    let empty = BTreeMap::new();
    for kind in FileKind::ALL {
        let Some(path) = triple.get(kind) else {
            continue;
        };
        let externals = externals_of[kind].get(&triple.pos).unwrap_or(&empty);
        let file = read_region_file(source, kind, triple.pos, path, externals)?;
        analysis.issues.extend_from_slice(file.issues());

        if kind == FileKind::Region {
            analysis.chunks = chunk_summaries(source, &file, externals, |_| true)?;
        }

        let externals = externals
            .iter()
            .map(|(pos, path)| {
                Ok(ExternalFile {
                    pos: *pos,
                    path: path.clone(),
                    size: source.size(path)?,
                })
            })
            .collect::<Result<_, S::Error>>()?;
        let info = FileInfo {
            path: path.clone(),
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
            externals,
        };
        analysis.files[kind] = Some(info);
    }
    analysis.issues.sort();
    Ok(analysis)
}

/// Reads and parses the file of `kind` at `pos`; `externals` are the `.mcc`
/// files of its kind in its region.
fn read_region_file<S: WorldSource>(
    source: &S,
    kind: FileKind,
    pos: RegionPos,
    path: &S::Path,
    externals: &BTreeMap<ChunkPos, S::Path>,
) -> Result<RegionFile, S::Error> {
    let present: BTreeSet<ChunkPos> = externals.keys().copied().collect();
    Ok(RegionFile::parse(kind, pos, source.read(path)?, &present))
}

type ChunkSummaries = Vec<(ChunkIndex, ChunkPos, Result<ChunkSummary, ContentError>)>;

/// Reads the summary of every chunk of a parsed region file that `wanted` accepts.
fn chunk_summaries<S: WorldSource>(
    source: &S,
    file: &RegionFile,
    externals: &BTreeMap<ChunkPos, S::Path>,
    wanted: impl Fn(ChunkPos) -> bool,
) -> Result<ChunkSummaries, S::Error> {
    let mut chunks = Vec::new();
    let mut decompressor = Decompressor::new();
    for entry in file.entries().iter().filter(|e| wanted(e.pos)) {
        // Every entry has a payload; `RegionFile::parse` lists no other.
        let Some(payload) = file.payload(entry.index) else {
            continue;
        };
        let external = match externals.get(&entry.pos) {
            Some(path) if entry.is_external() => Some(source.read(path)?),
            _ => None,
        };
        let summary = decompressor
            .decompress(payload, external.as_deref())
            .map_err(ContentError::from)
            .and_then(|nbt| read_chunk_summary(nbt).map_err(ContentError::from));
        chunks.push((entry.index, entry.pos, summary));
    }
    Ok(chunks)
}

/// The InhabitedTime of `chunks`, read exactly as a plan reads it but only
/// from the region files of `files` that hold them. Chunks that do not exist
/// are left out.
pub fn read_inhabited<S: WorldSource>(
    source: &S,
    files: &DimensionFiles<S::Path>,
    chunks: &BTreeSet<ChunkPos>,
) -> Result<BTreeMap<ChunkPos, Inhabited>, S::Error> {
    let regions: BTreeSet<RegionPos> = chunks.iter().map(|c| c.region()).collect();
    let externals = group_externals(files, FileKind::Region);
    let empty = BTreeMap::new();
    let mut found = BTreeMap::new();
    for triple in files.triples.iter().filter(|t| regions.contains(&t.pos)) {
        let Some(path) = &triple.files.region else {
            continue;
        };
        let externals = externals.get(&triple.pos).unwrap_or(&empty);
        let file = read_region_file(source, FileKind::Region, triple.pos, path, externals)?;
        let summaries = chunk_summaries(source, &file, externals, |pos| chunks.contains(&pos))?;
        for (_, pos, summary) in summaries {
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

/// The forced chunks whose surroundings are protected from deletion in
/// `dim`: none when GC does not target it, so nothing has to be read.
pub fn forceload_protection<S: WorldSource>(
    source: &S,
    layout: &WorldLayout<S::Path>,
    dim: &Dimension<S::Path>,
    is_targeted: bool,
) -> Known<BTreeSet<ChunkPos>, S::Path> {
    if is_targeted {
        layout::forced_chunks(source, layout, dim)
    } else {
        Known::Value(BTreeSet::new())
    }
}

/// Chebyshev distance of the forceload protection around each forced chunk.
const FORCELOAD_MARGIN: i32 = 2;

/// Decides the fate of every chunk of `dim` from the analyses of all its
/// triples, given in any order.
pub fn decide<P: Clone>(
    dim: &Dimension<P>,
    mut analyses: Vec<Analysis<P>>,
    is_targeted: bool,
    forced: &BTreeSet<ChunkPos>,
) -> DimensionPlan<P> {
    // The order of analysis must not change the plan.
    analyses.sort_by_key(|a| a.pos);
    // Deletion candidates: InhabitedTime 0 in a targeted, non-verbatim triple.
    let mut candidates = BTreeSet::new();
    for a in &analyses {
        for (_, pos, summary) in &a.chunks {
            if is_targeted && a.issues.is_empty() && inhabited(summary) == Inhabited::Value(0) {
                candidates.insert(*pos);
            }
        }
    }
    // Forceload protection: forced chunks and their margin.
    let mut forceload = BTreeSet::new();
    for c in forced {
        for dx in -FORCELOAD_MARGIN..=FORCELOAD_MARGIN {
            for dz in -FORCELOAD_MARGIN..=FORCELOAD_MARGIN {
                forceload.extend(c.offset(dx, dz));
            }
        }
    }
    // Structure reference protection: structure starts referenced by chunks
    // kept for any reason other than a structure reference (not a candidate,
    // or forceload-protected). One pass: chunks kept only because they are
    // referenced do not protect what they reference.
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
            files: a.files,
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
fn boundary<P>(triples: &[TriplePlan<P>], deleted: &BTreeSet<ChunkPos>) -> Vec<BoundaryCluster> {
    let neighbors4 = |p: ChunkPos| {
        [(1, 0), (-1, 0), (0, 1), (0, -1)]
            .into_iter()
            .filter_map(move |(dx, dz)| p.offset(dx, dz))
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
                    if let Some(n) = p.offset(dx, dz)
                        && marked.remove(&n)
                    {
                        queue.push_back(n);
                    }
                }
            }
        }
        clusters.push(cluster);
    }
    clusters.sort();
    clusters
}

/// Files of the player data folder that need attention.
pub struct PlayerFiles<P> {
    /// Temp files the game leaves behind, sorted by path.
    pub omitted: Vec<OmittedFile<P>>,
    /// `.dat_old` files without a `.dat`, sorted.
    pub dat_old_without_dat: Vec<P>,
}

/// Finds the [`PlayerFiles`]. Entries whose name is not UTF-8 are ignored.
pub fn player_files<S: WorldSource>(
    source: &S,
    layout: &WorldLayout<S::Path>,
) -> Result<PlayerFiles<S::Path>, S::Error> {
    let dir = layout::player_data_dir(source, layout);
    let mut omitted = Vec::new();
    let mut dat_old = Vec::new();
    for entry in source.list(&dir)? {
        if !entry.is_file {
            continue;
        }
        let Some(name) = source.name(&entry.path) else {
            continue;
        };
        if is_player_temp_file(name) {
            omitted.push(OmittedFile {
                path: entry.path,
                reason: OmitReason::PlayerTempFile,
            });
        } else if name
            .strip_suffix(".dat_old")
            .is_some_and(|stem| !source.exists(&source.join(&dir, &format!("{stem}.dat"))))
        {
            dat_old.push(entry.path);
        }
    }
    omitted.sort_by(|a, b| a.path.cmp(&b.path));
    dat_old.sort();
    Ok(PlayerFiles {
        omitted,
        dat_old_without_dat: dat_old,
    })
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

/// The sizes before and after GC, and the total the output needs.
///
/// `listed` holds the triples of every dimension, and `copied_files` every
/// file under the world folders that may be written to the output, with its
/// size. Every such file that is not part of a triple, an attached `.mcc` file
/// or an omitted file is copied as it is.
pub fn estimate<'a, P: Ord + 'a>(
    listed: impl IntoIterator<Item = &'a DimensionFiles<P>>,
    dimensions: &[DimensionPlan<P>],
    omitted: &[OmittedFile<P>],
    copied_files: impl IntoIterator<Item = (&'a P, u64)>,
) -> Estimate {
    let mut est = Estimate::default();
    for dim in dimensions {
        let mut sizes = KindSizes::default();
        for t in &dim.triples {
            sizes += PerKind::from_fn(|kind| t.sizes(kind));
        }
        est.total_output_bytes += sizes.map(|s| s.after).total();
        est.dimensions.insert(dim.dimension.id.clone(), sizes);
    }

    // A .mcc without its region file stays an ordinary file.
    let mut managed: BTreeSet<&P> = BTreeSet::new();
    for files in listed {
        for t in &files.triples {
            managed.extend(FileKind::ALL.into_iter().filter_map(|k| t.get(k)));
        }
    }
    for dim in dimensions {
        for t in &dim.triples {
            for kind in FileKind::ALL {
                if let Some(file) = t.file(kind) {
                    managed.extend(file.externals.iter().map(|x| &x.path));
                }
            }
        }
    }
    managed.extend(omitted.iter().map(|o| &o.path));
    est.other_files_bytes = copied_files
        .into_iter()
        .filter(|(path, _)| !managed.contains(path))
        .map(|(_, size)| size)
        .sum();
    est.total_output_bytes += est.other_files_bytes;
    est
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_temp_file_pattern() {
        assert!(is_player_temp_file(
            "0123abcd-0000-0000-0000-00000000beef-1.dat"
        ));
        assert!(!is_player_temp_file(
            "0123abcd-0000-0000-0000-00000000beef.dat"
        ));
        assert!(!is_player_temp_file(
            "0123ABCD-0000-0000-0000-00000000beef-1.dat"
        ));
        assert!(!is_player_temp_file(
            "0123abcd-0000-0000-0000-00000000beef-.dat"
        ));
        assert!(!is_player_temp_file(
            "0123abcd-0000-0000-0000-00000000beef-1.dat_old"
        ));
    }
}
