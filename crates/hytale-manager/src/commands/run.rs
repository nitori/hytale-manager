//! `hy run` — supervise the server.

use std::io::IsTerminal;
use std::process::ExitCode;

use anyhow::{Result, bail};
use hy_cli::RunArgs;
use hy_run::{Outcome, RunOptions, RunReporter};
use owo_colors::OwoColorize;

use crate::commands::{Context, install, java};
use crate::tui;
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

    let shell = hy_run::Shell::detect();
    let tui = !args.no_tui && (args.tui || tui::is_available());

    let mut options = RunOptions::new(resolved.executable);
    options.server_args = args.server_args;
    // The UI owns input when it is up. Otherwise forward the terminal — but not a
    // redirected stdin, which under systemd or CI is input meant for something else.
    options.forward_stdin = !tui && std::io::stdin().is_terminal();

    let outcome = if tui {
        run_with_ui(instance, options, shell, ctx).await?
    } else {
        let reporter = Reporter {
            printer: Some(ctx.printer),
            shared: None,
            shell,
        };
        hy_run::run(instance, &options, &reporter).await?
    };
    report(ctx, instance, &outcome);

    // A stop the operator asked for is a success, whatever the server exited with. Reporting
    // 130 (or Windows' 0xC000013A) would make systemd read a deliberate stop as a failure
    // and, under `Restart=on-failure`, start the server straight back up.
    if outcome.stopped_by_request {
        return Ok(ExitCode::SUCCESS);
    }
    Ok(ExitCode::from(u8::try_from(outcome.code).unwrap_or(1)))
}

/// Drive the supervisor underneath the console UI.
///
/// The UI owns the terminal, so nothing may print to it directly while this runs — the
/// reporter routes `hy`'s own messages into the output pane instead.
async fn run_with_ui(
    instance: &hy_instance::Instance,
    mut options: RunOptions,
    shell: hy_run::Shell,
    ctx: &Context,
) -> Result<Outcome> {
    let shared = tui::Shared::default();
    tui::install(&shared);

    options.output = shared.as_output();
    let stop = options.stop.clone();

    // Input goes through the same console the supervisor uses, so a typed `shutdown` and a
    // Ctrl-C-triggered one take exactly the same path.
    let console = hy_run::Console::new(false);
    options.console = Some(console.clone());

    let reporter = Reporter {
        printer: None,
        shared: Some(shared.clone()),
        shell,
    };

    let supervised = hy_run::run(instance, &options, &reporter);
    let outcome = tui::run(shared, console, stop, supervised).await?;
    let _ = ctx;
    Ok(outcome?)
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

/// Routes `hy`'s own progress messages to whichever surface is in use.
///
/// While the UI owns the terminal, printing to it directly would corrupt the frame, so the
/// messages become lines in its output pane instead.
struct Reporter {
    printer: Option<Printer>,
    shared: Option<tui::Shared>,
    shell: hy_run::Shell,
}

impl Reporter {
    fn say(&self, message: impl Into<String>) {
        let message = message.into();
        if let Some(shared) = &self.shared {
            shared.note(message);
        } else if let Some(printer) = &self.printer {
            printer.event(message);
        }
    }

    fn detail(&self, message: impl Into<String>) {
        let message = message.into();
        if let Some(shared) = &self.shared {
            shared.note(format!("  {message}"));
        } else if let Some(printer) = &self.printer {
            printer.detail(message);
        }
    }
}

impl RunReporter for Reporter {
    fn starting(&self, attempt: u32, command: &hy_run::ServerCommand) {
        if attempt == 1 {
            self.say("Starting the server".bold().to_string());
        }
        // Reprinted on every restart: a staged update can change what gets run.
        self.detail(format!("in {}", self.shell.path(&command.working_dir)));
        self.detail(command.display(self.shell));
        if attempt == 1 {
            self.detail("press Ctrl-C to stop");
        }
    }

    fn applied_update(&self) {
        self.say("Applied the staged update");
    }

    fn restarting(&self) {
        self.say("Restarting to finish the update");
    }

    fn stopping(&self) {
        // The UI announces its own stop, so saying it twice there would be noise.
        if self.shared.is_none() {
            self.say("Stopping — letting the server save first");
            self.detail("press Ctrl-C again to force");
        }
    }

    fn killing(&self) {
        if let Some(printer) = &self.printer {
            printer.warn("killing the server; saves may be lost");
        } else {
            self.say("killing the server; saves may be lost");
        }
    }
}
