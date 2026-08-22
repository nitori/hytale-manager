//! Finding backups, ours and the server's.
//!
//! The server writes its own every `--backup-frequency` minutes into `Server/backups/`, as
//! `YYYY-MM-DD_HH-MM-SS.zip` holding the contents of `universe/` — world data only, no
//! config, bans, whitelist, or mods. It has no restore command, so those archives are
//! write-only unless something else reads them. Listing both origins in one place is the
//! point: "what can I roll back to" is one question.

use std::path::{Path, PathBuf};

use hy_instance::Layout;
use jiff::civil;

use crate::error::Result;
use crate::history::History;
use crate::manifest::Manifest;
use crate::{archive, manifest};

pub const SNAPSHOT_DIR: &str = "snapshots";
const SNAPSHOT_SUFFIX: &str = ".tar.gz";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Taken by `hy backup create`: the `[backup] include` entries under `Server/`.
    Snapshot,
    /// Taken by the server on its timer: world data only.
    Server,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Snapshot => "snapshot",
            Self::Server => "server",
        }
    }

    /// Whether restoring it brings back everything, or only the world.
    pub fn is_complete(self) -> bool {
        self == Self::Snapshot
    }
}

#[derive(Debug, Clone)]
pub struct Backup {
    pub id: String,
    pub path: PathBuf,
    pub origin: Origin,
    pub created: jiff::Timestamp,
    pub size: u64,
    pub lineage: u32,
    pub manifest: Option<Manifest>,
}

impl Backup {
    /// Whether this belongs to the history the instance is on now. A backup from before a
    /// restore is on an abandoned branch, and restoring it again discards everything since.
    pub fn is_current_lineage(&self, history: &History) -> bool {
        self.lineage == history.current()
    }
}

pub fn snapshot_dir(layout: &Layout) -> PathBuf {
    layout.root().join(SNAPSHOT_DIR)
}

/// An id from a timestamp: `20260822-143000`.
pub fn id_for(at: jiff::Timestamp) -> String {
    at.to_zoned(jiff::tz::TimeZone::UTC)
        .strftime("%Y%m%d-%H%M%S")
        .to_string()
}

/// Parse an id produced by [`id_for`], which is UTC.
///
/// Preferred over the file's mtime, which changes when an archive is copied around.
fn parse_snapshot_id(id: &str) -> Option<jiff::Timestamp> {
    let (date, time) = id.split_once('-')?;
    if date.len() != 8 || time.len() != 6 {
        return None;
    }
    let civil = civil::date(
        date[..4].parse().ok()?,
        date[4..6].parse().ok()?,
        date[6..].parse().ok()?,
    )
    .at(
        time[..2].parse().ok()?,
        time[2..4].parse().ok()?,
        time[4..].parse().ok()?,
        0,
    );
    jiff::tz::TimeZone::UTC
        .to_zoned(civil)
        .ok()
        .map(|zoned| zoned.timestamp())
}

/// Parse the server's `2026-08-22_13-33-48.zip` naming.
///
/// Those stamps carry no zone, so they are read as local time — the server wrote them with
/// the machine's clock.
fn parse_server_stamp(stem: &str) -> Option<jiff::Timestamp> {
    let (date, time) = stem.split_once('_')?;
    let mut date = date.split('-');
    let mut time = time.split('-');

    let civil = civil::date(
        date.next()?.parse().ok()?,
        date.next()?.parse().ok()?,
        date.next()?.parse().ok()?,
    )
    .at(
        time.next()?.parse().ok()?,
        time.next()?.parse().ok()?,
        time.next()?.parse().ok()?,
        0,
    );

    jiff::tz::TimeZone::system()
        .to_zoned(civil)
        .ok()
        .map(|zoned| zoned.timestamp())
}

/// Every backup available, newest first.
pub fn list(layout: &Layout, history: &History) -> Result<Vec<Backup>> {
    let mut backups = snapshots(layout, history)?;
    backups.extend(server_backups(layout, history)?);
    backups.sort_by_key(|b| std::cmp::Reverse(b.created));
    Ok(backups)
}

fn snapshots(layout: &Layout, history: &History) -> Result<Vec<Backup>> {
    let dir = snapshot_dir(layout);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };

    let mut backups = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_suffix(SNAPSHOT_SUFFIX) else {
            continue;
        };

        let manifest = archive::read_manifest(&path).unwrap_or(None);
        let created = manifest
            .as_ref()
            .map(|m| m.created)
            .or_else(|| parse_snapshot_id(id))
            .or_else(|| modified_at(&path))
            .unwrap_or_else(jiff::Timestamp::now);

        backups.push(Backup {
            id: id.to_string(),
            size: entry.metadata().map(|m| m.len()).unwrap_or(0),
            // A manifest states its lineage; without one, fall back to the journal.
            lineage: manifest
                .as_ref()
                .map_or_else(|| history.lineage_at(created), |m| m.lineage),
            origin: Origin::Snapshot,
            created,
            manifest,
            path,
        });
    }
    Ok(backups)
}

fn server_backups(layout: &Layout, history: &History) -> Result<Vec<Backup>> {
    let mut backups = Vec::new();
    // Older ones are rotated into `archive/` once `--backup-max-count` is exceeded.
    for dir in [layout.server_backups(), layout.server_backups().join("archive")] {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(stem) = name.strip_suffix(".zip") else {
                continue;
            };
            let path = entry.path();
            let created = parse_server_stamp(stem)
                .or_else(|| modified_at(&path))
                .unwrap_or_else(jiff::Timestamp::now);

            backups.push(Backup {
                id: stem.to_string(),
                size: entry.metadata().map(|m| m.len()).unwrap_or(0),
                lineage: history.lineage_at(created),
                origin: Origin::Server,
                created,
                manifest: None,
                path,
            });
        }
    }
    Ok(backups)
}

fn modified_at(path: &Path) -> Option<jiff::Timestamp> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    jiff::Timestamp::try_from(modified).ok()
}

pub fn find(layout: &Layout, history: &History, id: &str) -> Result<Option<Backup>> {
    Ok(list(layout, history)?.into_iter().find(|b| b.id == id))
}

pub fn path_for(layout: &Layout, id: &str) -> PathBuf {
    snapshot_dir(layout).join(format!("{id}{SNAPSHOT_SUFFIX}"))
}

pub use manifest::MANIFEST_NAME;

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(root: &Path) -> Layout {
        std::fs::create_dir_all(root.join("Server/backups/archive")).unwrap();
        std::fs::write(root.join("Assets.zip"), b"").unwrap();
        std::fs::write(root.join("Server/HytaleServer.jar"), b"").unwrap();
        Layout::new(root)
    }

    #[test]
    fn ids_are_utc_timestamps() {
        let at: jiff::Timestamp = "2026-08-22T14:30:00Z".parse().unwrap();
        assert_eq!(id_for(at), "20260822-143000");
    }

    /// The server's real naming, taken from a live install.
    #[test]
    fn server_stamps_are_parsed() {
        let parsed = parse_server_stamp("2026-08-22_13-33-48").unwrap();
        let local = parsed.to_zoned(jiff::tz::TimeZone::system());
        assert_eq!(local.strftime("%Y-%m-%d %H:%M:%S").to_string(), "2026-08-22 13:33:48");
    }

    #[test]
    fn malformed_stamps_are_rejected() {
        assert!(parse_server_stamp("not-a-stamp").is_none());
        assert!(parse_server_stamp("2026-08-22").is_none());
    }

    /// Our ids are UTC and the server's stamps are local, so the two cannot be ordered by
    /// their text. Both are normalised to instants first.
    #[test]
    fn both_origins_are_ordered_by_real_time_not_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let layout = layout(dir.path());
        std::fs::create_dir_all(snapshot_dir(&layout)).unwrap();
        std::fs::write(snapshot_dir(&layout).join("20260822-120000.tar.gz"), b"x").unwrap();
        std::fs::write(
            layout.server_backups().join("2026-08-22_13-33-48.zip"),
            b"y",
        )
        .unwrap();

        let listed = list(&layout, &History::default()).unwrap();
        assert_eq!(listed.len(), 2);

        let server = listed.iter().find(|b| b.origin == Origin::Server).unwrap();
        let snapshot = listed.iter().find(|b| b.origin == Origin::Snapshot).unwrap();
        assert_eq!(snapshot.created, "2026-08-22T12:00:00Z".parse().unwrap());
        // Whichever is genuinely later must sort first, regardless of how the names read.
        let expected_first = if server.created > snapshot.created {
            Origin::Server
        } else {
            Origin::Snapshot
        };
        assert_eq!(listed[0].origin, expected_first);
    }

    #[test]
    fn a_snapshot_is_dated_from_its_id_not_its_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let layout = layout(dir.path());
        std::fs::create_dir_all(snapshot_dir(&layout)).unwrap();
        // Not a readable archive, so there is no manifest to fall back on. An mtime would
        // be "now" and would reorder archives merely because they were copied.
        std::fs::write(snapshot_dir(&layout).join("20260101-000000.tar.gz"), b"x").unwrap();

        let listed = list(&layout, &History::default()).unwrap();
        assert_eq!(listed[0].created, "2026-01-01T00:00:00Z".parse().unwrap());
    }

    #[test]
    fn snapshot_ids_round_trip() {
        let at: jiff::Timestamp = "2026-08-22T14:30:00Z".parse().unwrap();
        assert_eq!(parse_snapshot_id(&id_for(at)), Some(at));
        assert!(parse_snapshot_id("nonsense").is_none());
        assert!(parse_snapshot_id("2026-08-22").is_none());
    }

    #[test]
    fn rotated_server_backups_are_found() {
        let dir = tempfile::tempdir().unwrap();
        let layout = layout(dir.path());
        std::fs::write(
            layout
                .server_backups()
                .join("archive/2026-08-20_10-00-00.zip"),
            b"old",
        )
        .unwrap();

        let listed = list(&layout, &History::default()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "2026-08-20_10-00-00");
    }

    #[test]
    fn unrelated_files_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let layout = layout(dir.path());
        std::fs::create_dir_all(snapshot_dir(&layout)).unwrap();
        std::fs::write(snapshot_dir(&layout).join("history.toml"), b"").unwrap();
        std::fs::write(snapshot_dir(&layout).join("half.part"), b"").unwrap();
        std::fs::write(layout.server_backups().join("notes.txt"), b"").unwrap();

        assert!(list(&layout, &History::default()).unwrap().is_empty());
    }

    #[test]
    fn a_server_backup_is_not_a_complete_restore() {
        // It carries `universe/` only — no config, bans, whitelist, or mods.
        assert!(!Origin::Server.is_complete());
        assert!(Origin::Snapshot.is_complete());
    }

    #[test]
    fn hot_backups_are_assigned_a_lineage_by_time() {
        let dir = tempfile::tempdir().unwrap();
        let layout = layout(dir.path());
        std::fs::write(
            layout.server_backups().join("2026-08-22_13-33-48.zip"),
            b"y",
        )
        .unwrap();

        let mut history = History::default();
        // A restore after that backup puts it on the abandoned branch.
        history.record_restore("x", "2026-08-23T00:00:00Z".parse().unwrap());

        let listed = list(&layout, &history).unwrap();
        assert_eq!(listed[0].lineage, 1);
        assert!(!listed[0].is_current_lineage(&history));
    }
}
