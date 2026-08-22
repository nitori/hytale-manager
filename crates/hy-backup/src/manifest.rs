//! The record stored inside each archive.
//!
//! Written as the first entry so listing reads only the head of the stream rather than
//! decompressing a whole world. Keeping it inside rather than beside means an archive
//! copied off the machine still describes itself.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

pub const MANIFEST_NAME: &str = "hy-backup.toml";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub created: Timestamp,
    pub lineage: u32,

    /// Set when this snapshot was taken automatically just before a restore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_restore_of: Option<String>,

    /// The installed server version, if the instance recorded one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_version: Option<String>,
}

impl Manifest {
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    pub fn from_toml(text: &str) -> Option<Self> {
        toml::from_str(text).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let manifest = Manifest {
            id: "20260822-143000".to_string(),
            created: "2026-08-22T14:30:00Z".parse().unwrap(),
            lineage: 2,
            before_restore_of: Some("20260820-120000".to_string()),
            server_version: Some("0.5.9".to_string()),
        };

        let parsed = Manifest::from_toml(&manifest.to_toml()).unwrap();
        assert_eq!(parsed.id, manifest.id);
        assert_eq!(parsed.lineage, 2);
        assert_eq!(parsed.server_version.as_deref(), Some("0.5.9"));
        assert_eq!(parsed.before_restore_of.as_deref(), Some("20260820-120000"));
    }

    #[test]
    fn optional_fields_are_omitted() {
        let manifest = Manifest {
            id: "20260822-143000".to_string(),
            created: "2026-08-22T14:30:00Z".parse().unwrap(),
            lineage: 1,
            before_restore_of: None,
            server_version: None,
        };
        let text = manifest.to_toml();
        assert!(!text.contains("before_restore_of"), "{text}");
        assert!(Manifest::from_toml(&text).is_some());
    }

    #[test]
    fn garbage_is_rejected_rather_than_guessed_at() {
        assert!(Manifest::from_toml("not toml at all {{{").is_none());
    }
}
