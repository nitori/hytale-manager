//! Minimal `hytale.toml` reading.
//!
//! Only the `[java] version` requirement is needed in phase 1. The `hy-instance` crate
//! takes ownership of instance configuration in phase 2; this is deliberately the smallest
//! thing that lets the Java resolution consult the instance's requirement.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use hy_java::VersionRequest;
use serde::Deserialize;

pub const CONFIG_FILE: &str = "hytale.toml";

#[derive(Debug, Default, Deserialize)]
struct InstanceConfig {
    #[serde(default)]
    java: JavaSection,
}

#[derive(Debug, Default, Deserialize)]
struct JavaSection {
    version: Option<String>,
}

/// The `[java] version` requirement for an instance, with the file it came from.
///
/// Returns `None` when there is no config file or it states no requirement.
pub fn java_requirement(dir: &Path) -> Result<Option<(VersionRequest, PathBuf)>> {
    let path = dir.join(CONFIG_FILE);
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };

    let config: InstanceConfig = toml::from_str(&contents)
        .with_context(|| format!("failed to parse {}", path.display()))?;

    let Some(version) = config.java.version else {
        return Ok(None);
    };

    let request = version.parse::<VersionRequest>().with_context(|| {
        format!("invalid `[java] version` in {}: `{version}`", path.display())
    })?;

    Ok(Some((request, path)))
}
