use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no HytaleServer.jar in {0}")]
    MissingJar(PathBuf),

    #[error("no Assets.zip in {0}")]
    MissingAssets(PathBuf),

    #[error("another `hy run` is already using {0}")]
    AlreadyRunning(PathBuf),

    #[error(
        "`{flag}` in `[java] options` conflicts with `[java] aot`; set `aot = false` to \
         manage the cache yourself"
    )]
    AotConflict { flag: String },

    #[error("failed to apply the staged update")]
    Staging(#[source] std::io::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
