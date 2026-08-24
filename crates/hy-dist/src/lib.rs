//! Server distribution: which versions exist, and fetching the payload.
//!
//! Two services, deliberately: maven publishes the version list but not `Assets.zip`, and
//! the asset service hands out the whole payload but will not say what else it has. So a
//! requested version is checked against maven before the payload is fetched.

pub mod client;
pub mod error;
pub mod maven;
pub mod payload;

pub use client::{DistClient, PRE_RELEASE, RELEASE, validate_patchline};
pub use error::{Error, Result};
pub use maven::Metadata;
pub use payload::{PayloadClient, VersionManifest};
