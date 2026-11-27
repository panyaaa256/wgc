//! Building a [`Plan`] from a [`World`].

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use wgc_core::layout::{LOCK_FILE, forced_chunks};
use wgc_core::plan::{
    Inhabited, Plan, PlanOptions, analyze_triple, decide, estimate, group_externals, is_targeted,
    player_files,
};
use wgc_core::source::WorldSource;
use wgc_core::trust::{ChunkKey, InhabitedMap, TrustReport, check_trust_before_plan};
use wgc_core::types::{ChunkPos, DimensionId, FileKind, Known, UnknownReason};

use crate::host::{PathOf, Runner, World};
use crate::observe::{Observer, Progress, Warning};

/// Why a plan could not be built.
#[derive(Debug)]
pub enum PlanError<E> {
    World(E),
    Cancelled,
}

impl<E> From<E> for PlanError<E> {
    fn from(e: E) -> Self {
        PlanError::World(e)
    }
}

/// The observer the commands of a world report to.
pub type ObserverOf<'a, W> = &'a dyn Observer<Warning<PathOf<W>, <W as World>::LockError>>;

/// Reads the world and decides what to delete and how to write every file.
/// Region triples are read through `runner`; the plan does not depend on the
/// order in which they finish.
pub fn build_plan<W: World>(
    world: &W,
    runner: &impl Runner,
    options: &PlanOptions,
    observer: ObserverOf<'_, W>,
    cancel: &AtomicBool,
) -> Result<Plan<PathOf<W>>, PlanError<W::Error>> {
    let layout = world.layout();
    let source = world.source();
    // Enumerate first so that progress has a total.
    let listed = layout
        .dimensions
        .iter()
        .map(|dim| Ok((dim, world.list_dimension(dim)?)))
        .collect::<Result<Vec<_>, W::Error>>()?;
    let total: u64 = listed.iter().map(|(_, f)| f.triples.len() as u64).sum();
    observer.progress(Progress::Reading { done: 0, total });

    let mut dimensions = Vec::new();
    let mut protection_unknown = BTreeSet::new();
    let mut finished = 0;
    for (dim, files) in &listed {
        let externals = FileKind::ALL.map(|kind| group_externals(files, kind));
        let analyses = runner.try_map(
            &files.triples,
            |triple| {
                if cancel.load(Ordering::Relaxed) {
                    return Err(PlanError::Cancelled);
                }
                analyze_triple(source, triple, &externals)
                    .map_err(|e| PlanError::World(W::Error::from(e)))
            },
            |done| {
                observer.progress(Progress::Reading {
                    done: finished + done,
                    total,
                });
            },
        )?;
        finished += files.triples.len() as u64;

        let is_targeted = is_targeted(options.gc_dims.as_ref(), &dim.id);
        let forced = if is_targeted {
            match forced_chunks(source, layout, dim) {
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

    let players = player_files(source, layout).map_err(W::Error::from)?;
    let lock_files: BTreeSet<PathOf<W>> = layout
        .world_folders
        .iter()
        .map(|folder| source.join(folder, LOCK_FILE))
        .collect();
    let files = world.files()?;
    let estimate = estimate(
        listed.iter().map(|(_, files)| files),
        &dimensions,
        &players.omitted,
        files.iter().map(|(path, size)| (path, *size)),
        &lock_files,
    );
    Ok(Plan {
        layout: layout.clone(),
        gc_dims: options.gc_dims.clone(),
        dimensions,
        omitted_other_files: players.omitted,
        protection_unknown,
        dat_old_without_dat: players.dat_old_without_dat,
        estimate,
    })
}

/// Runs T-1..T-5 without a plan, so that `gc --out` can stop before reading
/// every region file. T-3 reads only the region files that hold the chunks
/// players stand in, and T-5 reads the forced chunks directly.
///
/// With `skip_t3`, T-3 reads nothing and is left out of the report: when
/// `--force` allows it, it cannot stop the run, and the check against the plan
/// decides whether it was forced.
pub fn trust_before_plan<W: World>(
    world: &W,
    gc_dims: Option<&BTreeSet<DimensionId>>,
    skip_t3: bool,
) -> TrustReport<PathOf<W>> {
    let inhabited_of =
        (!skip_t3).then_some(|wanted: &BTreeSet<ChunkKey>| inhabited_of(world, wanted));
    check_trust_before_plan(world.source(), world.layout(), gc_dims, inhabited_of)
}

/// The InhabitedTime of `chunks` in `dim`, read exactly as [`build_plan`]
/// reads it but only from the region files that hold them. Chunks that do not
/// exist are left out.
pub fn read_inhabited<W: World>(
    world: &W,
    dim: &wgc_core::layout::Dimension<PathOf<W>>,
    chunks: &BTreeSet<ChunkPos>,
) -> Result<BTreeMap<ChunkPos, Inhabited>, W::Error> {
    if chunks.is_empty() {
        return Ok(BTreeMap::new());
    }
    let files = world.list_dimension(dim)?;
    Ok(wgc_core::plan::read_inhabited(
        world.source(),
        &files,
        chunks,
    )?)
}

/// Reads the chunks from the region files, one dimension at a time. A file
/// that cannot be read makes the result unknown.
fn inhabited_of<W: World>(
    world: &W,
    wanted: &BTreeSet<ChunkKey>,
) -> Result<InhabitedMap, UnknownReason<PathOf<W>>> {
    let mut found = BTreeMap::new();
    for dim in &world.layout().dimensions {
        let chunks: BTreeSet<ChunkPos> = wanted
            .iter()
            .filter(|(id, _)| *id == dim.id)
            .map(|(_, pos)| *pos)
            .collect();
        let inhabited =
            read_inhabited(world, dim, &chunks).map_err(|e| UnknownReason::UnreadableFile {
                path: world.error_path(&e).unwrap_or_else(|| dim.dir.clone()),
            })?;
        found.extend(
            inhabited
                .into_iter()
                .map(|(pos, inhabited)| ((dim.id.clone(), pos), inhabited)),
        );
    }
    Ok(found)
}
