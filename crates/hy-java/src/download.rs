//! Resumable, checksum-verified downloads.
//!
//! JDK archives are ~140 MB and the Hytale asset bundle is ~3.3 GB, so an interrupted
//! transfer must not start over. We keep a `.part` file and resume it with a range request.

use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::error::{Error, Result};

/// What a download is checked against.
#[derive(Debug, Clone)]
pub enum Checksum {
    Sha256(String),
}

impl Checksum {
    pub fn expected(&self) -> &str {
        match self {
            Self::Sha256(hex) => hex,
        }
    }

    async fn digest(&self, path: &Path) -> Result<String> {
        match self {
            Self::Sha256(_) => digest_file::<Sha256>(path).await,
        }
    }

    /// `None` when the file matches; the actual digest when it does not.
    async fn mismatch(&self, path: &Path) -> Result<Option<String>> {
        let actual = self.digest(path).await?;
        Ok((!actual.eq_ignore_ascii_case(self.expected())).then_some(actual))
    }
}

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
/// verify the result.
///
/// `expected_size` is optional because the asset service's manifest carries no size; it is
/// used only to discard an oversized `.part` and as a progress fallback.
///
/// Returns the path to the verified file. If a verified copy is already present it is
/// reused without touching the network.
pub async fn download_verified(
    http: &reqwest::Client,
    url: &str,
    name: &str,
    checksum: &Checksum,
    expected_size: Option<u64>,
    dest_dir: &Path,
    progress: &dyn ProgressReporter,
) -> Result<PathBuf> {
    tokio::fs::create_dir_all(dest_dir).await?;
    let final_path = dest_dir.join(name);
    let part_path = dest_dir.join(format!("{name}.part"));

    // Reuse a previously verified download.
    if final_path.is_file() {
        if checksum.mismatch(&final_path).await?.is_none() {
            tracing::debug!("reusing cached download at {}", final_path.display());
            return Ok(final_path);
        }
        tracing::warn!("cached download failed verification; re-fetching");
        tokio::fs::remove_file(&final_path).await?;
    }

    let mut have = match tokio::fs::metadata(&part_path).await {
        Ok(meta) if expected_size.is_none_or(|size| meta.len() < size) => meta.len(),
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
    let response = request.send().await?;

    // Without a known size a stale `.part` can exceed the file; the server then rejects the
    // range instead of serving it, so start over rather than failing the install.
    let response = if response.status() == reqwest::StatusCode::RANGE_NOT_SATISFIABLE && have > 0 {
        tracing::debug!("stale partial download for {name}; restarting");
        let _ = tokio::fs::remove_file(&part_path).await;
        have = 0;
        http.get(url).send().await?.error_for_status()?
    } else {
        response.error_for_status()?
    };

    // A server that ignores the range header replies 200 with the whole body; in that case
    // discard what we had rather than concatenating garbage.
    let resuming = response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    if have > 0 && !resuming {
        tracing::debug!("server ignored range request; restarting download");
        have = 0;
    }

    let total = response
        .content_length()
        .map_or(expected_size.unwrap_or(0), |len| len + have);
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

    if let Some(actual) = checksum.mismatch(&part_path).await? {
        // Remove the bad file so a retry does not resume from it.
        let _ = tokio::fs::remove_file(&part_path).await;
        return Err(Error::ChecksumMismatch {
            name: name.to_string(),
            expected: checksum.expected().to_string(),
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

async fn digest_file<D: Digest>(path: &Path) -> Result<String> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = D::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut buf).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sha256_matches_the_reference_digest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("payload");
        tokio::fs::write(&path, b"abc").await.unwrap();

        assert_eq!(
            digest_file::<Sha256>(&path).await.unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// More than one read buffer, and not a whole number of them, so a dropped tail or a
    /// mishandled short read shows up rather than hiding behind an aligned size.
    #[tokio::test]
    async fn sha256_spans_multiple_read_buffers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large");
        tokio::fs::write(&path, vec![0u8; (1 << 20) + 7])
            .await
            .unwrap();

        assert_eq!(
            digest_file::<Sha256>(&path).await.unwrap(),
            "8cd66c0067f5824edbd967efc4f03d328c6a58727b96b37736e26638eba47fb0"
        );
    }

    #[test]
    fn hex_pads_single_digit_bytes() {
        assert_eq!(hex(&[0x00, 0x0f, 0xff]), "000fff");
    }
}
