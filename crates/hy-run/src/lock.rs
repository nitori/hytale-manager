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
    file: std::fs::File,
    path: PathBuf,
}

impl RunLock {
    /// Fails rather than waits: a second `hy run` is a mistake to report, not a queue to
    /// join.
    pub fn acquire(root: &Path) -> Result<Self> {
        let path = root.join(LOCK_FILE);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)?;

        match FileExt::try_lock(&file) {
            Ok(()) => Ok(Self { file, path }),
            Err(TryLockError::WouldBlock) => Err(Error::AlreadyRunning(root.to_path_buf())),
            Err(TryLockError::Error(err)) => Err(err.into()),
        }
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
        let _ = FileExt::unlock(&self.file);
    }
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
    fn is_held_tracks_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!RunLock::is_held(dir.path()));
        let lock = RunLock::acquire(dir.path()).unwrap();
        assert!(RunLock::is_held(dir.path()));
        drop(lock);
        assert!(!RunLock::is_held(dir.path()));
    }
}
