//! The on-disk `game/` layout.
//!
//! ```text
//! game/
//! ├── Assets.zip
//! ├── start.sh / start.bat     the server's launcher; hy does not use it
//! ├── jvm.options              legacy; imported once at adoption
//! ├── hytale.toml
//! ├── .java-version
//! ├── updater/staging/
//! └── Server/                  the server's working directory
//!     ├── HytaleServer.jar
//!     └── universe/ logs/ mods/ backups/ .cache/
//! ```
//!
//! The server requires this shape: its updater disables itself unless started from
//! `Server/` with `Assets.zip` and a launcher script in the parent.

use std::path::{Path, PathBuf};

use crate::config::CONFIG_FILE;

/// Path accessors for one instance directory. Asserts nothing about what exists on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    root: PathBuf,
}

impl Layout {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn server_dir(&self) -> PathBuf {
        self.root.join("Server")
    }

    pub fn jar(&self) -> PathBuf {
        self.server_dir().join("HytaleServer.jar")
    }

    pub fn assets(&self) -> PathBuf {
        self.root.join("Assets.zip")
    }

    pub fn server_config(&self) -> PathBuf {
        self.server_dir().join("config.json")
    }

    pub fn universe(&self) -> PathBuf {
        self.server_dir().join("universe")
    }

    pub fn logs(&self) -> PathBuf {
        self.server_dir().join("logs")
    }

    pub fn mods(&self) -> PathBuf {
        self.server_dir().join("mods")
    }

    /// The server's own periodic backups, distinct from `hy backup`'s cold snapshots.
    pub fn server_backups(&self) -> PathBuf {
        self.server_dir().join("backups")
    }

    /// Where a downloaded update waits until the server exits with code 8.
    pub fn staging(&self) -> PathBuf {
        self.root.join("updater").join("staging")
    }

    /// Where the previous version is kept for rollback after an update.
    pub fn updater_backup(&self) -> PathBuf {
        self.root.join("updater").join("backup")
    }

    pub fn config(&self) -> PathBuf {
        self.root.join(CONFIG_FILE)
    }

    pub fn jvm_options(&self) -> PathBuf {
        self.root.join("jvm.options")
    }

    /// Both variants count on every platform: an instance moved from Windows keeps its
    /// `start.bat`, and the updater's layout check accepts either.
    pub fn launcher(&self) -> Option<PathBuf> {
        [self.root.join("start.sh"), self.root.join("start.bat")]
            .into_iter()
            .find(|path| path.is_file())
    }

    /// Whether a staged update is waiting to be applied.
    pub fn has_staged_update(&self) -> bool {
        self.staging()
            .join("Server")
            .join("HytaleServer.jar")
            .is_file()
    }

    /// The jar and assets are the load-bearing pair; the server creates the rest itself.
    pub fn is_server_install(&self) -> bool {
        self.jar().is_file() && self.assets().is_file()
    }

    pub fn is_initialised(&self) -> bool {
        self.config().is_file()
    }

    pub fn validate(&self) -> Vec<Finding> {
        let mut findings = Vec::new();

        if !self.assets().is_file() {
            findings.push(Finding::MissingAssets(self.assets()));
        }
        if !self.server_dir().is_dir() {
            findings.push(Finding::MissingServerDir(self.server_dir()));
        } else if !self.jar().is_file() {
            findings.push(Finding::MissingJar(self.jar()));
        }
        if self.launcher().is_none() {
            findings.push(Finding::MissingLauncher(self.root.clone()));
        }

        findings
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    MissingAssets(PathBuf),
    MissingServerDir(PathBuf),
    MissingJar(PathBuf),
    /// Unused by `hy`, but the server's updater needs to see it.
    MissingLauncher(PathBuf),
}

impl Finding {
    /// A missing launcher is not fatal: the server runs, it just never updates itself.
    pub fn is_fatal(&self) -> bool {
        !matches!(self, Finding::MissingLauncher(_))
    }
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Finding::MissingAssets(path) => {
                write!(f, "missing {}", path.display())
            }
            Finding::MissingServerDir(path) => {
                write!(f, "missing server directory {}", path.display())
            }
            Finding::MissingJar(path) => {
                write!(f, "missing {}", path.display())
            }
            Finding::MissingLauncher(root) => write!(
                f,
                "no start.sh or start.bat in {} — the server disables its own update \
                 checker without one; run `/update setup` to restore it",
                root.display()
            ),
        }
    }
}

/// Search `start` and its ancestors for an instance root.
///
/// Uninitialised installs count, so `hy status` works on one the bootstrap jar made.
pub fn discover(start: &Path) -> Option<Layout> {
    start.ancestors().find_map(|dir| {
        let layout = Layout::new(dir);
        (layout.is_initialised() || layout.is_server_install()).then_some(layout)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server_install(root: &Path) {
        std::fs::create_dir_all(root.join("Server")).unwrap();
        std::fs::write(root.join("Assets.zip"), b"").unwrap();
        std::fs::write(root.join("Server/HytaleServer.jar"), b"").unwrap();
        std::fs::write(root.join("start.sh"), b"").unwrap();
    }

    #[test]
    fn paths_hang_off_the_root() {
        let layout = Layout::new("/srv/game");
        assert_eq!(layout.jar(), Path::new("/srv/game/Server/HytaleServer.jar"));
        assert_eq!(layout.assets(), Path::new("/srv/game/Assets.zip"));
        assert_eq!(layout.universe(), Path::new("/srv/game/Server/universe"));
        assert_eq!(layout.staging(), Path::new("/srv/game/updater/staging"));
    }

    #[test]
    fn discovery_walks_up_from_the_server_directory() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("game");
        server_install(&root);

        let found = discover(&root.join("Server")).expect("should find the parent instance");
        assert_eq!(found.root(), root);
    }

    #[test]
    fn discovery_finds_uninitialised_installs() {
        let dir = tempfile::tempdir().unwrap();
        server_install(dir.path());
        // No hytale.toml: a bootstrap-created install `hy` has never touched.
        assert!(!Layout::new(dir.path()).is_initialised());
        assert!(discover(dir.path()).is_some());
    }

    #[test]
    fn discovery_ignores_unrelated_directories() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Server")).unwrap();
        // A `Server/` directory alone is not an instance — no jar, no assets.
        assert!(discover(&dir.path().join("Server")).is_none());
    }

    #[test]
    fn validate_reports_a_missing_launcher_without_calling_it_fatal() {
        let dir = tempfile::tempdir().unwrap();
        server_install(dir.path());
        std::fs::remove_file(dir.path().join("start.sh")).unwrap();

        let findings = Layout::new(dir.path()).validate();
        assert_eq!(findings.len(), 1);
        assert!(matches!(findings[0], Finding::MissingLauncher(_)));
        // The server runs; it just silently stops updating itself.
        assert!(!findings[0].is_fatal());
    }

    #[test]
    fn validate_does_not_report_a_missing_jar_twice() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Assets.zip"), b"").unwrap();
        std::fs::write(dir.path().join("start.sh"), b"").unwrap();

        // With no `Server/` at all, the jar is missing because its directory is.
        let findings = Layout::new(dir.path()).validate();
        assert_eq!(findings.len(), 1);
        assert!(matches!(findings[0], Finding::MissingServerDir(_)));
    }

    #[test]
    fn validate_is_empty_for_a_complete_layout() {
        let dir = tempfile::tempdir().unwrap();
        server_install(dir.path());
        assert!(Layout::new(dir.path()).validate().is_empty());
    }
}
