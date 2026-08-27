use std::path::PathBuf;

use crate::request::VersionRequest;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unsupported platform: {0}/{1}")]
    UnsupportedPlatform(String, String),

    #[error("invalid Java version request `{0}`")]
    InvalidRequest(String),

    #[error(
        "no Java installation matching `{request}` was found, and automatic downloads are \
         disabled ({reason})"
    )]
    DownloadsDisabled {
        request: VersionRequest,
        reason: &'static str,
    },

    #[error("no Adoptium release matches `{0}` for {1}/{2}")]
    NoMatchingRelease(VersionRequest, String, String),

    #[error("`{0}` is not a usable Java installation")]
    NotAJavaHome(PathBuf),

    #[error("the archive for {0} did not contain a single top-level directory")]
    UnexpectedArchiveLayout(String),

    #[error(
        "the pinned Java version `{pin}` in {pin_file} does not satisfy the requirement \
         `{requirement}` in {config_file}"
    )]
    PinConflict {
        pin: String,
        pin_file: PathBuf,
        requirement: String,
        config_file: PathBuf,
    },

    #[error("failed to probe Java at {path}")]
    Probe {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(transparent)]
    Fetch(#[from] hy_fetch::Error),

    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
}

pub type Result<T> = std::result::Result<T, Error>;
