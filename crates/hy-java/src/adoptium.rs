//! Client for the Eclipse Adoptium API (<https://api.adoptium.net>).
//!
//! Two endpoints carry everything we need, both public and unauthenticated:
//!
//! ```text
//! /v3/info/available_releases
//!     → available_releases, available_lts_releases, most_recent_lts,
//!       most_recent_feature_release
//!
//! /v3/assets/feature_releases/{feature}/ga?os&architecture&image_type=jdk&vendor=eclipse
//!     → releases newest-first, each with a download URL, sha256, and size
//! ```

use serde::Deserialize;

use crate::error::Result;
use crate::platform::{Arch, Os};
use crate::version::JavaVersion;

const API_ROOT: &str = "https://api.adoptium.net/v3";
const USER_AGENT: &str = concat!("hy/", env!("CARGO_PKG_VERSION"));

/// How many releases to consider when resolving a specific patch version. Adoptium keeps
/// far more than this per feature release; we only ever want a recent one.
const PAGE_SIZE: u32 = 20;

pub struct AdoptiumClient {
    http: reqwest::Client,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AvailableReleases {
    pub available_releases: Vec<u32>,
    pub available_lts_releases: Vec<u32>,
    pub most_recent_lts: u32,
    pub most_recent_feature_release: u32,
}

/// A downloadable JDK archive.
#[derive(Debug, Clone)]
pub struct ReleaseAsset {
    pub version: JavaVersion,
    pub name: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

impl AdoptiumClient {
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder().user_agent(USER_AGENT).build()?;
        Ok(Self { http })
    }

    pub async fn available_releases(&self) -> Result<AvailableReleases> {
        let url = format!("{API_ROOT}/info/available_releases");
        tracing::debug!("GET {url}");
        let response = self.http.get(&url).send().await?.error_for_status()?;
        Ok(response.json().await?)
    }

    /// Generally-available releases for one feature version, newest first.
    pub async fn feature_release(
        &self,
        feature: u32,
        os: Os,
        arch: Arch,
        vendor: &str,
    ) -> Result<Vec<ReleaseAsset>> {
        let url = format!(
            "{API_ROOT}/assets/feature_releases/{feature}/ga\
             ?os={os}&architecture={arch}&image_type=jdk&jvm_impl=hotspot\
             &vendor={vendor}&page_size={PAGE_SIZE}&sort_order=DESC",
            os = os.adoptium(),
            arch = arch.adoptium(),
        );
        tracing::debug!("GET {url}");
        let response = self.http.get(&url).send().await?.error_for_status()?;
        let releases: Vec<RawRelease> = response.json().await?;

        Ok(releases
            .into_iter()
            .filter_map(|release| {
                let version = JavaVersion::from_release_name(&release.release_name)?;
                // Each release carries one binary for the os/arch we filtered on.
                let binary = release.binaries.into_iter().next()?;
                Some(ReleaseAsset {
                    version,
                    name: binary.package.name,
                    url: binary.package.link,
                    sha256: binary.package.checksum,
                    size: binary.package.size,
                })
            })
            .collect())
    }
}

#[derive(Debug, Deserialize)]
struct RawRelease {
    release_name: String,
    binaries: Vec<RawBinary>,
}

#[derive(Debug, Deserialize)]
struct RawBinary {
    package: RawPackage,
}

#[derive(Debug, Deserialize)]
struct RawPackage {
    name: String,
    link: String,
    checksum: String,
    size: u64,
}
