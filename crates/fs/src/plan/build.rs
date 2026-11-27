//! Building a [`Plan`] from the files on disk.

use std::sync::atomic::AtomicBool;

use super::*;
use crate::layout::WorldLayout;
use crate::types::{Observer, Warning};
use crate::world::{FsWorld, Rayon};

/// Reads the world and decides what to delete and how to write every file.
pub fn build_plan(
    layout: &WorldLayout,
    options: &PlanOptions,
    observer: &dyn Observer<Warning>,
    cancel: &AtomicBool,
) -> Result<Plan, PlanError> {
    wgc::plan::build_plan(&FsWorld::new(layout), &Rayon, options, observer, cancel)
        .map_err(PlanError::from)
}

#[cfg(test)]
mod tests;
