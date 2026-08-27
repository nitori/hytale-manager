//! Where `hy` keeps its own files, as opposed to a server instance's.

use std::path::{Path, PathBuf};

use etcetera::BaseStrategy;

use crate::error::Result;

/// `hy`'s own directory: managed Java runtimes and the download cache.
///
/// `HY_HOME` overrides it, which is what makes a test or a CI job self-contained.
pub fn home() -> Result<PathBuf> {
    if let Some(home) = std::env::var_os("HY_HOME") {
        return Ok(PathBuf::from(home));
    }
    let strategy =
        etcetera::choose_base_strategy().map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(strategy.data_dir().join("hy"))
}

/// Verified downloads, keyed by file name. Shared by the JDK and server-payload fetches, so
/// a resumed `.part` is found again whichever command started it.
pub fn cache_dir(home: &Path) -> PathBuf {
    home.join("cache").join("downloads")
}
