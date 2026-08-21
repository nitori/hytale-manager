//! Resumable, checksum-verified downloads.
//!
//! JDK archives are ~140 MB and the Hytale asset bundle is ~3.3 GB, so an interrupted
//! transfer must not start over. We keep a `.part` file and resume it with a range request.

use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::error::{Error, Result};

/// Progress sink, so this crate does not depend on a particular terminal UI.
pub trait ProgressReporter: Send + Sync {
    fn start(&self, name: &str, total: u64);
    fn advance(&self, delta: u64);
    fn finish(&self);
}

/// A reporter that discards everything.
pub struct NoProgress;

impl ProgressReporter for NoProgress {
    fn start(&self, _name: &str, _total: u64) {}
    fn advance(&self, _delta: u64) {}
    fn finish(&self) {}
}

/// Download `url` into `dest_dir`, resuming a partial transfer if one is present, and
/// verify the result against `expected_sha256`.
///
/// Returns the path to the verified file. If a verified copy is already present it is
/// reused without touching the network.
pub async fn download_verified(
    http: &reqwest::Client,
    url: &str,
    name: &str,
    expected_sha256: &str,
    expected_size: u64,
    dest_dir: &Path,
    progress: &dyn ProgressReporter,
) -> Result<PathBuf> {
    tokio::fs::create_dir_all(dest_dir).await?;
    let final_path = dest_dir.join(name);
    let part_path = dest_dir.join(format!("{name}.part"));

    // Reuse a previously verified download.
    if final_path.is_file() {
        let actual = sha256_file(&final_path).await?;
        if actual.eq_ignore_ascii_case(expected_sha256) {
            tracing::debug!("reusing cached download at {}", final_path.display());
            return Ok(final_path);
        }
        tracing::warn!("cached download failed verification; re-fetching");
        tokio::fs::remove_file(&final_path).await?;
    }

    let mut have = match tokio::fs::metadata(&part_path).await {
        Ok(meta) if meta.len() < expected_size => meta.len(),
        // A `.part` at or beyond the expected size is junk; start clean.
        Ok(_) => {
            tokio::fs::remove_file(&part_path).await?;
            0
        }
        Err(_) => 0,
    };

    let mut request = http.get(url);
    if have > 0 {
        tracing::debug!("resuming {name} at {have} bytes");
        request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let response = request.send().await?.error_for_status()?;

    // A server that ignores the range header replies 200 with the whole body; in that case
    // discard what we had rather than concatenating garbage.
    let resuming = response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    if have > 0 && !resuming {
        tracing::debug!("server ignored range request; restarting download");
        have = 0;
    }

    let total = response
        .content_length()
        .map_or(expected_size, |len| len + have);
    progress.start(name, total);
    progress.advance(have);

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(!resuming)
        .open(&part_path)
        .await?;
    if resuming {
        file.seek(std::io::SeekFrom::End(0)).await?;
    }

    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        progress.advance(chunk.len() as u64);
    }
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    progress.finish();

    let actual = sha256_file(&part_path).await?;
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        // Remove the bad file so a retry does not resume from it.
        let _ = tokio::fs::remove_file(&part_path).await;
        return Err(Error::ChecksumMismatch {
            name: name.to_string(),
            expected: expected_sha256.to_string(),
            actual,
        });
    }

    // Defence in depth. The store lock normally makes this the only process writing this
    // `.part` file, but `flock` is unreliable on some filesystems (NFS, and the v9fs mounts
    // WSL2 uses for Windows drives). If another process got here first, its result is
    // already verified, so adopt it rather than failing.
    if let Err(err) = tokio::fs::rename(&part_path, &final_path).await
        && !final_path.is_file()
    {
        return Err(err.into());
    }

    Ok(final_path)
}

async fn sha256_file(path: &Path) -> Result<String> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut buf).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}
