//! Server distribution: which versions exist, and fetching the jar.
//!
//! `Assets.zip` is deliberately absent here — it is not a maven artifact. Only the server's
//! own bootstrap can fetch it, which is why installing runs the jar rather than just
//! downloading files.

pub mod bootstrap;
pub mod client;
pub mod error;
pub mod maven;

pub use bootstrap::Signal;
pub use client::{DistClient, PRE_RELEASE, RELEASE, validate_patchline};
pub use error::{Error, Result};
pub use maven::Metadata;
