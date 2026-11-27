//! Building a [`Plan`] from the files on disk.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use rayon::prelude::*;
use wgc_core::plan::{analyze_triple, decide, estimate, group_externals, player_files};

use super::*;
use crate::layout::{self, Dimension, WorldLayout};
use crate::source::FsSource;
use crate::types::{ChunkPos, FileKind, Known, Observer, Progress};

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
        let externals = FileKind::ALL.map(|kind| group_externals(files, kind));
        let analyses = files
            .triples
            .par_iter()
            .map(|triple| {
                if cancel.load(Ordering::Relaxed) {
                    return Err(PlanError::Cancelled);
                }
                let analysis = analyze_triple(&FsSource, triple, &externals)?;
                let mut n = done.lock().unwrap_or_else(PoisonError::into_inner);
                *n += 1;
                observer.progress(Progress::Reading { done: *n, total });
                Ok(analysis)
            })
            .collect::<Result<_, _>>()?;

        let is_targeted = is_targeted(options.gc_dims.as_ref(), &dim.id);
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

    let players = player_files(&FsSource, layout)?;
    let lock_files: BTreeSet<PathBuf> = layout
        .world_folders
        .iter()
        .map(|f| f.join(crate::lock::LOCK_FILE))
        .collect();
    let walked = layout::world_files(layout)?;
    let estimate = estimate(
        listed.iter().map(|(_, files)| files),
        &dimensions,
        &players.omitted,
        walked.files.iter().map(|f| (&f.path, f.size)),
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

/// The InhabitedTime of `chunks` in `dim`, read exactly as [`build_plan`]
/// reads it but only from the region files that hold them. Chunks that do not
/// exist are left out.
pub(crate) fn read_inhabited(
    dim: &Dimension,
    chunks: &BTreeSet<ChunkPos>,
) -> Result<BTreeMap<ChunkPos, Inhabited>, PlanError> {
    if chunks.is_empty() {
        return Ok(BTreeMap::new());
    }
    let files = layout::list_dimension_files(dim)?;
    wgc_core::plan::read_inhabited(&FsSource, &files, chunks)
}

#[cfg(test)]
mod tests;
