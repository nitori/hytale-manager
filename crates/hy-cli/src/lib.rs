//! Command-line surface for `hy`.
//!
//! This crate holds the clap definitions and nothing else, so `--help` rendering, shell
//! completions, and docs can be produced without building the implementation crates.
//! Version requests stay `String` here; parsing them belongs to `hy-java`.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

pub use clap_complete::Shell;

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
    /// Set up a server instance in a directory
    Init(InitArgs),

    /// Download and set up a server, authenticating it
    Install(InstallArgs),

    /// Run the server, restarting it to apply updates
    Run(RunArgs),

    /// Show the state of a server instance
    Status(StatusArgs),

    /// Snapshot, restore, and prune server state
    Backup(BackupNamespace),

    /// Manage Java runtimes
    Java(JavaNamespace),

    /// Write a systemd unit that runs this instance
    Systemd(SystemdArgs),

    /// Print a shell completion script
    Completions(CompletionsArgs),
}

#[derive(Debug, Args)]
pub struct CompletionsArgs {
    /// The shell to generate for
    #[arg(value_name = "SHELL")]
    pub shell: Shell,
}

#[derive(Debug, Args)]
pub struct SystemdArgs {
    /// Unit name; defaults to `hy-<instance directory>`
    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,

    /// Which systemd instance the unit is for
    #[arg(long, value_enum, default_value_t = Scope::System)]
    pub scope: Scope,

    /// The account to run the server as; defaults to the current user
    #[arg(long, value_name = "USER")]
    pub user: Option<String>,

    /// The group to run the server as; defaults to the user's primary group
    #[arg(long, value_name = "GROUP")]
    pub group: Option<String>,

    /// The `hy` binary to invoke; defaults to this one
    #[arg(long, value_name = "PATH")]
    pub exec: Option<PathBuf>,

    /// Write the unit here instead of to stdout; `.service` is added if missing
    #[arg(short, long, value_name = "PATH")]
    pub output: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Scope {
    /// `systemctl`, run by root as another account
    System,
    /// `systemctl --user`, run as whoever is logged in
    User,
}

/// Write the completion script for `shell` to `out`.
pub fn completions(shell: Shell, out: &mut dyn std::io::Write) {
    let mut command = <Cli as clap::CommandFactory>::command();
    clap_complete::generate(shell, &mut command, "hy", out);
}

#[derive(Debug, Args)]
pub struct BackupNamespace {
    #[command(subcommand)]
    pub command: BackupCommand,
}

#[derive(Debug, Subcommand)]
pub enum BackupCommand {
    /// Take a snapshot of the instance
    Create(BackupCreateArgs),

    /// List snapshots and the server's own backups
    List,

    /// Restore a backup, snapshotting the current state first
    Restore(BackupRestoreArgs),

    /// Delete old snapshots
    Prune(BackupPruneArgs),
}

#[derive(Debug, Args)]
pub struct BackupCreateArgs {
    /// Snapshot even though the server is running
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct BackupRestoreArgs {
    /// The id from `hy backup list`
    #[arg(value_name = "ID")]
    pub id: String,

    /// Roll back everything the backup holds, not just the world
    ///
    /// Off by default: rolling back whitelist.json would lock out anyone added since, and
    /// the same goes for bans and config.
    #[arg(long, conflicts_with = "include")]
    pub all: bool,

    /// Roll back only these entries, e.g. `universe,config.json`
    #[arg(long, value_name = "NAMES", value_delimiter = ',')]
    pub include: Vec<String>,

    /// Restore even though the server is running
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct BackupPruneArgs {
    /// How many snapshots to keep; defaults to `[backup] keep`
    #[arg(long, value_name = "N")]
    pub keep: Option<usize>,
}

#[derive(Debug, Args)]
pub struct InitArgs {
    /// The instance directory; defaults to the working directory
    #[arg(value_name = "DIR")]
    pub dir: Option<PathBuf>,

    #[command(flatten)]
    pub selector: JavaSelector,
}

#[derive(Debug, Args)]
pub struct StatusArgs {
    #[command(flatten)]
    pub selector: JavaSelector,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    #[command(flatten)]
    pub selector: JavaSelector,

    /// Use the plain scrolling output instead of the console UI
    #[arg(long, conflicts_with = "tui")]
    pub no_tui: bool,

    /// Force the console UI in a terminal where it is disabled by default
    #[arg(long)]
    pub tui: bool,

    /// Fail instead of installing a missing server
    #[arg(long)]
    pub no_install: bool,

    /// Arguments passed through to the server, after `--`
    #[arg(last = true, value_name = "SERVER_ARGS")]
    pub server_args: Vec<String>,
}

#[derive(Debug, Args)]
// `--version` here means the server's, not `hy`'s; `hy --version` still reports the tool.
#[command(disable_version_flag = true)]
pub struct InstallArgs {
    /// The instance directory; defaults to the working directory
    #[arg(value_name = "DIR")]
    pub dir: Option<PathBuf>,

    /// The server version; defaults to the newest on the patchline
    #[arg(long, value_name = "VERSION")]
    pub version: Option<String>,

    /// Which channel to install from
    #[arg(long, value_name = "release|pre-release")]
    pub patchline: Option<String>,

    /// Reinstall even if a server is already present
    #[arg(long)]
    pub force: bool,

    #[command(flatten)]
    pub selector: JavaSelector,
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

/// The `-j` / `--java` selector, shared by every command that needs a JVM.
#[derive(Debug, Args, Clone)]
pub struct JavaSelector {
    /// The Java version to use, e.g. `25`, `25.0.4`, `>=25`, `lts`, `latest`, or a path
    ///
    /// If no matching runtime is installed, one is downloaded automatically.
    #[arg(
        short = 'j',
        long = "java",
        value_name = "VERSION|PATH",
        env = "HY_JAVA"
    )]
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
