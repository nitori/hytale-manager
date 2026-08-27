//! The two-stage Java resolution.
//!
//! **Stage A — which version is wanted.** First match wins:
//!   1. `-j` / `--java` on the command line
//!   2. `.java-version` in the instance directory
//!   3. the `[java] version` requirement in `hytale.toml`
//!   4. the built-in default, `>=25`
//!
//! **Stage B — how to satisfy it.** First match wins:
//!   1. the managed store
//!   2. a system installation, *if* it satisfies the requirement
//!   3. an automatic download
//!
//! An open requirement such as `>=25` resolves to the newest **LTS** that satisfies it, not
//! the newest release overall. Java 26 is generally available, but the Hytale manual
//! specifies 25, and non-LTS releases stop receiving patches after roughly six months.
//! `latest` opts in deliberately.
//!
//! Note the asymmetry: the LTS preference governs what we *install*. An already-present
//! Java 26 still satisfies `>=25` — we do not download a second JDK to avoid one that is
//! already there and valid.

use std::cmp::Reverse;
use std::path::{Path, PathBuf};

use crate::adoptium::{AdoptiumClient, AvailableReleases, ReleaseAsset};
use crate::discovery::{self, SystemJava};
use crate::distribution::JavaDistribution;
use hy_fetch::ProgressReporter;
use crate::error::{Error, Result};
use crate::key::InstallKey;
use crate::pin;
use crate::platform::{Arch, Os};
use crate::request::{VersionRequest, VersionSpec};
use crate::store::{ManagedInstall, Store};
use crate::version::{JavaVersion, KNOWN_LTS};

/// Whether `hy` may install a JDK on its own initiative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DownloadPolicy {
    /// Install whatever is needed, whenever it is needed.
    #[default]
    Automatic,
    /// Only `hy java install` may download.
    Manual,
    /// Never download.
    Never,
}

impl std::str::FromStr for DownloadPolicy {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "automatic" | "auto" | "always" => Ok(Self::Automatic),
            "manual" => Ok(Self::Manual),
            "never" => Ok(Self::Never),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ResolveOptions {
    pub downloads: DownloadPolicy,
    pub offline: bool,
    /// True when the user explicitly asked for an install, which unlocks
    /// [`DownloadPolicy::Manual`].
    pub explicit_install: bool,
}

/// Where a resolved JVM came from.
#[derive(Debug, Clone)]
pub enum JavaSource {
    Managed { key: InstallKey, fresh: bool },
    System { vendor: Option<String> },
}

/// A system JDK that was found but did not satisfy the requirement. Reported so the
/// operator understands why a download happened.
#[derive(Debug, Clone)]
pub struct RejectedJava {
    pub home: PathBuf,
    pub version: JavaVersion,
}

#[derive(Debug, Clone)]
pub struct ResolvedJava {
    pub home: PathBuf,
    pub executable: PathBuf,
    pub version: JavaVersion,
    pub distribution: Option<JavaDistribution>,
    pub source: JavaSource,
    pub rejected: Vec<RejectedJava>,
}

/// Which input supplied the version request.
#[derive(Debug, Clone)]
pub enum RequestOrigin {
    CommandLine,
    PinFile(PathBuf),
    Config(PathBuf),
    Default,
}

impl std::fmt::Display for RequestOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CommandLine => f.write_str("--java"),
            Self::PinFile(p) => write!(f, "{}", p.display()),
            Self::Config(p) => write!(f, "{}", p.display()),
            Self::Default => f.write_str("default"),
        }
    }
}

/// Stage A: decide which version is wanted.
///
/// `config` is the `[java] version` requirement together with the file it came from. A pin
/// that contradicts that requirement is a hard error naming both files — silent precedence
/// between two config files is exactly the kind of thing that costs someone an afternoon.
pub fn requested(
    cli: Option<VersionRequest>,
    config: Option<(VersionRequest, PathBuf)>,
    dir: &Path,
) -> Result<(VersionRequest, RequestOrigin)> {
    if let Some(request) = cli {
        return Ok((request, RequestOrigin::CommandLine));
    }

    if let Some(raw) = pin::read_raw(dir)? {
        let pinned: VersionRequest = raw.parse()?;
        if let Some((requirement, config_file)) = &config
            && !pin_satisfies(&pinned, requirement)
        {
            return Err(Error::PinConflict {
                // Quote what the operator actually wrote, not the reparsed request: the
                // request drops the build number, and an error that echoes something the
                // user cannot find in the file is worse than no echo at all.
                pin: raw,
                pin_file: pin::path(dir),
                requirement: requirement.to_string(),
                config_file: config_file.clone(),
            });
        }
        return Ok((pinned, RequestOrigin::PinFile(pin::path(dir))));
    }

    if let Some((requirement, file)) = config {
        return Ok((requirement, RequestOrigin::Config(file)));
    }

    Ok((
        VersionRequest::default_requirement(),
        RequestOrigin::Default,
    ))
}

/// Whether a concrete pin could satisfy a requirement.
///
/// A pin that names a range rather than a version cannot be proven to conflict, so it is
/// allowed; only a concrete pin below the requirement is rejected.
pub fn pin_satisfies(pinned: &VersionRequest, requirement: &VersionRequest) -> bool {
    match &pinned.spec {
        // A concrete pin: check the version it names against the requirement.
        VersionSpec::Prefix(components) => {
            let version = JavaVersion::new(components.clone(), None);
            let distribution = pinned
                .distribution
                .or(requirement.distribution)
                .unwrap_or_default();
            requirement.matches(distribution, &version)
        }
        // Anything else is itself a range; we cannot prove a conflict, so allow it.
        _ => true,
    }
}

pub struct Resolver {
    store: Store,
    client: AdoptiumClient,
    http: reqwest::Client,
    os: Os,
    arch: Arch,
}

impl Resolver {
    pub fn new(store: Store) -> Result<Self> {
        let os = Os::current().ok_or_else(|| {
            Error::UnsupportedPlatform(
                std::env::consts::OS.to_string(),
                std::env::consts::ARCH.to_string(),
            )
        })?;
        let arch = Arch::current().ok_or_else(|| {
            Error::UnsupportedPlatform(
                std::env::consts::OS.to_string(),
                std::env::consts::ARCH.to_string(),
            )
        })?;
        Ok(Self {
            store,
            client: AdoptiumClient::new()?,
            http: reqwest::Client::builder()
                .user_agent(concat!("hy/", env!("CARGO_PKG_VERSION")))
                .build()?,
            os,
            arch,
        })
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn os(&self) -> Os {
        self.os
    }

    pub fn arch(&self) -> Arch {
        self.arch
    }

    /// Turn a relative request into a concrete one.
    ///
    /// `latest` and `lts` name a moving target that only the registry can define, so they
    /// must be pinned to a feature version *before* the store is consulted — otherwise
    /// `latest` is trivially satisfied by whatever happens to be installed. Falls back to
    /// the local interpretation when the network is unavailable, which is the best that can
    /// be done offline.
    async fn concretise(
        &self,
        request: &VersionRequest,
        options: ResolveOptions,
    ) -> VersionRequest {
        if !matches!(request.spec, VersionSpec::Latest | VersionSpec::Lts) {
            return request.clone();
        }
        if options.offline {
            tracing::debug!("offline: interpreting `{request}` against installed runtimes");
            return request.clone();
        }
        match self.client.available_releases().await {
            Ok(available) => {
                let feature = self.feature_version(request, &available);
                tracing::debug!("`{request}` resolves to feature release {feature}");
                VersionRequest {
                    distribution: request.distribution,
                    spec: VersionSpec::Prefix(vec![feature]),
                }
            }
            Err(err) => {
                tracing::debug!("could not reach Adoptium ({err}); using installed runtimes");
                request.clone()
            }
        }
    }

    /// Stage B: satisfy a request, installing a JDK if necessary and permitted.
    pub async fn resolve(
        &self,
        request: &VersionRequest,
        options: ResolveOptions,
        progress: &dyn ProgressReporter,
    ) -> Result<ResolvedJava> {
        let request = &self.concretise(request, options).await;

        // 1. The managed store.
        let mut managed: Vec<ManagedInstall> = self
            .store
            .installs()?
            .into_iter()
            .filter(|i| request.matches(i.key.distribution, &i.key.version))
            .collect();
        managed.sort_by_key(|i| Reverse(preference(&request.spec, &i.key.version)));

        if let Some(install) = managed.into_iter().next() {
            tracing::debug!("using managed install {}", install.key);
            return Ok(ResolvedJava {
                home: install.java_home().to_path_buf(),
                executable: install.java_executable(),
                version: install.key.version.clone(),
                distribution: Some(install.key.distribution),
                source: JavaSource::Managed {
                    key: install.key,
                    fresh: false,
                },
                rejected: Vec::new(),
            });
        }

        // 2. A system installation, if it satisfies the requirement.
        let os = self.os;
        let system = tokio::task::spawn_blocking(move || discovery::discover(os))
            .await
            .map_err(std::io::Error::other)?;

        let (matching, rejected): (Vec<SystemJava>, Vec<SystemJava>) =
            system.into_iter().partition(|java| {
                request.matches(java.distribution.unwrap_or_default(), &java.version)
            });

        if let Some(java) = pick_system(&request.spec, matching) {
            tracing::debug!("using system Java at {}", java.home.display());
            return Ok(ResolvedJava {
                home: java.home,
                executable: java.executable,
                version: java.version,
                distribution: java.distribution,
                source: JavaSource::System {
                    vendor: java.vendor,
                },
                rejected: Vec::new(),
            });
        }

        let rejected: Vec<RejectedJava> = rejected
            .into_iter()
            .map(|j| RejectedJava {
                home: j.home,
                version: j.version,
            })
            .collect();

        // 3. Download.
        if let Some(reason) = self.download_blocked(options) {
            return Err(Error::DownloadsDisabled {
                request: request.clone(),
                reason,
            });
        }

        let install = self.install(request, progress).await?;
        Ok(ResolvedJava {
            home: install.java_home().to_path_buf(),
            executable: install.java_executable(),
            version: install.key.version.clone(),
            distribution: Some(install.key.distribution),
            source: JavaSource::Managed {
                key: install.key,
                fresh: true,
            },
            rejected,
        })
    }

    fn download_blocked(&self, options: ResolveOptions) -> Option<&'static str> {
        if options.offline {
            return Some("--offline");
        }
        match options.downloads {
            DownloadPolicy::Never => Some("--no-java-download"),
            DownloadPolicy::Manual if !options.explicit_install => {
                Some("java-downloads = \"manual\"")
            }
            _ => None,
        }
    }

    /// Download and install the newest JDK satisfying `request`.
    pub async fn install(
        &self,
        request: &VersionRequest,
        progress: &dyn ProgressReporter,
    ) -> Result<ManagedInstall> {
        let asset = self.find_asset(request).await?;
        let distribution = request.distribution.unwrap_or_default();
        let key = InstallKey::new(distribution, asset.version.clone(), self.os, self.arch);
        self.install_asset(&asset, &key, progress).await
    }

    /// Install an asset the caller has already resolved.
    pub async fn install_asset(
        &self,
        asset: &ReleaseAsset,
        key: &InstallKey,
        progress: &dyn ProgressReporter,
    ) -> Result<ManagedInstall> {
        self.store.install(&self.http, asset, key, progress).await
    }

    /// The newest published asset satisfying `request`.
    pub async fn find_asset(&self, request: &VersionRequest) -> Result<ReleaseAsset> {
        let distribution = request.distribution.unwrap_or_default();
        let available = self.client.available_releases().await?;
        let feature = self.feature_version(request, &available);

        let releases = self
            .client
            .feature_release(feature, self.os, self.arch, distribution.vendor())
            .await?;

        releases
            .into_iter()
            .find(|asset| request.matches(distribution, &asset.version))
            .ok_or_else(|| {
                Error::NoMatchingRelease(
                    request.clone(),
                    self.os.to_string(),
                    self.arch.to_string(),
                )
            })
    }

    /// Which feature release to fetch. This is where the LTS preference lives.
    fn feature_version(&self, request: &VersionRequest, available: &AvailableReleases) -> u32 {
        match &request.spec {
            VersionSpec::Prefix(components) => components
                .first()
                .copied()
                .unwrap_or(available.most_recent_lts),
            VersionSpec::AtLeast(components) => {
                let floor = components.first().copied().unwrap_or(0);
                // Newest LTS at or above the floor; only fall back to a non-LTS release if
                // no LTS satisfies the bound (e.g. `>=26` before the next LTS ships).
                available
                    .available_lts_releases
                    .iter()
                    .copied()
                    .filter(|v| *v >= floor)
                    .max()
                    .or_else(|| {
                        available
                            .available_releases
                            .iter()
                            .copied()
                            .filter(|v| *v >= floor)
                            .max()
                    })
                    .unwrap_or(available.most_recent_feature_release)
            }
            VersionSpec::Lts | VersionSpec::Any => available.most_recent_lts,
            VersionSpec::Latest => available.most_recent_feature_release,
        }
    }
}

/// Rank candidates: prefer LTS, then the newest version.
///
/// The LTS tie-break is suppressed for an explicit `latest`, where preferring an older LTS
/// over a newer release would invert what was asked for. It is a no-op for a `Prefix`
/// request, since every candidate then shares a feature version.
fn preference(spec: &VersionSpec, version: &JavaVersion) -> (bool, JavaVersion) {
    let prefer_lts = !matches!(spec, VersionSpec::Latest);
    (
        prefer_lts && KNOWN_LTS.contains(&version.major()),
        version.clone(),
    )
}

fn pick_system(spec: &VersionSpec, mut candidates: Vec<SystemJava>) -> Option<SystemJava> {
    candidates.sort_by_key(|j| Reverse(preference(spec, &j.version)));
    candidates.into_iter().next()
}

/// The portable pin string for a resolved JVM, for writing to `.java-version`.
pub fn pin_for(resolved: &ResolvedJava) -> String {
    pin::value(resolved.distribution.unwrap_or_default(), &resolved.version)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(s: &str) -> VersionRequest {
        s.parse().unwrap()
    }

    fn available() -> AvailableReleases {
        AvailableReleases {
            available_releases: vec![8, 11, 17, 21, 25, 26],
            available_lts_releases: vec![8, 11, 17, 21, 25],
            most_recent_lts: 25,
            most_recent_feature_release: 26,
        }
    }

    fn resolver() -> Resolver {
        Resolver::new(Store::new(PathBuf::from("/nonexistent"))).unwrap()
    }

    #[test]
    fn open_bound_prefers_lts_over_newer_ga() {
        let r = resolver();
        // Java 26 is generally available, but `>=25` must select 25.
        assert_eq!(r.feature_version(&req(">=25"), &available()), 25);
        assert_eq!(r.feature_version(&req("lts"), &available()), 25);
        // Opting in explicitly reaches 26.
        assert_eq!(r.feature_version(&req("latest"), &available()), 26);
        assert_eq!(r.feature_version(&req("26"), &available()), 26);
    }

    #[test]
    fn open_bound_above_every_lts_falls_back_to_ga() {
        let r = resolver();
        assert_eq!(r.feature_version(&req(">=26"), &available()), 26);
    }

    #[test]
    fn pin_conflict_detection() {
        assert!(pin_satisfies(&req("temurin-25.0.4.1+1"), &req(">=25")));
        assert!(!pin_satisfies(&req("temurin-21.0.4+7"), &req(">=25")));
        assert!(pin_satisfies(&req("25"), &req(">=25")));
    }

    #[test]
    fn stage_a_precedence() {
        let dir = tempfile::tempdir().unwrap();
        let config = Some((req(">=25"), PathBuf::from("hytale.toml")));

        // Nothing present: the default requirement.
        let (r, origin) = requested(None, None, dir.path()).unwrap();
        assert_eq!(r, VersionRequest::default_requirement());
        assert!(matches!(origin, RequestOrigin::Default));

        // Config beats the default.
        let (r, origin) = requested(None, config.clone(), dir.path()).unwrap();
        assert_eq!(r, req(">=25"));
        assert!(matches!(origin, RequestOrigin::Config(_)));

        // A pin beats config.
        pin::write(dir.path(), "temurin-25.0.4.1+1").unwrap();
        let (r, origin) = requested(None, config.clone(), dir.path()).unwrap();
        assert_eq!(r, req("temurin-25.0.4.1+1"));
        assert!(matches!(origin, RequestOrigin::PinFile(_)));

        // The command line beats everything.
        let (r, origin) = requested(Some(req("26")), config, dir.path()).unwrap();
        assert_eq!(r, req("26"));
        assert!(matches!(origin, RequestOrigin::CommandLine));
    }

    #[test]
    fn conflicting_pin_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        pin::write(dir.path(), "temurin-21.0.4+7").unwrap();
        let err = requested(
            None,
            Some((req(">=25"), PathBuf::from("hytale.toml"))),
            dir.path(),
        )
        .unwrap_err();
        assert!(matches!(err, Error::PinConflict { .. }));
    }
}
