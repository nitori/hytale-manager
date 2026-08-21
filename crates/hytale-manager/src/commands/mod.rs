pub mod java;

use std::path::PathBuf;

use hy_java::ResolveOptions;

use crate::printer::Printer;

/// Everything a command needs that is not its own arguments.
pub struct Context {
    pub printer: Printer,
    /// The instance directory: `--dir`, or the working directory.
    pub dir: PathBuf,
    pub options: ResolveOptions,
    /// Whether progress bars should be drawn.
    pub progress: bool,
}
