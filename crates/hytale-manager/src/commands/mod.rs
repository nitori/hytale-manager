pub mod backup;
pub mod completions;
pub mod init;
pub mod install;
pub mod java;
pub mod run;
pub mod self_update;
pub mod status;
pub mod systemd;

use std::path::{Path, PathBuf};

use anyhow::Result;
use hy_instance::Instance;
use hy_java::ResolveOptions;

use crate::printer::Printer;

/// Everything a command needs that is not its own arguments.
pub struct Context {
    pub printer: Printer,
    /// Where `hy` was pointed: `--dir`, or the working directory.
    pub dir: PathBuf,
    /// `None` is not an error: `hy java install` and `hy init` both run outside instances.
    pub instance: Option<Instance>,
    pub options: ResolveOptions,
    /// Whether progress bars should be drawn.
    pub progress: bool,
}

impl Context {
    pub fn require_instance(&self) -> Result<&Instance> {
        match &self.instance {
            Some(instance) => Ok(instance),
            None => Err(hy_instance::Error::NotFound(self.dir.clone()).into()),
        }
    }

    /// The pin belongs beside `hytale.toml`: running `hy` from inside `Server/` would
    /// otherwise write a second pin that the next invocation would not see.
    pub fn pin_dir(&self) -> &Path {
        match &self.instance {
            Some(instance) => instance.root(),
            None => &self.dir,
        }
    }
}
