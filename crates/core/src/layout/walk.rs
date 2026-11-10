//! Listing every file under the world folders.

use std::fs;
use std::path::{Path, PathBuf};

use super::{LayoutError, WorldLayout, read_dir_opt};

/// A file found under a world folder.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct WorldFile {
    pub path: PathBuf,
    pub size: u64,
    /// Reached through a symbolic link (the file itself or a parent directory).
    pub via_symlink: bool,
}

/// What a walk found, sorted by path.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorldFiles {
    pub files: Vec<WorldFile>,
    /// Directories, including empty ones.
    pub dirs: Vec<PathBuf>,
    /// Entries that are neither regular files nor directories, and dangling links.
    pub special: Vec<PathBuf>,
    /// Symbolic links that were followed.
    pub links: Vec<PathBuf>,
}

/// Walks all world folders, following symbolic links like the game does.
///
/// A directory reached through links appears under every path that leads to
/// it (the output copies each path). Only a real loop, a link to a directory
/// that is its own ancestor, is an error. A dangling link is reported in
/// `special`. Entries are visited in name order.
pub fn world_files(layout: &WorldLayout) -> Result<WorldFiles, LayoutError> {
    files_in(&layout.world_folders)
}

/// Like [`world_files`], for any list of folders.
pub fn files_in(folders: &[PathBuf]) -> Result<WorldFiles, LayoutError> {
    let mut out = WorldFiles::default();
    for folder in folders {
        walk(folder, false, &mut Vec::new(), &mut out)?;
    }
    out.files.sort();
    out.files.dedup_by(|a, b| a.path == b.path);
    out.dirs.sort();
    out.dirs.dedup();
    out.special.sort();
    out.special.dedup();
    out.links.sort();
    out.links.dedup();
    Ok(out)
}

fn walk(
    dir: &Path,
    via_symlink: bool,
    ancestors: &mut Vec<PathBuf>,
    out: &mut WorldFiles,
) -> Result<(), LayoutError> {
    let canonical = fs::canonicalize(dir).map_err(|e| LayoutError::io(dir, e))?;
    if ancestors.contains(&canonical) {
        return Err(LayoutError::SymlinkLoop {
            path: dir.to_path_buf(),
        });
    }
    ancestors.push(canonical);
    out.dirs.push(dir.to_path_buf());
    let mut entries = read_dir_opt(dir)?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let is_link = entry
            .file_type()
            .map_err(|e| LayoutError::io(&path, e))?
            .is_symlink();
        let meta = match fs::metadata(&path) {
            Ok(meta) => meta,
            // A dangling link: nothing to copy.
            Err(_) if is_link => {
                out.special.push(path);
                continue;
            }
            Err(e) => return Err(LayoutError::io(&path, e)),
        };
        if is_link {
            out.links.push(path.clone());
        }
        if meta.is_dir() {
            walk(&path, via_symlink || is_link, ancestors, out)?;
        } else if meta.is_file() {
            out.files.push(WorldFile {
                path,
                size: meta.len(),
                via_symlink: via_symlink || is_link,
            });
        } else {
            out.special.push(path);
        }
    }
    ancestors.pop();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::detect;
    use super::super::test_util::*;
    use super::*;

    #[test]
    fn lists_files_and_empty_dirs() {
        let t = tempfile::tempdir().unwrap();
        let w = t.path();
        region(w, "r.0.0.mca");
        level_dat(w, 4325);
        fs::create_dir_all(w.join("data/empty")).unwrap();
        let l = detect(w).unwrap();
        let found = world_files(&l).unwrap();
        let names: Vec<_> = found
            .files
            .iter()
            .map(|f| f.path.strip_prefix(&l.root).unwrap().to_owned())
            .collect();
        assert_eq!(
            names,
            [
                PathBuf::from("level.dat"),
                PathBuf::from("region/r.0.0.mca")
            ]
        );
        assert!(found.dirs.contains(&l.root.join("data/empty")));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_followed_and_loops_rejected() {
        let t = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let w = t.path();
        region(w, "r.0.0.mca");
        level_dat(w, 4325);
        write(&outside.path().join("x.dat"), b"abc");
        std::os::unix::fs::symlink(outside.path(), w.join("linked")).unwrap();
        let l = detect(w).unwrap();
        let found = world_files(&l).unwrap();
        let linked = found
            .files
            .iter()
            .find(|f| f.path.ends_with("linked/x.dat"))
            .unwrap();
        assert!(linked.via_symlink);
        assert_eq!(linked.size, 3);

        // Two links to the same directory are not a loop; both paths are listed.
        std::os::unix::fs::symlink(outside.path(), w.join("linked_again")).unwrap();
        std::os::unix::fs::symlink("/nonexistent/wgc-test", w.join("dangling")).unwrap();
        let found = world_files(&l).unwrap();
        assert!(
            found
                .files
                .iter()
                .any(|f| f.path.ends_with("linked_again/x.dat"))
        );
        assert_eq!(found.special, [l.root.join("dangling")]);

        std::os::unix::fs::symlink(w, w.join("data_loop")).unwrap();
        assert!(matches!(
            world_files(&l),
            Err(LayoutError::SymlinkLoop { .. })
        ));
    }
}
