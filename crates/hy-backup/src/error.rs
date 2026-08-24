use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no backup with id `{0}`")]
    NotFound(String),

    #[error("`{0}` is one of the server's own backups; restoring those is not supported yet")]
    UnsupportedOrigin(String),

    #[error("the server is running; stop it first, or pass --force to snapshot anyway")]
    ServerRunning,

    #[error("{0} has no server installed to back up")]
    NothingToBackUp(PathBuf),

    #[error("backup `{id}` is missing its manifest and cannot be trusted")]
    MissingManifest { id: String },

    #[error("failed to parse {path}")]
    Parse {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },

    #[error(transparent)]
    Instance(#[from] hy_instance::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
