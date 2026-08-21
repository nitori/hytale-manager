//! The `.java-version` pin file.
//!
//! `hytale.toml` states what the server *needs* (`>=25`); `.java-version` records what this
//! instance *uses*. Same split uv draws between `requires-python` and `.python-version`.
//!
//! The pin is deliberately **portable** — `temurin-25.0.4.1+1`, never the full install key
//! `temurin-25.0.4.1+1-linux-x86_64`. Instances get moved between machines and
//! architectures, so OS and arch are resolved locally at use time.
//!
//! `.java-version` is also the jenv convention, so other tooling reads the same file.

use std::path::{Path, PathBuf};

use crate::distribution::JavaDistribution;
use crate::error::Result;
use crate::request::VersionRequest;
use crate::version::JavaVersion;

pub const PIN_FILE: &str = ".java-version";

pub fn path(dir: &Path) -> PathBuf {
    dir.join(PIN_FILE)
}

/// The raw pin string, ignoring comments and blank lines.
pub fn read_raw(dir: &Path) -> Result<Option<String>> {
    let path = path(dir);
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    Ok(contents
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string))
}

/// The pin parsed as a version request, or `None` if there is no pin file.
///
/// A malformed pin is an error rather than a silent fallback: the operator wrote it down
/// on purpose, and quietly ignoring it would start the wrong JVM.
pub fn read(dir: &Path) -> Result<Option<VersionRequest>> {
    match read_raw(dir)? {
        Some(raw) => Ok(Some(raw.parse()?)),
        None => Ok(None),
    }
}

/// The portable pin string for a resolved installation.
pub fn value(distribution: JavaDistribution, version: &JavaVersion) -> String {
    format!("{distribution}-{version}")
}

pub fn write(dir: &Path, pin: &str) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(path(dir), format!("{pin}\n"))?;
    Ok(())
}

/// Write the pin only if one is not already present.
///
/// Returns whether a file was written.
pub fn write_if_absent(dir: &Path, pin: &str) -> Result<bool> {
    if path(dir).exists() {
        return Ok(false);
    }
    write(dir, pin)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_value_is_portable() {
        let version: JavaVersion = "25.0.4.1+1".parse().unwrap();
        let pin = value(JavaDistribution::Temurin, &version);
        assert_eq!(pin, "temurin-25.0.4.1+1");
        // No OS or architecture, so the file survives a move between machines.
        assert!(!pin.contains("linux"));
        assert!(!pin.contains("x86_64"));
        // And it parses back into a request.
        assert!(pin.parse::<VersionRequest>().is_ok());
    }

    #[test]
    fn reads_skipping_comments() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(path(dir.path()), "# chosen by hy\n\ntemurin-25.0.4.1+1\n").unwrap();
        assert_eq!(
            read_raw(dir.path()).unwrap().as_deref(),
            Some("temurin-25.0.4.1+1")
        );
    }

    #[test]
    fn write_if_absent_does_not_clobber() {
        let dir = tempfile::tempdir().unwrap();
        assert!(write_if_absent(dir.path(), "25").unwrap());
        assert!(!write_if_absent(dir.path(), "26").unwrap());
        assert_eq!(read_raw(dir.path()).unwrap().as_deref(), Some("25"));
    }
}
