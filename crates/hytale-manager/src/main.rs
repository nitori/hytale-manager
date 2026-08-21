mod commands;
mod config;
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
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            anstream::eprintln!("{} {err}", "error:".red().bold());
            for cause in err.chain().skip(1) {
                anstream::eprintln!("  {} {cause}", "caused by:".dimmed());
            }
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> anyhow::Result<()> {
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

    match cli.command {
        Command::Java(namespace) => match namespace.command {
            JavaCommand::Install(args) => commands::java::install(args, &ctx).await,
            JavaCommand::List(args) => commands::java::list(args, &ctx).await,
            JavaCommand::Find(args) => commands::java::find(args, &ctx).await,
            JavaCommand::Pin(args) => commands::java::pin(args, &ctx).await,
            JavaCommand::Uninstall(args) => commands::java::uninstall(args, &ctx).await,
            JavaCommand::Dir => commands::java::dir(&ctx),
        },
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
