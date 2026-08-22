//! Per-instance mutual exclusion.
//!
//! Two servers sharing one `universe/` corrupt it, and nothing in the server prevents
//! that. The lock is advisory between `hy` processes only — it is not a PID file and
//! nothing reads it for control.

use std::path::{Path, PathBuf};

use fs4::{FileExt, TryLockError};

use crate::error::{Error, Result};

pub const LOCK_FILE: &str = ".hy-run.lock";

pub struct RunLock {
    /// Taken in `drop` on Windows, which cannot unlink a file that is still open.
    file: Option<std::fs::File>,
    path: PathBuf,
}

impl RunLock {
    /// Fails rather than waits: a second `hy run` is a mistake to report, not a queue to
    /// join.
    pub fn acquire(root: &Path) -> Result<Self> {
        let path = root.join(LOCK_FILE);

        // The lock is removed on release, and `flock` binds to the inode rather than the
        // path — so a file opened just before someone else unlinks it is an orphan that
        // locks successfully while a fresh file at the same path locks independently. Both
        // holders would then believe they own the instance. Re-open until the inode we
        // locked is still the one the path names.
        for _ in 0..16 {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(false)
                .open(&path)?;

            match FileExt::try_lock(&file) {
                Ok(()) => {}
                Err(TryLockError::WouldBlock) => {
                    return Err(Error::AlreadyRunning(root.to_path_buf()));
                }
                Err(TryLockError::Error(err)) => return Err(err.into()),
            }

            if is_file_at(&file, &path) {
                return Ok(Self {
                    file: Some(file),
                    path,
                });
            }
            let _ = FileExt::unlock(&file);
        }

        Err(Error::AlreadyRunning(root.to_path_buf()))
    }

    /// Whether some other process holds the lock, without taking it.
    pub fn is_held(root: &Path) -> bool {
        let path = root.join(LOCK_FILE);
        let Ok(file) = std::fs::OpenOptions::new().write(true).open(&path) else {
            return false;
        };
        match FileExt::try_lock(&file) {
            Ok(()) => {
                let _ = FileExt::unlock(&file);
                false
            }
            Err(TryLockError::WouldBlock) => true,
            Err(TryLockError::Error(_)) => false,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for RunLock {
    fn drop(&mut self) {
        // A crash leaves the file behind either way; it is harmless, and the next run
        // reuses it.
        #[cfg(unix)]
        {
            // Unlinked *before* unlocking, so the path is already empty when the lock frees
            // up: a waiting process then creates a fresh file rather than opening the one
            // about to be orphaned. Unlocking first would reopen the window this whole
            // dance exists to close.
            let _ = std::fs::remove_file(&self.path);
            if let Some(file) = &self.file {
                let _ = FileExt::unlock(file);
            }
        }

        #[cfg(not(unix))]
        {
            // Windows refuses to unlink an open file, so the handle has to go first. That
            // leaves a gap in which another process may take the lock — and then the
            // removal simply fails and the file stays, which is untidy but never unsafe.
            if let Some(file) = self.file.take() {
                let _ = FileExt::unlock(&file);
                drop(file);
            }
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Whether `file` is still the file `path` names.
///
/// Windows will not unlink a file that is open, so the orphaned-inode case cannot arise
/// there and the check is unnecessary.
#[cfg(unix)]
fn is_file_at(file: &std::fs::File, path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;

    let (Ok(held), Ok(named)) = (file.metadata(), std::fs::metadata(path)) else {
        return false;
    };
    held.dev() == named.dev() && held.ino() == named.ino()
}

#[cfg(not(unix))]
fn is_file_at(_file: &std::fs::File, _path: &Path) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_acquire_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let _first = RunLock::acquire(dir.path()).unwrap();
        assert!(matches!(
            RunLock::acquire(dir.path()),
            Err(Error::AlreadyRunning(_))
        ));
    }

    #[test]
    fn releasing_allows_a_later_run() {
        let dir = tempfile::tempdir().unwrap();
        drop(RunLock::acquire(dir.path()).unwrap());
        assert!(RunLock::acquire(dir.path()).is_ok());
    }

    #[test]
    fn releasing_cleans_the_file_up() {
        let dir = tempfile::tempdir().unwrap();
        let lock = RunLock::acquire(dir.path()).unwrap();
        assert!(dir.path().join(LOCK_FILE).exists());

        drop(lock);
        assert!(
            !dir.path().join(LOCK_FILE).exists(),
            "a released lock should not leave a file behind"
        );
    }

    /// A lock still held keeps its file, so a concurrent `acquire` has something to contend
    /// on rather than quietly creating a second one.
    #[test]
    fn a_held_lock_keeps_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let _held = RunLock::acquire(dir.path()).unwrap();
        assert!(RunLock::acquire(dir.path()).is_err());
        assert!(dir.path().join(LOCK_FILE).exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_lock_on_an_orphaned_inode_is_not_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(LOCK_FILE);
        std::fs::write(&path, b"").unwrap();

        let orphan = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        // Freshly created at the same path, a different inode entirely.
        std::fs::write(&path, b"").unwrap();

        assert!(!is_file_at(&orphan, &path));
    }

    #[test]
    fn is_held_tracks_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!RunLock::is_held(dir.path()));
        let lock = RunLock::acquire(dir.path()).unwrap();
        assert!(RunLock::is_held(dir.path()));
        drop(lock);
        assert!(!RunLock::is_held(dir.path()));
    }
}
