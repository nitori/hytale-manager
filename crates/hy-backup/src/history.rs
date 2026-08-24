//! The lineage journal.
//!
//! Restoring forks the history: backups taken after an older state was restored do not
//! descend from the ones taken before it. Listing by timestamp alone hides that, and the
//! consequence is not cosmetic — someone restoring "the most recent backup" after a
//! rollback can silently land on the abandoned branch.
//!
//! Every restore is recorded here. A backup's lineage is whichever one was in effect when
//! it was taken, so the server's own hot backups get classified too, without needing it to
//! cooperate or us to write anything into its files.

use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

pub const HISTORY_FILE: &str = "history.toml";

/// Lineage 1 is the history an instance starts with, before any restore.
pub const INITIAL_LINEAGE: u32 = 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct History {
    #[serde(default, rename = "restore")]
    pub restores: Vec<Restore>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Restore {
    pub at: Timestamp,
    /// The backup that was restored.
    pub from: String,
    /// The lineage that began at this point.
    pub lineage: u32,
}

impl History {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join(HISTORY_FILE)
    }

    pub fn read(dir: &Path) -> Result<Self> {
        let path = Self::path(dir);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Ok(Self::default());
        };
        toml::from_str(&text).map_err(|source| Error::Parse {
            path,
            source: Box::new(source),
        })
    }

    pub fn write(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        let text = toml::to_string_pretty(self).unwrap_or_default();
        let path = Self::path(dir);
        let temporary = path.with_extension("toml.tmp");
        std::fs::write(&temporary, text)?;
        std::fs::rename(&temporary, &path)?;
        Ok(())
    }

    /// The lineage the instance is on now.
    pub fn current(&self) -> u32 {
        self.restores
            .last()
            .map_or(INITIAL_LINEAGE, |restore| restore.lineage)
    }

    /// The lineage in effect at `at` — which is the lineage of a backup taken then.
    ///
    /// Restores are compared by time rather than recorded per backup, so this classifies
    /// the server's hot backups as well as our own.
    pub fn lineage_at(&self, at: Timestamp) -> u32 {
        self.restores
            .iter()
            .rfind(|restore| restore.at <= at)
            .map_or(INITIAL_LINEAGE, |restore| restore.lineage)
    }

    /// Record a restore, starting a new lineage.
    pub fn record_restore(&mut self, from: &str, at: Timestamp) -> u32 {
        let lineage = self
            .restores
            .iter()
            .map(|restore| restore.lineage)
            .max()
            .unwrap_or(INITIAL_LINEAGE)
            + 1;
        self.restores.push(Restore {
            at,
            from: from.to_string(),
            lineage,
        });
        lineage
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    #[test]
    fn an_untouched_instance_is_on_the_first_lineage() {
        let history = History::default();
        assert_eq!(history.current(), INITIAL_LINEAGE);
        assert_eq!(
            history.lineage_at(at("2026-08-22T12:00:00Z")),
            INITIAL_LINEAGE
        );
    }

    #[test]
    fn a_restore_starts_a_new_lineage() {
        let mut history = History::default();
        assert_eq!(history.record_restore("a", at("2026-08-22T12:00:00Z")), 2);
        assert_eq!(history.current(), 2);
        assert_eq!(history.record_restore("b", at("2026-08-23T12:00:00Z")), 3);
        assert_eq!(history.current(), 3);
    }

    /// The case the journal exists for: after restoring an old state, the backups taken
    /// between it and the restore belong to an abandoned branch.
    #[test]
    fn backups_are_classified_by_when_they_were_taken() {
        let mut history = History::default();
        history.record_restore("b1", at("2026-08-22T12:00:00Z"));

        // Taken before the restore: the old branch.
        assert_eq!(history.lineage_at(at("2026-08-21T09:00:00Z")), 1);
        assert_eq!(history.lineage_at(at("2026-08-22T11:59:59Z")), 1);
        // Taken after: the branch that continues.
        assert_eq!(history.lineage_at(at("2026-08-22T12:00:01Z")), 2);
        assert_eq!(history.lineage_at(at("2026-09-01T09:00:00Z")), 2);
    }

    #[test]
    fn a_backup_taken_exactly_at_a_restore_belongs_to_the_new_lineage() {
        let mut history = History::default();
        history.record_restore("b1", at("2026-08-22T12:00:00Z"));
        assert_eq!(history.lineage_at(at("2026-08-22T12:00:00Z")), 2);
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let mut history = History::default();
        history.record_restore("20260820-120000", at("2026-08-22T12:00:00Z"));
        history.write(dir.path()).unwrap();

        let read = History::read(dir.path()).unwrap();
        assert_eq!(read.restores.len(), 1);
        assert_eq!(read.restores[0].from, "20260820-120000");
        assert_eq!(read.current(), 2);
    }

    #[test]
    fn a_missing_journal_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(History::read(dir.path()).unwrap().restores.is_empty());
    }
}
