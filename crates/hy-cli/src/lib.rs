//! Command-line surface for `hy`.
//!
//! This crate holds the clap definitions and nothing else, so `--help` rendering, shell
//! completions, and docs can be produced without building the implementation crates.
//! Version requests stay `String` here; parsing them belongs to `hy-java`.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "hy",
    version,
    about = "Manage Hytale server installations, backups, and processes",
    disable_help_subcommand = true,
    propagate_version = true
)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Args)]
pub struct GlobalArgs {
    /// Suppress all output except errors
    #[arg(short, long, global = true, conflicts_with = "verbose")]
    pub quiet: bool,

    /// Use verbose output; repeat for more detail
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Control colored output
    #[arg(long, global = true, value_enum, default_value_t = ColorChoice::Auto)]
    pub color: ColorChoice,

    /// Disable all network access
    #[arg(long, global = true, env = "HY_OFFLINE")]
    pub offline: bool,

    /// The server instance directory
    #[arg(long, global = true, value_name = "PATH", env = "HY_DIR")]
    pub dir: Option<PathBuf>,

    /// Never download a Java runtime automatically
    #[arg(long, global = true)]
    pub no_java_download: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ColorChoice {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Manage Java runtimes
    Java(JavaNamespace),
}

#[derive(Debug, Args)]
pub struct JavaNamespace {
    #[command(subcommand)]
    pub command: JavaCommand,
}

#[derive(Debug, Subcommand)]
pub enum JavaCommand {
    /// Install a Java runtime
    Install(JavaInstallArgs),

    /// List available Java runtimes
    List(JavaListArgs),

    /// Show which Java runtime would be used
    Find(JavaFindArgs),

    /// Pin this instance to a Java version
    Pin(JavaPinArgs),

    /// Remove a managed Java runtime
    Uninstall(JavaUninstallArgs),

    /// Print the managed runtime directory
    Dir,
}

/// The `-p` / `--java` selector, shared by every command that needs a JVM.
#[derive(Debug, Args, Clone)]
pub struct JavaSelector {
    /// The Java version to use, e.g. `25`, `25.0.4`, `>=25`, `lts`, `latest`, or a path
    ///
    /// If no matching runtime is installed, one is downloaded automatically.
    #[arg(short = 'p', long = "java", value_name = "VERSION|PATH", env = "HY_JAVA")]
    pub java: Option<String>,
}

#[derive(Debug, Args)]
pub struct JavaInstallArgs {
    /// The version to install; defaults to the newest LTS meeting the requirement
    #[arg(value_name = "VERSION")]
    pub request: Option<String>,

    /// Reinstall even if the version is already present
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct JavaListArgs {
    /// Only list runtimes managed by hy
    #[arg(long, conflicts_with = "only_system")]
    pub only_managed: bool,

    /// Only list runtimes found on the system
    #[arg(long)]
    pub only_system: bool,
}

#[derive(Debug, Args)]
pub struct JavaFindArgs {
    #[command(flatten)]
    pub selector: JavaSelector,

    /// Print only the path to the java executable
    #[arg(long)]
    pub executable: bool,
}

#[derive(Debug, Args)]
pub struct JavaPinArgs {
    /// The version to write to .java-version
    #[arg(value_name = "VERSION")]
    pub request: String,

    /// Write the pin without checking that a matching runtime is available
    #[arg(long)]
    pub no_resolve: bool,
}

#[derive(Debug, Args)]
pub struct JavaUninstallArgs {
    /// The install key, as shown by `hy java list`
    #[arg(value_name = "KEY")]
    pub key: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_is_well_formed() {
        Cli::command().debug_assert();
    }
}
