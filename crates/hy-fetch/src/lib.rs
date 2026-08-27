//! Fetching files for `hy`, and the locations they are kept in.
//!
//! Both the JDK archives (`hy-java`) and the server payload (`hy-dist`) are large downloads
//! that must survive an interrupted transfer and be checked against a published digest.
//! That machinery is here rather than in either of them, so neither has to depend on the
//! other for it.
//!
//! [`paths`] lives here for the same reason: the download cache is what the two share, and
//! resolving `HY_HOME` in one place keeps them from disagreeing about where it is.

pub mod digest;
pub mod download;
pub mod error;
pub mod paths;

pub use digest::{Checksum, Digester, digest_file};
pub use download::{NoProgress, ProgressReporter, download_verified};
pub use error::{Error, Result};
pub use paths::{cache_dir, home};
