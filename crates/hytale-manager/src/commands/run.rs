//! `hy run` — supervise the server.

use std::process::ExitCode;

use anyhow::{Result, bail};
use hy_cli::RunArgs;
use hy_run::{Outcome, RunOptions, RunReporter};
use owo_colors::OwoColorize;

use crate::commands::{Context, install, java};
use crate::printer::Printer;

pub async fn run(args: RunArgs, ctx: &Context) -> Result<ExitCode> {
    // Refusing here is deliberate: `hy install` scaffolds because setting up is what it was
    // asked to do, but a bare `hy run` in the wrong directory should not put a server there.
    let Some(instance) = ctx.instance.as_ref() else {
        bail!(
            "no Hytale server instance in {}; run `hy init` here first, or `hy install` to \
             set one up",
            ctx.dir.display()
        );
    };
    let installed;
    let instance = if instance.layout().is_server_install() {
        instance
    } else {
        installed = provision(instance, &args, ctx).await?;
        &installed
    };

    let resolved = java::resolve_for(&args.selector, ctx).await?;

    let options = RunOptions {
        java: resolved.executable,
        server_args: args.server_args,
    };
    let reporter = Reporter {
        printer: ctx.printer,
        shell: hy_run::Shell::detect(),
    };

    let outcome = hy_run::run(instance, &options, &reporter).await?;
    report(ctx, instance, &outcome);

    // A stop the operator asked for is a success, whatever the server exited with. Reporting
    // 130 (or Windows' 0xC000013A) would make systemd read a deliberate stop as a failure
    // and, under `Restart=on-failure`, start the server straight back up.
    if outcome.stopped_by_request {
        return Ok(ExitCode::SUCCESS);
    }
    Ok(ExitCode::from(u8::try_from(outcome.code).unwrap_or(1)))
}

/// Install a missing server before running it, the way `uv run` provisions what it needs.
///
/// Device authorisation needs a human at a terminal, so a non-interactive `hy run` — under
/// systemd, in CI — fails with instructions instead of blocking forever on a code nobody
/// will read.
async fn provision(
    instance: &hy_instance::Instance,
    args: &RunArgs,
    ctx: &Context,
) -> Result<hy_instance::Instance> {
    if args.no_install {
        bail!(
            "no server installed in {} (--no-install was given)",
            instance.root().display()
        );
    }
    if ctx.options.offline {
        bail!(
            "no server installed in {} and --offline was given",
            instance.root().display()
        );
    }
    if !install::can_authenticate_interactively() {
        bail!(
            "no server installed in {}; run `hy install` from a terminal first, because \
             authenticating needs a device code to be entered",
            instance.root().display()
        );
    }

    ctx.printer
        .event("No server installed here — installing one first".to_string());
    install::provision(instance, &args.selector, None, None, ctx).await
}

fn report(ctx: &Context, instance: &hy_instance::Instance, outcome: &Outcome) {
    if outcome.suspect_update {
        ctx.printer.warn(format!(
            "the server exited with code {} shortly after an update was applied",
            outcome.code
        ));
        ctx.printer.detail(format!(
            "the previous version is in {}",
            instance.layout().updater_backup().display()
        ));
        ctx.printer
            .detail("to roll back, remove Server/ and Assets.zip, then restore from there");
        return;
    }

    if outcome.stopped_by_request || outcome.code == 0 {
        ctx.printer.event("Server stopped".to_string());
    } else {
        ctx.printer
            .event(format!("Server exited with code {}", outcome.code));
    }
}

struct Reporter {
    printer: Printer,
    shell: hy_run::Shell,
}

impl RunReporter for Reporter {
    fn starting(&self, attempt: u32, command: &hy_run::ServerCommand) {
        if attempt == 1 {
            self.printer.event("Starting the server".bold().to_string());
        }
        // Reprinted on every restart: the AOT flag appears once the cache exists, so the
        // command genuinely differs between cycles.
        self.printer
            .detail(format!("in {}", self.shell.path(&command.working_dir)));
        self.printer.detail(command.display(self.shell));
        if attempt == 1 {
            self.printer.detail("press Ctrl-C to stop");
        }
    }

    fn applied_update(&self) {
        self.printer.event("Applied the staged update".to_string());
    }

    fn restarting(&self) {
        self.printer
            .event("Restarting to finish the update".to_string());
    }

    fn stopping(&self) {
        self.printer
            .event("Stopping — letting the server save first".to_string());
        self.printer.detail("press Ctrl-C again to force");
    }

    fn killing(&self) {
        self.printer.warn("killing the server; saves may be lost");
    }
}
