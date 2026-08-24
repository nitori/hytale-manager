//! Fetching the server payload without running the jar.
//!
//! Recovered from the server's `UpdateService`. Two hops, because the bytes live behind a
//! signed URL rather than on an open path:
//!
//! ```text
//! GET {account-data}/game-assets/version/{patchline}.json   Bearer <token>
//!   -> {"url": …}  -> GET that -> {version, download_url, sha256}
//! GET {account-data}/game-assets/{download_url}             Bearer <token>
//!   -> {"url": …}  -> GET that -> the .zip
//! ```
//!
//! The signed URLs are short-lived, so each is requested immediately before it is used.

use std::path::{Path, PathBuf};

use hy_java::download::{Checksum, ProgressReporter, download_verified};
use serde::Deserialize;

use crate::error::{Error, Result};

const ACCOUNT_DATA_URL: &str = "https://account-data.hytale.com";

/// What a patchline currently publishes.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct VersionManifest {
    pub version: String,
    /// A key relative to `/game-assets/`, not a URL to fetch directly.
    pub download_url: String,
    pub sha256: String,
}

#[derive(Debug, Deserialize)]
struct SignedUrl {
    url: String,
}

/// Talks to the asset service on behalf of one authenticated account.
#[derive(Debug, Clone)]
pub struct PayloadClient {
    http: reqwest::Client,
    base: String,
    token: String,
}

impl PayloadClient {
    pub fn new(http: reqwest::Client, access_token: impl Into<String>) -> Self {
        Self {
            http,
            base: ACCOUNT_DATA_URL.to_owned(),
            token: access_token.into(),
        }
    }

    /// Point at a different asset service, for tests.
    pub fn with_base(mut self, base: impl Into<String>) -> Self {
        self.base = base.into().trim_end_matches('/').to_owned();
        self
    }

    /// Ask for a signed URL for `key`, relative to `/game-assets/`.
    async fn signed(&self, key: &str) -> Result<String> {
        let url = format!("{}/game-assets/{key}", self.base);
        let response = self
            .http
            .get(&url)
            .bearer_auth(&self.token)
            .header("Accept", "application/json")
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            return Err(Error::AssetService {
                what: "a signed URL",
                status: status.as_u16(),
                body: response.text().await.unwrap_or_default(),
            });
        }
        Ok(response.json::<SignedUrl>().await?.url)
    }

    /// What `patchline` currently publishes.
    pub async fn manifest(&self, patchline: &str) -> Result<VersionManifest> {
        let signed = self.signed(&format!("version/{patchline}.json")).await?;
        let response = self.http.get(&signed).send().await?;

        let status = response.status();
        if !status.is_success() {
            return Err(Error::AssetService {
                what: "the version manifest",
                status: status.as_u16(),
                body: response.text().await.unwrap_or_default(),
            });
        }
        Ok(response.json().await?)
    }

    /// Download the payload archive into `dest_dir`, verifying its digest.
    ///
    /// Reuses the JDK downloader, so a 3.3 GB transfer that drops resumes rather than
    /// starting over.
    pub async fn download(
        &self,
        manifest: &VersionManifest,
        dest_dir: &Path,
        progress: &dyn ProgressReporter,
    ) -> Result<PathBuf> {
        let signed = self.signed(&manifest.download_url).await?;
        let name = format!("hytale-{}.zip", manifest.version);
        Ok(download_verified(
            &self.http,
            &signed,
            &name,
            &Checksum::Sha256(manifest.sha256.clone()),
            None,
            dest_dir,
            progress,
        )
        .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The keys are the server's, not ours: `download_url` is snake_case even though the
    /// Java field beside it is not.
    #[test]
    fn a_manifest_parses_from_the_services_json() {
        let manifest: VersionManifest = serde_json::from_str(
            r#"{"version":"0.5.9","download_url":"builds/0.5.9/payload.zip","sha256":"abc"}"#,
        )
        .unwrap();
        assert_eq!(manifest.version, "0.5.9");
        assert_eq!(manifest.download_url, "builds/0.5.9/payload.zip");
    }

    /// A key we guessed wrong would only surface as a runtime decode failure, so the exact
    /// spelling is worth pinning.
    #[test]
    fn camel_case_keys_are_not_accepted() {
        assert!(
            serde_json::from_str::<VersionManifest>(
                r#"{"version":"0.5.9","downloadUrl":"x","sha256":"abc"}"#
            )
            .is_err()
        );
    }

    /// Keys are joined straight onto the base, so a trailing slash would produce
    /// `//game-assets/` and a signed-URL request that does not match.
    #[test]
    fn a_trailing_slash_on_the_base_is_dropped() {
        let client = PayloadClient::new(reqwest::Client::new(), "t").with_base("https://example/");
        assert_eq!(client.base, "https://example");
    }
}

/// Unpack the payload archive into `root`, the instance directory.
///
/// The archive carries the layout the server expects — `Assets.zip` and the launcher beside
/// a `Server/` directory — so entries are written where they name themselves rather than
/// being sorted by us. Paths are checked against escaping `root`, since an archive is
/// untrusted input.
pub fn extract_into(archive: &Path, root: &Path) -> Result<Vec<PathBuf>> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file))?;
    let mut written = Vec::new();

    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let Some(relative) = entry.enclosed_name() else {
            return Err(Error::UnsafeArchivePath(entry.name().to_owned()));
        };
        let target = root.join(&relative);

        if entry.is_dir() {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::fs::File::create(&target)?;
        std::io::copy(&mut entry, &mut out)?;

        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode));
        }
        written.push(relative.to_path_buf());
    }
    Ok(written)
}

#[cfg(test)]
mod extract_tests {
    use super::*;
    use std::io::Write;

    fn archive_with(entries: &[(&str, &[u8])]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("payload.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        for (name, body) in entries {
            zip.start_file::<_, ()>(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(body).unwrap();
        }
        zip.finish().unwrap();
        (dir, path)
    }

    #[test]
    fn the_archive_layout_is_preserved() {
        let (_src, archive) = archive_with(&[
            ("Assets.zip", b"assets"),
            ("Server/HytaleServer.jar", b"jar"),
        ]);
        let into = tempfile::tempdir().unwrap();
        let written = extract_into(&archive, into.path()).unwrap();

        assert_eq!(written.len(), 2);
        assert_eq!(
            std::fs::read(into.path().join("Assets.zip")).unwrap(),
            b"assets"
        );
        assert_eq!(
            std::fs::read(into.path().join("Server/HytaleServer.jar")).unwrap(),
            b"jar"
        );
    }

    /// An archive is untrusted input; a `../` entry must not be able to write outside the
    /// instance.
    #[test]
    fn an_escaping_entry_is_refused() {
        let (_src, archive) = archive_with(&[("../escaped", b"nope")]);
        let into = tempfile::tempdir().unwrap();
        assert!(extract_into(&archive, into.path()).is_err());
        assert!(!into.path().parent().unwrap().join("escaped").exists());
    }
}
