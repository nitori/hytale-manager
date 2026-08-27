//! Checksums, in both the whole-file and the incremental form.

use std::path::Path;

use sha2::{Digest as _, Sha256};
use tokio::io::AsyncReadExt;

use crate::error::Result;

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

    pub fn matches(&self, actual: &str) -> bool {
        actual.eq_ignore_ascii_case(self.expected())
    }

    /// An empty hasher for the same algorithm.
    pub fn digester(&self) -> Digester {
        match self {
            Self::Sha256(_) => Digester::sha256(),
        }
    }

    pub async fn digest(&self, path: &Path) -> Result<String> {
        digest_file(path, self.digester()).await
    }

    /// `None` when the file matches; the actual digest when it does not.
    pub async fn mismatch(&self, path: &Path) -> Result<Option<String>> {
        let actual = self.digest(path).await?;
        Ok((!self.matches(&actual)).then_some(actual))
    }
}

/// A hasher fed as the bytes arrive, so a transfer that starts from zero is verified
/// without reading the file back — which for a multi-gigabyte payload is the difference
/// between one pass over it and two.
pub enum Digester {
    Sha256(Sha256),
}

impl Digester {
    pub fn sha256() -> Self {
        Self::Sha256(Sha256::new())
    }

    pub fn update(&mut self, bytes: &[u8]) {
        match self {
            Self::Sha256(hasher) => hasher.update(bytes),
        }
    }

    pub fn finish(self) -> String {
        match self {
            Self::Sha256(hasher) => hex(&hasher.finalize()),
        }
    }
}

pub async fn digest_file(path: &Path, mut digester: Digester) -> Result<String> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut buf).await?;
        if read == 0 {
            break;
        }
        digester.update(&buf[..read]);
    }
    Ok(digester.finish())
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

    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[tokio::test]
    async fn sha256_matches_the_reference_digest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("payload");
        tokio::fs::write(&path, b"abc").await.unwrap();

        assert_eq!(digest_file(&path, Digester::sha256()).await.unwrap(), ABC);
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
            digest_file(&path, Digester::sha256()).await.unwrap(),
            "8cd66c0067f5824edbd967efc4f03d328c6a58727b96b37736e26638eba47fb0"
        );
    }

    /// The incremental and whole-file forms must not be able to disagree: the resumed path
    /// verifies with one and the fresh path with the other.
    #[tokio::test]
    async fn incremental_and_whole_file_digests_agree() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chunked");
        tokio::fs::write(&path, b"abc").await.unwrap();

        let mut digester = Digester::sha256();
        digester.update(b"a");
        digester.update(b"bc");

        assert_eq!(digester.finish(), ABC);
        assert_eq!(digest_file(&path, Digester::sha256()).await.unwrap(), ABC);
    }

    #[test]
    fn hex_pads_single_digit_bytes() {
        assert_eq!(hex(&[0x00, 0x0f, 0xff]), "000fff");
    }

    #[test]
    fn checksum_comparison_ignores_digest_case() {
        let checksum = Checksum::Sha256(ABC.to_uppercase());
        assert!(checksum.matches(ABC));
    }
}
