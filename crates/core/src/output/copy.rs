//! Step 7: writing the output (docs/spec/output.md "書き出し").

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use super::{OutputError, destination, io_err, is_excluded, sync_dir};
use crate::layout::{WorldFile, WorldFiles};
use crate::plan::{ExternalAction, FileAction, Plan, TriplePlan};
use crate::region::{RegionFile, write_region};
use crate::types::{ChunkPos, FileKind, Observer, Progress, Warning};

/// How each source file was written.
#[derive(Debug, Default)]
pub(crate) struct Written {
    /// Destination of every file written, with the source and whether it was rewritten.
    pub files: BTreeMap<PathBuf, WrittenFile>,
    pub owner_not_preserved: bool,
}

#[derive(Debug)]
pub(crate) struct WrittenFile {
    pub source: PathBuf,
    pub rewrite: Option<Rewrite>,
}

/// A repacked region file: the triple and kind it came from.
#[derive(Debug)]
pub(crate) struct Rewrite {
    pub dim: usize,
    pub triple: usize,
    pub kind: FileKind,
}

/// What the plan says about a source path.
enum Role<'a> {
    Triple(usize, usize, &'a TriplePlan, FileKind),
    External(&'a TriplePlan, FileKind, ChunkPos),
}

pub(crate) fn write_output(
    plan: &Plan,
    walked: &WorldFiles,
    out: &Path,
    observer: &dyn Observer,
    cancel: &AtomicBool,
) -> Result<Written, OutputError> {
    let layout = &plan.layout;
    let mut roles: BTreeMap<&Path, Role<'_>> = BTreeMap::new();
    for (d, dim) in plan.dimensions.iter().enumerate() {
        for (i, t) in dim.triples.iter().enumerate() {
            for kind in FileKind::ALL {
                if let Some(file) = t.file(kind) {
                    roles.insert(&file.path, Role::Triple(d, i, t, kind));
                    for x in &file.externals {
                        roles.insert(&x.path, Role::External(t, kind, x.pos));
                    }
                }
            }
        }
    }
    let omitted: BTreeSet<&Path> = plan
        .omitted_other_files
        .iter()
        .map(|o| o.path.as_path())
        .collect();
    for dir in walked.dirs.iter().filter(|d| !is_excluded(layout, d)) {
        let dest = destination(layout, out, dir);
        fs::create_dir_all(&dest).map_err(io_err(&dest))?;
    }
    for link in &walked.links {
        observer.warn(Warning::SymlinkFollowed(link.clone()));
    }
    for special in &walked.special {
        observer.warn(Warning::SpecialFileSkipped(special.clone()));
    }

    let mut written = Written::default();
    let mut warned_owner = false;
    let total = walked.files.len() as u64;
    for (n, file) in walked.files.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(OutputError::Cancelled);
        }
        observer.progress(Progress::Writing {
            done: n as u64,
            total,
        });
        if is_excluded(layout, &file.path) || omitted.contains(file.path.as_path()) {
            continue;
        }
        let dest = destination(layout, out, &file.path);
        let rewrite = match roles.get(file.path.as_path()) {
            Some(Role::Triple(d, i, t, kind)) => match t.action(*kind) {
                Some(FileAction::Verbatim) | None => (copy_file(&file.path, &dest)?, None),
                Some(FileAction::Rewrite { keep }) => {
                    let bytes = fs::read(&file.path).map_err(io_err(&file.path))?;
                    let externals = t
                        .file(*kind)
                        .map(|f| f.externals.iter().map(|x| x.pos).collect())
                        .unwrap_or_default();
                    let source = RegionFile::parse(*kind, t.pos, bytes, &externals);
                    let packed = write_region(&source, &keep).map_err(|e| OutputError::Io {
                        path: file.path.clone(),
                        source: std::io::Error::other(e),
                    })?;
                    let handle = write_file(&dest, &packed)?;
                    (
                        handle,
                        Some(Rewrite {
                            dim: *d,
                            triple: *i,
                            kind: *kind,
                        }),
                    )
                }
                Some(FileAction::Omit(_)) => continue,
            },
            Some(Role::External(t, kind, pos)) => match t.external_action(*kind, *pos) {
                ExternalAction::Copy => (copy_file(&file.path, &dest)?, None),
                ExternalAction::Omit => continue,
            },
            None => (copy_file(&file.path, &dest)?, None),
        };
        let (handle, rewrite) = rewrite;
        let owner_ok = finish(file, handle, &dest)?;
        if !owner_ok && !warned_owner {
            warned_owner = true;
            written.owner_not_preserved = true;
            observer.warn(Warning::OwnerNotPreserved);
        }
        written.files.insert(
            dest,
            WrittenFile {
                source: file.path.clone(),
                rewrite,
            },
        );
    }
    observer.progress(Progress::Writing { done: total, total });

    // Directory entries must be durable before complete.json says the output is done.
    for dir in walked.dirs.iter().rev().filter(|d| !is_excluded(layout, d)) {
        sync_dir(&destination(layout, out, dir))?;
    }
    sync_dir(out)?;
    Ok(written)
}

/// Copies file contents (never a hard link). The file is synced by [`finish`].
fn copy_file(source: &Path, dest: &Path) -> Result<File, OutputError> {
    let mut from = File::open(source).map_err(io_err(source))?;
    let mut to = File::create(dest).map_err(io_err(dest))?;
    std::io::copy(&mut from, &mut to).map_err(io_err(dest))?;
    Ok(to)
}

fn write_file(dest: &Path, bytes: &[u8]) -> Result<File, OutputError> {
    let mut to = File::create(dest).map_err(io_err(dest))?;
    to.write_all(bytes).map_err(io_err(dest))?;
    Ok(to)
}

/// Sets the modification time on the open handle, then the owner (as root),
/// then the permissions (after chown, which clears setuid/setgid), and syncs
/// data and metadata. Returns whether the owner matches the source.
fn finish(file: &WorldFile, handle: File, dest: &Path) -> Result<bool, OutputError> {
    let meta = fs::metadata(&file.path).map_err(io_err(&file.path))?;
    if let Ok(modified) = meta.modified() {
        handle.set_modified(modified).map_err(io_err(dest))?;
    }
    let owner_ok = owner(&meta, dest)?;
    fs::set_permissions(dest, meta.permissions()).map_err(io_err(dest))?;
    handle.sync_all().map_err(io_err(dest))?;
    Ok(owner_ok)
}

#[cfg(unix)]
fn owner(meta: &fs::Metadata, dest: &Path) -> Result<bool, OutputError> {
    use std::os::unix::fs::MetadataExt;
    if crate::sys::is_root() {
        return match std::os::unix::fs::chown(dest, Some(meta.uid()), Some(meta.gid())) {
            Ok(()) => Ok(true),
            // Filesystems without owners (vfat, some network mounts): warn instead of failing.
            Err(e)
                if matches!(
                    e.raw_os_error(),
                    Some(libc::EPERM | libc::EINVAL | libc::ENOTSUP)
                ) =>
            {
                Ok(false)
            }
            Err(e) => Err(io_err(dest)(e)),
        };
    }
    Ok(meta.uid() == crate::sys::effective_uid())
}

#[cfg(windows)]
fn owner(_: &fs::Metadata, _: &Path) -> Result<bool, OutputError> {
    Ok(true)
}
