use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no Hytale server instance found in {0} or any parent directory")]
    NotFound(PathBuf),

    #[error("{0} is already a Hytale server instance")]
    AlreadyInitialised(PathBuf),

    // The two parser errors are boxed because they dominate the size of every `Result` in
    // the crate, and of anything that wraps this.
    #[error("failed to parse {path}")]
    Parse {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },

    #[error("failed to parse {path}")]
    Edit {
        path: PathBuf,
        #[source]
        source: Box<toml_edit::TomlError>,
    },

    #[error("invalid `{key}` in {path}: {message}")]
    InvalidValue {
        key: &'static str,
        path: PathBuf,
        message: String,
    },

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
