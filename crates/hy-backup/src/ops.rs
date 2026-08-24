//! Creating, restoring, and pruning.

use std::path::{Path, PathBuf};

use hy_instance::Layout;

use crate::error::{Error, Result};
use crate::history::History;
use crate::manifest::Manifest;
use crate::store::{self, Backup, Origin};
use crate::{archive, store::snapshot_dir};

pub struct CreateOptions<'a> {
    /// Entries under `Server/` to archive.
    pub include: &'a [String],
    pub server_version: Option<&'a str>,
    /// Set when this is the safety copy taken immediately before restoring `id`.
    pub before_restore_of: Option<&'a str>,
}

/// Snapshot `Server/` into `snapshots/<id>.tar.gz`.
pub fn create(layout: &Layout, history: &History, options: &CreateOptions) -> Result<Backup> {
    let server = layout.server_dir();
    if !server.is_dir() {
        return Err(Error::NothingToBackUp(layout.root().to_path_buf()));
    }

    let created = jiff::Timestamp::now();
    let id = store::id_for(created);
    let manifest = Manifest {
        id: id.clone(),
        created,
        lineage: history.current(),
        before_restore_of: options.before_restore_of.map(str::to_string),
        server_version: options.server_version.map(str::to_string),
    };

    let destination = store::path_for(layout, &id);
    archive::create(&server, options.include, &manifest, &destination)?;

    Ok(Backup {
        id,
        size: std::fs::metadata(&destination)
            .map(|m| m.len())
            .unwrap_or(0),
        path: destination,
        origin: Origin::Snapshot,
        created,
        lineage: manifest.lineage,
        manifest: Some(manifest),
    })
}

/// What a restore should roll back.
#[derive(Debug, Clone)]
pub enum Restrict {
    /// Only the world. Rolling back `whitelist.json` would lock out someone added since,
    /// and the same argument holds for bans, config, and mods — those are usually meant to
    /// survive a rollback even though they are worth capturing.
    World,
    /// Everything the archive carries.
    Everything,
    /// Named top-level entries.
    Only(Vec<String>),
}

impl Restrict {
    pub const WORLD: &'static str = "universe";

    fn wants(&self, entry: &Path) -> bool {
        match self {
            Self::Everything => true,
            Self::World => entry == Path::new(Self::WORLD),
            Self::Only(names) => names.iter().any(|n| Path::new(n) == entry),
        }
    }
}

/// Replace instance state with `backup`, and record the new lineage.
///
/// The caller is expected to have taken a safety snapshot first; this does not, so that
/// the safety copy is visible in the listing as an ordinary backup.
pub fn restore(
    layout: &Layout,
    history: &mut History,
    backup: &Backup,
    restrict: &Restrict,
) -> Result<Vec<PathBuf>> {
    let restored = match backup.origin {
        Origin::Snapshot => restore_snapshot(layout, backup, restrict)?,
        Origin::Server => restore_server_backup(layout, backup, restrict)?,
    };

    // Nothing changed, so the history did not fork.
    if restored.is_empty() {
        return Ok(restored);
    }

    history.record_restore(&backup.id, jiff::Timestamp::now());
    history.write(&snapshot_dir(layout))?;
    Ok(restored)
}

/// Replace exactly the selected entries, so everything else — the jar, the AOT config, and
/// anything not being rolled back — survives untouched.
fn restore_snapshot(layout: &Layout, backup: &Backup, restrict: &Restrict) -> Result<Vec<PathBuf>> {
    let server = layout.server_dir();
    let wanted: Vec<PathBuf> = archive::covered_entries(&backup.path)?
        .into_iter()
        .filter(|entry| restrict.wants(entry))
        .collect();

    for entry in &wanted {
        remove(&server.join(entry))?;
    }
    archive::extract_selected(&backup.path, &server, &wanted)?;
    Ok(wanted)
}

/// The server's archives hold the contents of `universe/` and nothing else, so there is
/// only ever the world to roll back — which is what a default restore does anyway.
fn restore_server_backup(
    layout: &Layout,
    backup: &Backup,
    restrict: &Restrict,
) -> Result<Vec<PathBuf>> {
    let world = PathBuf::from(Restrict::WORLD);
    if !restrict.wants(&world) {
        return Ok(Vec::new());
    }

    let universe = layout.universe();
    remove(&universe)?;
    std::fs::create_dir_all(&universe)?;

    let file = std::fs::File::open(&backup.path)?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file))
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    zip.extract(&universe)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(vec![world])
}

fn remove(path: &Path) -> Result<()> {
    if path.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// Delete all but the `keep` newest snapshots, returning what went.
///
/// Only ours. The server prunes its own with `--backup-max-count`, and deleting out from
/// under it would be both surprising and a fight over the same directory.
pub fn prune(layout: &Layout, history: &History, keep: usize) -> Result<Vec<Backup>> {
    let mut ours: Vec<Backup> = store::list(layout, history)?
        .into_iter()
        .filter(|b| b.origin == Origin::Snapshot)
        .collect();

    if ours.len() <= keep {
        return Ok(Vec::new());
    }

    let doomed = ours.split_off(keep);
    for backup in &doomed {
        std::fs::remove_file(&backup.path)?;
    }
    Ok(doomed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance(root: &Path) -> Layout {
        std::fs::create_dir_all(root.join("Server/universe/worlds/default")).unwrap();
        std::fs::create_dir_all(root.join("Server/mods")).unwrap();
        std::fs::write(root.join("Assets.zip"), b"").unwrap();
        std::fs::write(root.join("Server/HytaleServer.jar"), b"jar").unwrap();
        std::fs::write(root.join("Server/config.json"), b"{\"a\":1}").unwrap();
        std::fs::write(root.join("Server/universe/memories.json"), b"before").unwrap();
        std::fs::write(
            root.join("Server/universe/worlds/default/config.json"),
            b"world-before",
        )
        .unwrap();
        Layout::new(root)
    }

    fn include() -> Vec<String> {
        ["universe", "mods", "config.json"]
            .map(str::to_string)
            .to_vec()
    }

    fn options(include: &[String]) -> CreateOptions<'_> {
        CreateOptions {
            include,
            server_version: Some("0.5.9"),
            before_restore_of: None,
        }
    }

    #[test]
    fn create_then_restore_brings_the_world_back() {
        let dir = tempfile::tempdir().unwrap();
        let layout = instance(dir.path());
        let history = History::default();

        let backup = create(&layout, &history, &options(&include())).unwrap();

        std::fs::write(layout.universe().join("memories.json"), b"after").unwrap();
        std::fs::write(layout.server_config(), b"{\"a\":2}").unwrap();

        let mut history = history;
        restore(&layout, &mut history, &backup, &Restrict::World).unwrap();

        assert_eq!(
            std::fs::read(layout.universe().join("memories.json")).unwrap(),
            b"before"
        );
        // Settings changed since the backup survive: rolling back a whitelist would lock
        // out anyone added in the meantime, and config is the same argument.
        assert_eq!(std::fs::read(layout.server_config()).unwrap(), b"{\"a\":2}");
    }

    #[test]
    fn restoring_everything_does_roll_back_settings() {
        let dir = tempfile::tempdir().unwrap();
        let layout = instance(dir.path());
        let history = History::default();
        let backup = create(&layout, &history, &options(&include())).unwrap();

        std::fs::write(layout.server_config(), b"{\"a\":2}").unwrap();

        let mut history = history;
        let restored = restore(&layout, &mut history, &backup, &Restrict::Everything).unwrap();

        assert_eq!(std::fs::read(layout.server_config()).unwrap(), b"{\"a\":1}");
        assert!(
            restored
                .iter()
                .any(|e| e == std::path::Path::new("config.json"))
        );
    }

    #[test]
    fn a_named_selection_restores_only_that() {
        let dir = tempfile::tempdir().unwrap();
        let layout = instance(dir.path());
        let history = History::default();
        let backup = create(&layout, &history, &options(&include())).unwrap();

        std::fs::write(layout.universe().join("memories.json"), b"after").unwrap();
        std::fs::write(layout.server_config(), b"{\"a\":2}").unwrap();

        let mut history = history;
        let restrict = Restrict::Only(vec!["config.json".to_string()]);
        restore(&layout, &mut history, &backup, &restrict).unwrap();

        assert_eq!(std::fs::read(layout.server_config()).unwrap(), b"{\"a\":1}");
        assert_eq!(
            std::fs::read(layout.universe().join("memories.json")).unwrap(),
            b"after",
            "the world was not asked for"
        );
    }

    #[test]
    fn restoring_starts_a_new_lineage() {
        let dir = tempfile::tempdir().unwrap();
        let layout = instance(dir.path());
        let mut history = History::default();
        let backup = create(&layout, &history, &options(&include())).unwrap();

        assert_eq!(history.current(), 1);
        restore(&layout, &mut history, &backup, &Restrict::World).unwrap();
        assert_eq!(history.current(), 2);

        // And it is durable, so a later `hy backup list` classifies correctly.
        let reread = History::read(&snapshot_dir(&layout)).unwrap();
        assert_eq!(reread.current(), 2);
    }

    #[test]
    fn restore_removes_files_the_backup_does_not_have() {
        let dir = tempfile::tempdir().unwrap();
        let layout = instance(dir.path());
        let history = History::default();
        let backup = create(&layout, &history, &options(&include())).unwrap();

        std::fs::write(layout.universe().join("stray.json"), b"added later").unwrap();

        let mut history = history;
        restore(&layout, &mut history, &backup, &Restrict::World).unwrap();

        // A restore is a replacement, not a merge; a leftover file would corrupt the world.
        assert!(!layout.universe().join("stray.json").exists());
    }

    #[test]
    fn restore_leaves_the_jar_alone() {
        let dir = tempfile::tempdir().unwrap();
        let layout = instance(dir.path());
        let history = History::default();
        let backup = create(&layout, &history, &options(&include())).unwrap();

        let mut history = history;
        restore(&layout, &mut history, &backup, &Restrict::World).unwrap();

        // It was excluded, so it is not the backup's business to put one back.
        assert_eq!(std::fs::read(layout.jar()).unwrap(), b"jar");
    }

    #[test]
    fn prune_keeps_the_newest_and_never_the_servers() {
        let dir = tempfile::tempdir().unwrap();
        let layout = instance(dir.path());
        let history = History::default();

        // Snapshots are dated from their id, so the archives need no real content here.
        std::fs::create_dir_all(snapshot_dir(&layout)).unwrap();
        for id in ["20260820-120000", "20260821-120000", "20260822-120000"] {
            std::fs::write(snapshot_dir(&layout).join(format!("{id}.tar.gz")), b"x").unwrap();
        }
        std::fs::create_dir_all(layout.server_backups()).unwrap();
        std::fs::write(
            layout.server_backups().join("2026-08-22_13-33-48.zip"),
            b"hot",
        )
        .unwrap();

        let removed = prune(&layout, &history, 2).unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].id, "20260820-120000", "the oldest goes first");

        let left = store::list(&layout, &history).unwrap();
        assert_eq!(
            left.iter().filter(|b| b.origin == Origin::Snapshot).count(),
            2
        );
        assert_eq!(
            left.iter().filter(|b| b.origin == Origin::Server).count(),
            1,
            "the server's own retention is its business"
        );
    }

    #[test]
    fn prune_below_the_limit_does_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let layout = instance(dir.path());
        let history = History::default();
        create(&layout, &history, &options(&include())).unwrap();
        assert!(prune(&layout, &history, 10).unwrap().is_empty());
    }

    #[test]
    fn an_instance_without_a_server_cannot_be_backed_up() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::new(dir.path());
        assert!(matches!(
            create(&layout, &History::default(), &options(&include())),
            Err(Error::NothingToBackUp(_))
        ));
    }
}
