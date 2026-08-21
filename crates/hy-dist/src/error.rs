#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unknown patchline `{0}`")]
    UnknownPatchline(String),

    #[error("could not parse the version metadata for `{patchline}`")]
    Metadata {
        patchline: String,
        #[source]
        source: quick_xml::Error,
    },

    #[error("the version metadata for `{0}` listed no versions")]
    NoVersions(String),

    #[error("version `{version}` is not published on `{patchline}`; available: {available}")]
    NotPublished {
        version: String,
        patchline: String,
        available: String,
    },

    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error(transparent)]
    Download(#[from] hy_java::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
