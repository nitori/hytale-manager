//! Applying a staged update.
//!
//! The server downloads updates into `updater/staging/` and exits with code 8; applying
//! them is the launcher's job. The file set below is exactly what `start.sh` copies —
//! deliberately selective, since `Server/` also holds config, saves, and mods that must
//! survive.

use std::path::Path;

use hy_instance::Layout;

use crate::error::{Error, Result};

/// Returns whether anything was applied.
pub fn apply(layout: &Layout) -> Result<bool> {
    let staging = layout.staging();
    if !layout.has_staged_update() {
        return Ok(false);
    }

    let server = layout.server_dir();
    copy_file(&staging.join("Server/HytaleServer.jar"), &layout.jar())?;

    // Replaced wholesale rather than merged: a licence dropped upstream must not linger.
    let licences = staging.join("Server/Licenses");
    if licences.is_dir() {
        let target = server.join("Licenses");
        if target.is_dir() {
            std::fs::remove_dir_all(&target).map_err(Error::Staging)?;
        }
        copy_dir(&licences, &target)?;
    }

    for name in ["Assets.zip", "start.sh", "start.bat"] {
        let source = staging.join(name);
        if source.is_file() {
            copy_file(&source, &layout.root().join(name))?;
        }
    }

    std::fs::remove_dir_all(&staging).map_err(Error::Staging)?;
    Ok(true)
}

fn copy_file(source: &Path, target: &Path) -> Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(Error::Staging)?;
    }
    std::fs::copy(source, target).map_err(Error::Staging)?;
    Ok(())
}

fn copy_dir(source: &Path, target: &Path) -> Result<()> {
    std::fs::create_dir_all(target).map_err(Error::Staging)?;
    for entry in std::fs::read_dir(source).map_err(Error::Staging)? {
        let entry = entry.map_err(Error::Staging)?;
        let to = target.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            copy_file(&entry.path(), &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instance(root: &Path) -> Layout {
        std::fs::create_dir_all(root.join("Server")).unwrap();
        std::fs::write(root.join("Assets.zip"), b"old assets").unwrap();
        std::fs::write(root.join("Server/HytaleServer.jar"), b"old jar").unwrap();
        std::fs::write(root.join("start.sh"), b"old script").unwrap();
        Layout::new(root)
    }

    fn stage(root: &Path) {
        std::fs::create_dir_all(root.join("updater/staging/Server")).unwrap();
        std::fs::write(
            root.join("updater/staging/Server/HytaleServer.jar"),
            b"new jar",
        )
        .unwrap();
    }

    #[test]
    fn nothing_staged_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let layout = instance(dir.path());
        assert!(!apply(&layout).unwrap());
    }

    #[test]
    fn a_staged_jar_replaces_the_installed_one() {
        let dir = tempfile::tempdir().unwrap();
        let layout = instance(dir.path());
        stage(dir.path());

        assert!(apply(&layout).unwrap());
        assert_eq!(std::fs::read(layout.jar()).unwrap(), b"new jar");
        assert!(!layout.staging().exists(), "staging should be cleared");
    }

    #[test]
    fn saves_config_and_mods_survive() {
        let dir = tempfile::tempdir().unwrap();
        let layout = instance(dir.path());
        std::fs::create_dir_all(layout.universe()).unwrap();
        std::fs::write(layout.universe().join("world.dat"), b"precious").unwrap();
        std::fs::write(layout.server_config(), b"{}").unwrap();
        std::fs::create_dir_all(layout.mods()).unwrap();
        std::fs::write(layout.mods().join("mod.jar"), b"mod").unwrap();
        stage(dir.path());

        apply(&layout).unwrap();

        assert_eq!(
            std::fs::read(layout.universe().join("world.dat")).unwrap(),
            b"precious"
        );
        assert!(layout.server_config().is_file());
        assert!(layout.mods().join("mod.jar").is_file());
    }

    #[test]
    fn assets_and_scripts_are_replaced_when_staged() {
        let dir = tempfile::tempdir().unwrap();
        let layout = instance(dir.path());
        stage(dir.path());
        std::fs::write(dir.path().join("updater/staging/Assets.zip"), b"new assets").unwrap();
        std::fs::write(dir.path().join("updater/staging/start.sh"), b"new script").unwrap();

        apply(&layout).unwrap();

        assert_eq!(std::fs::read(layout.assets()).unwrap(), b"new assets");
        assert_eq!(
            std::fs::read(dir.path().join("start.sh")).unwrap(),
            b"new script"
        );
    }

    #[test]
    fn assets_are_kept_when_the_update_omits_them() {
        let dir = tempfile::tempdir().unwrap();
        let layout = instance(dir.path());
        stage(dir.path());

        apply(&layout).unwrap();

        // A jar-only update must not blank the 3.3 GB asset bundle.
        assert_eq!(std::fs::read(layout.assets()).unwrap(), b"old assets");
    }

    #[test]
    fn licences_are_replaced_not_merged() {
        let dir = tempfile::tempdir().unwrap();
        let layout = instance(dir.path());
        let licences = layout.server_dir().join("Licenses");
        std::fs::create_dir_all(&licences).unwrap();
        std::fs::write(licences.join("dropped.txt"), b"old").unwrap();

        stage(dir.path());
        let staged = dir.path().join("updater/staging/Server/Licenses/nested");
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(staged.join("kept.txt"), b"new").unwrap();

        apply(&layout).unwrap();

        assert!(!licences.join("dropped.txt").exists());
        assert_eq!(
            std::fs::read(licences.join("nested/kept.txt")).unwrap(),
            b"new"
        );
    }

    #[test]
    fn a_staging_dir_without_a_jar_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let layout = instance(dir.path());
        std::fs::create_dir_all(layout.staging().join("Server")).unwrap();
        std::fs::write(layout.staging().join("Assets.zip"), b"partial").unwrap();

        // A half-finished download must not be applied.
        assert!(!apply(&layout).unwrap());
        assert_eq!(std::fs::read(layout.assets()).unwrap(), b"old assets");
    }
}
