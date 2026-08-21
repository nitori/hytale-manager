//! The managed JDK store.
//!
//! ```text
//! ~/.local/share/hy/                       (%LOCALAPPDATA%\hy on Windows)
//! ├── java/
//! │   ├── .locks/
//! │   └── temurin-25.0.4.1+1-linux-x86_64/  ← install key
//! └── cache/downloads/
//! ```
//!
//! Two invariants matter:
//!
//! * **Atomicity.** An install is extracted into a temporary directory on the same
//!   filesystem and then renamed into place, so a half-extracted JDK is never discoverable
//!   as a usable one.
//! * **Exclusion.** A lockfile per key means two concurrent `hy run` invocations do not
//!   race on the same download.

use std::path::{Path, PathBuf};

use etcetera::BaseStrategy;
use fs4::{FileExt, TryLockError};

use crate::adoptium::ReleaseAsset;
use crate::download::{Checksum, ProgressReporter, download_verified};
use crate::error::{Error, Result};
use crate::key::InstallKey;
use crate::platform::ArchiveKind;

#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ManagedInstall {
    pub key: InstallKey,
    path: PathBuf,
}

impl ManagedInstall {
    /// The JDK home. Installs are normalised at extraction time (macOS bundles are
    /// unwrapped), so this is simply the install directory.
    pub fn java_home(&self) -> &Path {
        &self.path
    }

    pub fn java_executable(&self) -> PathBuf {
        self.path.join("bin").join(self.key.os.java_executable())
    }
}

impl Store {
    /// Locate the store, honouring `HY_HOME`.
    pub fn from_env() -> Result<Self> {
        if let Some(home) = std::env::var_os("HY_HOME") {
            return Ok(Self {
                root: PathBuf::from(home),
            });
        }
        let strategy =
            etcetera::choose_base_strategy().map_err(|e| std::io::Error::other(e.to_string()))?;
        Ok(Self {
            root: strategy.data_dir().join("hy"),
        })
    }

    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn java_dir(&self) -> PathBuf {
        self.root.join("java")
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache").join("downloads")
    }

    /// Every complete managed install, newest first.
    pub fn installs(&self) -> Result<Vec<ManagedInstall>> {
        let dir = self.java_dir();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Ok(Vec::new());
        };

        let mut installs = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            // Skip lock storage and in-flight extractions.
            if name.starts_with('.') {
                continue;
            }
            if !entry.path().is_dir() {
                continue;
            }
            let Ok(key) = name.parse::<InstallKey>() else {
                tracing::debug!("ignoring unrecognised entry in java store: {name}");
                continue;
            };
            installs.push(ManagedInstall {
                key,
                path: entry.path(),
            });
        }

        installs.sort_by(|a, b| b.key.version.cmp(&a.key.version));
        Ok(installs)
    }

    pub fn find(&self, key: &InstallKey) -> Result<Option<ManagedInstall>> {
        Ok(self.installs()?.into_iter().find(|i| &i.key == key))
    }

    /// Download and install `asset` under `key`, unless it is already present.
    pub async fn install(
        &self,
        http: &reqwest::Client,
        asset: &ReleaseAsset,
        key: &InstallKey,
        progress: &dyn ProgressReporter,
    ) -> Result<ManagedInstall> {
        let target = self.java_dir().join(key.to_string());

        // Hold the lock across the whole download-and-extract, so a second process waits
        // rather than duplicating the work.
        let _guard = self.lock(key)?;

        // Another process may have completed the install while we waited for the lock.
        if target.is_dir() {
            tracing::debug!("{key} already installed");
            return Ok(ManagedInstall {
                key: key.clone(),
                path: target,
            });
        }

        let archive = download_verified(
            http,
            &asset.url,
            &asset.name,
            &Checksum::Sha256(asset.sha256.clone()),
            Some(asset.size),
            &self.cache_dir(),
            progress,
        )
        .await?;

        // Extract beside the destination so the final move is a same-filesystem rename.
        let staging = tempfile::Builder::new()
            .prefix(".tmp-")
            .tempdir_in(self.java_dir())?;
        let staging_path = staging.path().to_path_buf();
        let kind = key.os.archive_kind();
        let name = asset.name.clone();

        tokio::task::spawn_blocking(move || match kind {
            ArchiveKind::TarGz => extract_tar_gz(&archive, &staging_path),
            ArchiveKind::Zip => extract_zip(&archive, &staging_path),
        })
        .await
        .map_err(std::io::Error::other)??;

        let extracted = single_directory(staging.path(), &name)?;

        // macOS JDKs nest the real home under `Contents/Home`; unwrap it so `java_home()`
        // is the install directory on every platform.
        let source = {
            let nested = extracted.join("Contents").join("Home");
            if nested.is_dir() { nested } else { extracted }
        };

        std::fs::rename(&source, &target).or_else(|err| {
            // A cross-device rename can happen if the store root spans filesystems.
            if target.is_dir() { Ok(()) } else { Err(err) }
        })?;

        Ok(ManagedInstall {
            key: key.clone(),
            path: target,
        })
    }

    pub fn uninstall(&self, key: &InstallKey) -> Result<bool> {
        let target = self.java_dir().join(key.to_string());
        if !target.is_dir() {
            return Ok(false);
        }
        let _guard = self.lock(key)?;
        std::fs::remove_dir_all(&target)?;
        Ok(true)
    }

    fn lock(&self, key: &InstallKey) -> Result<LockGuard> {
        let dir = self.java_dir().join(".locks");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{key}.lock"));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)?;

        match FileExt::try_lock(&file) {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                tracing::info!("waiting for another process to finish installing {key}");
                FileExt::lock(&file)?;
            }
            Err(TryLockError::Error(err)) => return Err(err.into()),
        }
        Ok(LockGuard { file })
    }
}

struct LockGuard {
    file: std::fs::File,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// Archives contain exactly one top-level directory; anything else is unexpected.
fn single_directory(root: &Path, archive_name: &str) -> Result<PathBuf> {
    let mut dirs = std::fs::read_dir(root)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir());
    let first = dirs
        .next()
        .ok_or_else(|| Error::UnexpectedArchiveLayout(archive_name.to_string()))?;
    if dirs.next().is_some() {
        return Err(Error::UnexpectedArchiveLayout(archive_name.to_string()));
    }
    Ok(first)
}

fn extract_tar_gz(archive: &Path, dest: &Path) -> Result<()> {
    let file = std::fs::File::open(archive)?;
    let decoder = flate2::read::GzDecoder::new(std::io::BufReader::new(file));
    let mut tar = tar::Archive::new(decoder);
    tar.set_preserve_permissions(true);
    tar.unpack(dest)?;
    Ok(())
}

fn extract_zip(archive: &Path, dest: &Path) -> Result<()> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file))?;
    zip.extract(dest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contention_is_distinguishable_from_io_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("install.lock");

        let first = std::fs::File::create(&path).unwrap();
        FileExt::try_lock(&first).expect("an uncontended lock should be acquired");

        // A separate open file description contends even within one process.
        let second = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        assert!(matches!(
            FileExt::try_lock(&second),
            Err(TryLockError::WouldBlock)
        ));
    }

    #[test]
    fn installs_ignores_lock_and_staging_entries() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf());
        let java = store.java_dir();
        std::fs::create_dir_all(java.join(".locks")).unwrap();
        std::fs::create_dir_all(java.join(".tmp-abc123/jdk-25")).unwrap();
        std::fs::create_dir_all(java.join("not-an-install-key")).unwrap();
        std::fs::create_dir_all(java.join("temurin-25.0.4.1+1-linux-x86_64")).unwrap();

        let installs = store.installs().unwrap();
        assert_eq!(installs.len(), 1);
        assert_eq!(
            installs[0].key.to_string(),
            "temurin-25.0.4.1+1-linux-x86_64"
        );
    }

    #[test]
    fn installs_are_ordered_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf());
        for key in [
            "temurin-25.0.4.1+1-linux-x86_64",
            "temurin-26.0.2+10-linux-x86_64",
            "temurin-21.0.4+7-linux-x86_64",
        ] {
            std::fs::create_dir_all(store.java_dir().join(key)).unwrap();
        }
        let versions: Vec<String> = store
            .installs()
            .unwrap()
            .iter()
            .map(|i| i.key.version.to_string())
            .collect();
        assert_eq!(versions, ["26.0.2+10", "25.0.4.1+1", "21.0.4+7"]);
    }
}
