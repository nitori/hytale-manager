//! Java runtime provisioning for `hy`.
//!
//! The Hytale server is a Java 25 application, and the operator should never have to
//! install a JDK by hand. Any command that needs a JVM runs the resolution in
//! [`resolve`], which finds a suitable installation or downloads one from Eclipse
//! Adoptium.
//!
//! ```no_run
//! # async fn example() -> Result<(), hy_java::Error> {
//! use hy_java::{DownloadPolicy, NoProgress, ResolveOptions, Resolver, Store, VersionRequest};
//!
//! let resolver = Resolver::new(Store::from_env()?)?;
//! let request = VersionRequest::default_requirement(); // >=25
//! let java = resolver
//!     .resolve(&request, ResolveOptions::default(), &NoProgress)
//!     .await?;
//! println!("{} at {}", java.version, java.home.display());
//! # Ok(())
//! # }
//! ```

pub mod adoptium;
pub mod discovery;
pub mod distribution;
pub mod download;
pub mod error;
pub mod key;
pub mod pin;
pub mod platform;
pub mod request;
pub mod resolve;
pub mod store;
pub mod version;

pub use adoptium::{AdoptiumClient, AvailableReleases, ReleaseAsset};
pub use discovery::SystemJava;
pub use distribution::JavaDistribution;
pub use download::{Checksum, NoProgress, ProgressReporter};
pub use error::{Error, Result};
pub use key::InstallKey;
pub use platform::{Arch, Os};
pub use request::{VersionRequest, VersionSpec};
pub use resolve::{
    DownloadPolicy, JavaSource, RejectedJava, RequestOrigin, ResolveOptions, ResolvedJava,
    Resolver, pin_for, pin_satisfies, requested,
};
pub use store::{ManagedInstall, Store};
pub use version::JavaVersion;
