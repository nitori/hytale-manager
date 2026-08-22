//! Guessing whether the server is still writing.
//!
//! `hy run` holds a lock, and that is the authoritative answer — but only where locks work.
//! On the v9fs mounts WSL2 uses for Windows drives `flock` is unreliable, and a server
//! started from Windows holds a lock no Linux process can see at all. Either way the check
//! fails *open*, and a snapshot taken from a live world is torn.
//!
//! Recent writes under `Server/` are a weaker signal but an entirely portable one, so they
//! back the lock up rather than replace it.

use std::path::Path;
use std::time::Duration;

use hy_instance::Layout;
use jiff::Timestamp;

/// Long enough to span an idle server's pauses between writes, short enough that a server
/// stopped a moment ago does not keep looking alive.
pub const ACTIVE_WINDOW: Duration = Duration::from_secs(120);

/// The most recent write anywhere under `Server/`, ignoring our own output.
pub fn last_write(layout: &Layout) -> Option<Timestamp> {
    newest(&layout.server_dir())
}

/// Whether the server appears to have written recently.
pub fn looks_active(layout: &Layout, within: Duration) -> bool {
    let Some(last) = last_write(layout) else {
        return false;
    };
    let elapsed = Timestamp::now().as_millisecond() - last.as_millisecond();
    // A timestamp in the future means clock skew; treat it as recent rather than ancient.
    elapsed < 0 || (elapsed as u128) < within.as_millis()
}

fn newest(dir: &Path) -> Option<Timestamp> {
    let mut latest: Option<Timestamp> = None;
    let entries = std::fs::read_dir(dir).ok()?;

    for entry in entries.flatten() {
        let path = entry.path();
        let candidate = if path.is_dir() {
            newest(&path)
        } else {
            std::fs::metadata(&path)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|m| Timestamp::try_from(m).ok())
        };
        if let Some(candidate) = candidate
            && latest.is_none_or(|current| candidate > current)
        {
            latest = Some(candidate);
        }
    }
    latest
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(root: &Path) -> Layout {
        std::fs::create_dir_all(root.join("Server/universe")).unwrap();
        Layout::new(root)
    }

    #[test]
    fn a_freshly_written_world_looks_active() {
        let dir = tempfile::tempdir().unwrap();
        let layout = layout(dir.path());
        std::fs::write(layout.universe().join("memories.json"), b"just now").unwrap();

        assert!(looks_active(&layout, ACTIVE_WINDOW));
    }

    #[test]
    fn an_untouched_world_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let layout = layout(dir.path());
        std::fs::write(layout.universe().join("memories.json"), b"old").unwrap();

        // Nothing has been written within the last instant.
        assert!(!looks_active(&layout, Duration::from_millis(1)));
    }

    #[test]
    fn an_empty_server_directory_is_not_active() {
        let dir = tempfile::tempdir().unwrap();
        let layout = layout(dir.path());
        assert!(!looks_active(&layout, ACTIVE_WINDOW));
        assert!(last_write(&layout).is_none());
    }

    #[test]
    fn nested_writes_are_seen() {
        let dir = tempfile::tempdir().unwrap();
        let layout = layout(dir.path());
        let deep = layout.universe().join("worlds/default/resources");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("Time.json"), b"tick").unwrap();

        assert!(looks_active(&layout, ACTIVE_WINDOW));
    }
}
