//! Reading and writing snapshot archives.
//!
//! Contents are taken from `Server/` and stored relative to it, choosing entries by
//! allowlist. A denylist would fail open — anything new the server starts writing lands in
//! every archive, which on a real install would have meant a 106 MB AOT config, a telemetry
//! spool, and a credential file.

use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;

use crate::error::Result;
use crate::manifest::{MANIFEST_NAME, Manifest};

/// Write a `.tar.gz` of the `include` entries under `source`, manifest first.
///
/// Missing entries are skipped rather than refused: `bans.json` does not exist until
/// somebody is banned.
pub fn create(
    source: &Path,
    include: &[String],
    manifest: &Manifest,
    destination: &Path,
) -> Result<PathBuf> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Written to a temporary name so an interrupted backup is never listed as a real one.
    let partial = destination.with_extension("part");
    let file = std::fs::File::create(&partial)?;
    let encoder = GzEncoder::new(file, Compression::default());
    let mut builder = tar::Builder::new(encoder);

    let toml = manifest.to_toml();
    let mut header = tar::Header::new_gnu();
    header.set_size(toml.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(manifest.created.as_second().max(0) as u64);
    header.set_cksum();
    builder.append_data(&mut header, MANIFEST_NAME, toml.as_bytes())?;

    for name in include {
        // A name with separators would let an entry escape `Server/`.
        let relative = Path::new(name);
        if relative.components().count() != 1 {
            tracing::warn!("ignoring `{name}` in [backup] include: not a plain name");
            continue;
        }

        let path = source.join(relative);
        if path.is_dir() {
            builder.append_dir(relative, &path)?;
            append_tree(&mut builder, source, relative)?;
        } else if path.is_file() {
            builder.append_path_with_name(&path, relative)?;
        } else {
            tracing::debug!("nothing to back up at {}", path.display());
        }
    }

    builder.into_inner()?.finish()?.flush()?;
    std::fs::rename(&partial, destination)?;
    Ok(destination.to_path_buf())
}

fn append_tree<W: Write>(
    builder: &mut tar::Builder<W>,
    root: &Path,
    relative: &Path,
) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(root.join(relative)) else {
        return Ok(());
    };

    for entry in entries.flatten() {
        let child = relative.join(entry.file_name());
        let path = entry.path();
        if path.is_dir() {
            builder.append_dir(&child, &path)?;
            append_tree(builder, root, &child)?;
        } else if path.is_file() {
            builder.append_path_with_name(&path, &child)?;
        }
    }
    Ok(())
}

fn open(archive: &Path) -> Result<tar::Archive<GzDecoder<std::fs::File>>> {
    let file = std::fs::File::open(archive)?;
    Ok(tar::Archive::new(GzDecoder::new(file)))
}

/// Read the manifest without decompressing the rest, which is why it is stored first.
pub fn read_manifest(archive: &Path) -> Result<Option<Manifest>> {
    let mut tar = open(archive)?;
    // Only the head is worth scanning; the manifest is always written first.
    let Some(entry) = tar.entries()?.next() else {
        return Ok(None);
    };
    let mut entry = entry?;
    if entry.path()?.as_ref() != Path::new(MANIFEST_NAME) {
        return Ok(None);
    }
    let mut text = String::new();
    entry.read_to_string(&mut text)?;
    Ok(Manifest::from_toml(&text))
}

/// The top-level names an archive covers, excluding the manifest.
///
/// Restoring replaces exactly these, so state the archive does not carry — the jar, the
/// AOT config — survives untouched.
pub fn covered_entries(archive: &Path) -> Result<Vec<PathBuf>> {
    let mut tar = open(archive)?;
    let mut names = Vec::new();
    for entry in tar.entries()? {
        let path = entry?.path()?.into_owned();
        if path.as_path() == Path::new(MANIFEST_NAME) {
            continue;
        }
        if let Some(Component::Normal(first)) = path.components().next() {
            let first = PathBuf::from(first);
            if !names.contains(&first) {
                names.push(first);
            }
        }
    }
    Ok(names)
}

/// Unpack everything except the manifest into `destination`.
pub fn extract(archive: &Path, destination: &Path) -> Result<()> {
    extract_matching(archive, destination, |_| true)
}

/// Unpack only entries under one of `wanted`, so a restore can roll back the world without
/// dragging back settings that were deliberately changed since.
pub fn extract_selected(archive: &Path, destination: &Path, wanted: &[PathBuf]) -> Result<()> {
    extract_matching(archive, destination, |path| {
        path.components()
            .next()
            .is_some_and(|first| wanted.iter().any(|w| w.as_path() == Path::new(&first)))
    })
}

fn extract_matching(
    archive: &Path,
    destination: &Path,
    keep: impl Fn(&Path) -> bool,
) -> Result<()> {
    std::fs::create_dir_all(destination)?;
    let mut tar = open(archive)?;
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        if path == Path::new(MANIFEST_NAME) || !keep(&path) {
            continue;
        }
        entry.unpack_in(destination)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        Manifest {
            id: "20260822-143000".to_string(),
            created: "2026-08-22T14:30:00Z".parse().unwrap(),
            lineage: 1,
            before_restore_of: None,
            server_version: Some("0.5.9".to_string()),
        }
    }

    fn server_tree(root: &Path) {
        std::fs::create_dir_all(root.join("universe/world")).unwrap();
        std::fs::write(root.join("universe/world/region.dat"), b"world").unwrap();
        std::fs::create_dir_all(root.join("mods")).unwrap();
        std::fs::write(root.join("mods/plugin.jar"), b"mod").unwrap();
        std::fs::write(root.join("config.json"), b"{}").unwrap();
        std::fs::write(root.join("HytaleServer.jar"), b"jar").unwrap();
        std::fs::create_dir_all(root.join("logs")).unwrap();
        std::fs::write(root.join("logs/latest.log"), b"noise").unwrap();
        std::fs::create_dir_all(root.join("backups")).unwrap();
        std::fs::write(root.join("backups/hot.zip"), b"hot").unwrap();
    }

    fn include() -> Vec<String> {
        ["universe", "mods", "config.json", "bans.json"]
            .map(str::to_string)
            .to_vec()
    }

    #[test]
    fn round_trips_a_server_tree() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("Server");
        server_tree(&source);
        let archive = dir.path().join("snap.tar.gz");

        create(&source, &include(), &manifest(), &archive).unwrap();

        let restored = dir.path().join("restored");
        extract(&archive, &restored).unwrap();

        assert_eq!(
            std::fs::read(restored.join("universe/world/region.dat")).unwrap(),
            b"world"
        );
        assert_eq!(std::fs::read(restored.join("mods/plugin.jar")).unwrap(), b"mod");
        assert!(restored.join("config.json").is_file());
    }

    #[test]
    fn unlisted_entries_stay_out() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("Server");
        server_tree(&source);
        let archive = dir.path().join("snap.tar.gz");
        create(&source, &include(), &manifest(), &archive).unwrap();

        let restored = dir.path().join("restored");
        extract(&archive, &restored).unwrap();

        assert!(!restored.join("logs").exists(), "logs are noise");
        // A file the operator never listed is never swept in.
        assert!(!restored.join("bans.json").exists());
        // The server's own hot backups must not be nested inside our snapshot.
        assert!(!restored.join("backups").exists());
        assert!(!restored.join("HytaleServer.jar").exists());
    }

    #[test]
    fn the_manifest_is_readable_without_unpacking() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("Server");
        server_tree(&source);
        let archive = dir.path().join("snap.tar.gz");
        create(&source, &include(), &manifest(), &archive).unwrap();

        let read = read_manifest(&archive).unwrap().unwrap();
        assert_eq!(read.id, "20260822-143000");
        assert_eq!(read.server_version.as_deref(), Some("0.5.9"));
    }

    #[test]
    fn the_manifest_is_not_unpacked_into_the_server() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("Server");
        server_tree(&source);
        let archive = dir.path().join("snap.tar.gz");
        create(&source, &include(), &manifest(), &archive).unwrap();

        let restored = dir.path().join("restored");
        extract(&archive, &restored).unwrap();
        assert!(!restored.join(MANIFEST_NAME).exists());
    }

    #[test]
    fn covered_entries_lists_what_a_restore_would_replace() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("Server");
        server_tree(&source);
        let archive = dir.path().join("snap.tar.gz");
        create(&source, &include(), &manifest(), &archive).unwrap();

        let mut covered = covered_entries(&archive).unwrap();
        covered.sort();
        assert_eq!(
            covered,
            [
                PathBuf::from("config.json"),
                PathBuf::from("mods"),
                PathBuf::from("universe")
            ]
        );
    }

    #[test]
    fn an_interrupted_write_leaves_no_archive() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("Server");
        server_tree(&source);
        let archive = dir.path().join("snap.tar.gz");
        create(&source, &include(), &manifest(), &archive).unwrap();

        // The `.part` file is renamed into place only once the stream is complete.
        assert!(!archive.with_extension("part").exists());
        assert!(archive.is_file());
    }
}
