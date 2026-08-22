mod commands;
mod logging;
mod printer;
mod progress;

use std::io::IsTerminal;
use std::process::ExitCode;

use clap::Parser;
use hy_cli::{Cli, ColorChoice, Command, JavaCommand};
use hy_java::{DownloadPolicy, ResolveOptions};
use owo_colors::OwoColorize;

use crate::commands::Context;
use crate::printer::Printer;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    configure_color(cli.global.color);
    logging::init(cli.global.verbose, cli.global.quiet);

    match run(cli).await {
        Ok(code) => code,
        Err(err) => {
            anstream::eprintln!("{} {} {err}", printer::tag(), "error:".red().bold());
            for cause in err.chain().skip(1) {
                anstream::eprintln!("{}   {} {cause}", printer::tag(), "caused by:".dimmed());
            }
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> anyhow::Result<ExitCode> {
    let printer = Printer::new(cli.global.quiet, cli.global.verbose);

    let dir = match cli.global.dir {
        Some(dir) => dir,
        None => std::env::current_dir()?,
    };

    // `--no-java-download` is the flag; `HY_JAVA_DOWNLOADS` is the durable setting.
    let downloads = if cli.global.no_java_download {
        DownloadPolicy::Never
    } else {
        std::env::var("HY_JAVA_DOWNLOADS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or_default()
    };

    let ctx = Context {
        printer,
        instance: discover_instance(&dir)?,
        dir,
        options: ResolveOptions {
            downloads,
            offline: cli.global.offline,
            explicit_install: matches!(
                cli.command,
                Command::Java(hy_cli::JavaNamespace {
                    command: JavaCommand::Install(_) | JavaCommand::Pin(_)
                })
            ),
        },
        progress: !printer.is_quiet() && std::io::stderr().is_terminal(),
    };

    // Only `hy run` has an exit code of its own: it propagates the server's.
    if let Command::Run(args) = cli.command {
        return commands::run::run(args, &ctx).await;
    }

    match cli.command {
        Command::Init(args) => commands::init::init(args, &ctx),
        Command::Install(args) => commands::install::install(args, &ctx).await,
        Command::Status(args) => commands::status::status(args, &ctx).await,
        Command::Java(namespace) => match namespace.command {
            JavaCommand::Install(args) => commands::java::install(args, &ctx).await,
            JavaCommand::List(args) => commands::java::list(args, &ctx).await,
            JavaCommand::Find(args) => commands::java::find(args, &ctx).await,
            JavaCommand::Pin(args) => commands::java::pin(args, &ctx).await,
            JavaCommand::Uninstall(args) => commands::java::uninstall(args, &ctx).await,
            JavaCommand::Dir => commands::java::dir(&ctx),
        },
        Command::Run(_) => unreachable!("handled above"),
    }
    .map(|()| ExitCode::SUCCESS)
}

/// Only a real failure is an error; an unparsable `hytale.toml` must not be swallowed into
/// silently ignoring settings the operator wrote down.
fn discover_instance(dir: &std::path::Path) -> anyhow::Result<Option<hy_instance::Instance>> {
    match hy_instance::Instance::discover(dir) {
        Ok(instance) => Ok(Some(instance)),
        Err(hy_instance::Error::NotFound(_)) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

/// Set the global color choice.
///
/// `anstream` does the actual work: it strips ANSI codes on the way out when color is off,
/// which `owo_colors::set_override` alone would not do — that only governs
/// `if_supports_color`, while `.bold()` and friends always emit codes. Auto-detection
/// (terminal check, `NO_COLOR`, `CLICOLOR_FORCE`) is anstream's default behaviour.
fn configure_color(choice: ColorChoice) {
    let choice = match choice {
        ColorChoice::Always => anstream::ColorChoice::Always,
        ColorChoice::Never => anstream::ColorChoice::Never,
        ColorChoice::Auto => anstream::ColorChoice::Auto,
    };
    choice.write_global();
}
