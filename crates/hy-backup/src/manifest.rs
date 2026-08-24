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
