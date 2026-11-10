//! Locking `session.lock` (docs/spec/lock.md).
//!
//! The lock must exclude Java's `FileChannel.tryLock`, which uses POSIX
//! record locks on Unix and `LockFileEx` on Windows. `flock` (and therefore
//! `std::fs::File::try_lock`) does not conflict with POSIX locks on Linux,
//! so it must not be used. All unsafe code is in [`sys`].

mod sys;

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use crate::layout::WorldLayout;

/// File name of the lock in each world folder.
pub const LOCK_FILE: &str = "session.lock";

/// Why `session.lock` could not be locked.
#[derive(Debug, thiserror::Error)]
pub enum LockError {
    /// Another process (the game or a server) holds the lock.
    #[error("{path} is locked by another process")]
    InUse { path: PathBuf },
    /// `session.lock` does not exist (stop L-1).
    #[error("{path} does not exist")]
    Missing { path: PathBuf },
    /// `session.lock` cannot be opened for writing, which locking needs.
    #[error("{path} cannot be opened for writing")]
    NotWritable { path: PathBuf },
    /// This process already holds the lock. Opening the file again could
    /// release it (POSIX locks belong to the process), so it is refused.
    #[error("{path} is already locked by this process")]
    HeldByThisProcess { path: PathBuf },
    #[error("cannot lock {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// A held lock on one world folder's `session.lock`. Dropping it releases the lock.
///
/// While it is held, do not open `session.lock` for anything else: on Unix
/// systems without OFD locks, closing any other descriptor of the file
/// releases the lock.
#[derive(Debug)]
pub struct SessionLock {
    path: PathBuf,
    file: Option<File>,
}

/// `session.lock` files this process holds, by canonical path.
static HELD: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

fn held() -> std::sync::MutexGuard<'static, BTreeSet<PathBuf>> {
    HELD.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Drop for SessionLock {
    fn drop(&mut self) {
        // Close (and so unlock) before allowing this process to open the file again.
        drop(self.file.take());
        held().remove(&self.path);
    }
}

impl SessionLock {
    /// Tries to lock `<world_folder>/session.lock` without waiting.
    ///
    /// The file is opened for reading and writing without creating or
    /// truncating it; its content is never changed.
    pub fn try_acquire(world_folder: &Path) -> Result<SessionLock, LockError> {
        let folder = fs::canonicalize(world_folder).unwrap_or_else(|_| world_folder.to_path_buf());
        let path = folder.join(LOCK_FILE);
        let mut held = held();
        if held.contains(&path) {
            return Err(LockError::HeldByThisProcess { path });
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|e| match e.kind() {
                io::ErrorKind::NotFound => LockError::Missing { path: path.clone() },
                io::ErrorKind::PermissionDenied => LockError::NotWritable { path: path.clone() },
                _ => LockError::Io {
                    path: path.clone(),
                    source: e,
                },
            })?;
        match sys::try_lock(&file) {
            Ok(true) => {
                held.insert(path.clone());
                Ok(SessionLock {
                    path,
                    file: Some(file),
                })
            }
            Ok(false) => Err(LockError::InUse { path }),
            Err(source) => Err(LockError::Io { path, source }),
        }
    }

    /// Whether another process holds the lock. Releases it again right away.
    /// A missing `session.lock` counts as not locked. Fails with
    /// [`LockError::HeldByThisProcess`] if this process holds it.
    pub fn is_locked(world_folder: &Path) -> Result<bool, LockError> {
        match Self::try_acquire(world_folder) {
            Ok(_) => Ok(false),
            Err(LockError::InUse { .. }) => Ok(true),
            Err(LockError::Missing { .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Path of the locked file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Locks on every world folder of a layout.
#[derive(Debug)]
pub struct SessionLocks(Vec<SessionLock>);

impl SessionLocks {
    /// Locks all `world_folders` in order. Folders that resolve to the same
    /// directory are locked once. On failure, the locks taken so far are released.
    pub fn try_acquire_all(layout: &WorldLayout) -> Result<SessionLocks, LockError> {
        let mut folders: Vec<PathBuf> = layout
            .world_folders
            .iter()
            .map(|f| fs::canonicalize(f).unwrap_or_else(|_| f.clone()))
            .collect();
        folders.sort();
        folders.dedup();
        folders
            .iter()
            .map(|folder| SessionLock::try_acquire(folder))
            .collect::<Result<Vec<_>, _>>()
            .map(SessionLocks)
    }

    pub fn locks(&self) -> &[SessionLock] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};

    use super::*;

    fn world_with_lock() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join(LOCK_FILE), "\u{2603}").unwrap();
        t
    }

    #[test]
    fn missing_lock_file() {
        let t = tempfile::tempdir().unwrap();
        assert!(matches!(
            SessionLock::try_acquire(t.path()),
            Err(LockError::Missing { .. })
        ));
        assert!(!SessionLock::is_locked(t.path()).unwrap());
        assert!(
            !t.path().join(LOCK_FILE).exists(),
            "must not create session.lock"
        );
    }

    #[test]
    fn content_is_not_changed() {
        let t = world_with_lock();
        let lock = SessionLock::try_acquire(t.path()).unwrap();
        drop(lock);
        assert_eq!(
            std::fs::read_to_string(t.path().join(LOCK_FILE)).unwrap(),
            "\u{2603}"
        );
    }

    #[test]
    fn second_lock_in_the_same_process_is_refused() {
        let t = world_with_lock();
        let _lock = SessionLock::try_acquire(t.path()).unwrap();
        assert!(matches!(
            SessionLock::try_acquire(t.path()),
            Err(LockError::HeldByThisProcess { .. })
        ));
        assert!(matches!(
            SessionLock::is_locked(t.path()),
            Err(LockError::HeldByThisProcess { .. })
        ));
    }

    #[test]
    fn released_on_drop() {
        let t = world_with_lock();
        let lock = SessionLock::try_acquire(t.path()).unwrap();
        drop(lock);
        let _again = SessionLock::try_acquire(t.path()).unwrap();
    }

    /// Starts a Java program that tries to lock `lock_path` like Minecraft does
    /// and prints `locked` or `busy`. Returns `None` when Java is not installed.
    fn java_locker(lock_path: &Path) -> Option<(tempfile::TempDir, std::process::Child)> {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("Locker.java");
        std::fs::write(
            &source,
            r#"
import java.nio.channels.*; import java.nio.file.*;
public class Locker {
  public static void main(String[] a) throws Exception {
    FileChannel ch = FileChannel.open(Paths.get(a[0]), StandardOpenOption.WRITE);
    FileLock lock = ch.tryLock();
    System.out.println(lock == null ? "busy" : "locked");
    System.out.flush();
    System.in.read();
  }
}"#,
        )
        .unwrap();
        let child = Command::new("java")
            .arg(&source)
            .arg(lock_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        Some((dir, child))
    }

    fn answer(child: &mut std::process::Child) -> String {
        let mut line = String::new();
        let _ = BufReader::new(child.stdout.as_mut().unwrap()).read_line(&mut line);
        line.trim().to_owned()
    }

    fn finish(mut child: std::process::Child) {
        child.stdin.as_mut().unwrap().write_all(b"\n").unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn excludes_java_file_locks_both_ways() {
        let world = world_with_lock();
        let lock_path = world.path().join(LOCK_FILE);

        // Java holds the lock: we see it as in use.
        let Some((_src, mut child)) = java_locker(&lock_path) else {
            eprintln!("java not found; skipping");
            return;
        };
        // A `java` that cannot run single-file programs (stub, JRE, Java < 11) prints nothing.
        let first = answer(&mut child);
        if first.is_empty() {
            let _ = child.kill();
            eprintln!("java cannot run the test program; skipping");
            return;
        }
        assert_eq!(first, "locked");
        assert!(matches!(
            SessionLock::try_acquire(world.path()),
            Err(LockError::InUse { .. })
        ));
        assert!(SessionLock::is_locked(world.path()).unwrap());
        finish(child);

        // We hold the lock: Java cannot get it, also after other file I/O in the world.
        let lock = SessionLock::try_acquire(world.path()).unwrap();
        std::fs::write(world.path().join("level.dat"), b"x").unwrap();
        let _ = std::fs::read(world.path().join("level.dat")).unwrap();
        let (_src, mut child) = java_locker(&lock_path).unwrap();
        assert_eq!(answer(&mut child), "busy");
        finish(child);
        drop(lock);

        // Released: Java gets it.
        let (_src, mut child) = java_locker(&lock_path).unwrap();
        assert_eq!(answer(&mut child), "locked");
        finish(child);
    }
}
