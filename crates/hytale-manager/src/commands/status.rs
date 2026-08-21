//! `hy status` — describe an instance without changing it.
//!
//! Java resolves with downloads disabled, so asking what an instance would use never costs
//! a 140 MB install.

use std::fmt::Display;

use anyhow::Result;
use hy_cli::StatusArgs;
use hy_instance::{Finding, Instance};
use hy_java::{DownloadPolicy, NoProgress, ResolveOptions, Resolver, Store};
use owo_colors::OwoColorize;

use crate::commands::{Context, java};
use crate::printer::Printer;

pub async fn status(args: StatusArgs, ctx: &Context) -> Result<()> {
    let instance = ctx.require_instance()?;
    let layout = instance.layout();
    let config = instance.config();

    ctx.printer
        .event(format!("Instance {}", instance.root().display().bold()));

    if !layout.is_initialised() {
        ctx.printer.detail("not initialised — run `hy init`");
    }

    let state = if hy_run::RunLock::is_held(instance.root()) {
        "running".green().to_string()
    } else {
        "stopped".to_string()
    };
    field(ctx.printer, "state", state);
    field(ctx.printer, "patchline", config.server.patchline);
    field(
        ctx.printer,
        "version",
        config.server.version.as_deref().unwrap_or("not installed"),
    );
    if let Some(bind) = &config.server.bind {
        field(ctx.printer, "bind", format!("{bind} (UDP)"));
    }

    report_layout(ctx, instance);
    report_java(args, ctx).await?;
    report_backups(ctx, instance);

    if let Some(stray) = instance.stray_jvm_options()? {
        ctx.printer.warn(format!(
            "{} does not match `[java] options` and is not used",
            layout.jvm_options().display()
        ));
        ctx.printer.detail(format!("file:   {}", stray.join(" ")));
        ctx.printer
            .detail(format!("in use: {}", config.java.options.join(" ")));
    }

    Ok(())
}

fn report_layout(ctx: &Context, instance: &Instance) {
    let layout = instance.layout();
    let findings = layout.validate();

    if findings.is_empty() {
        field(ctx.printer, "layout", "complete".green().to_string());
    } else {
        let summary = if findings.iter().any(Finding::is_fatal) {
            "incomplete".red().to_string()
        } else {
            "usable, with warnings".yellow().to_string()
        };
        field(ctx.printer, "layout", summary);
        for finding in &findings {
            ctx.printer.detail(format!("  {finding}"));
        }
    }

    if layout.has_staged_update() {
        field(
            ctx.printer,
            "update",
            "staged, applies on next start".to_string(),
        );
    }
}

async fn report_java(args: StatusArgs, ctx: &Context) -> Result<()> {
    if let Some(raw) = args.selector.java.as_deref()
        && java::looks_like_path(raw)
    {
        let resolved = java::probe_path(raw)?;
        field(
            ctx.printer,
            "java",
            format!("{} at {}", resolved.version, resolved.home.display()),
        );
        return Ok(());
    }

    let (request, origin) = java::stage_a(Some(&args.selector), ctx)?;

    let resolver = Resolver::new(Store::from_env()?)?;
    let options = ResolveOptions {
        downloads: DownloadPolicy::Never,
        ..ctx.options
    };

    match resolver.resolve(&request, options, &NoProgress).await {
        Ok(java) => field(
            ctx.printer,
            "java",
            format!(
                "{} — {}",
                java.version.to_string().bold(),
                java.home.display()
            ),
        ),
        Err(_) => field(
            ctx.printer,
            "java",
            format!("{} — installs on first run", "not present".yellow()),
        ),
    }
    ctx.printer
        .detail(format!("  requirement `{request}` from {origin}"));

    Ok(())
}

fn report_backups(ctx: &Context, instance: &Instance) {
    let hot = &instance.config().server.hot_backup;
    let value = if hot.enabled {
        format!("every {} min (by the server)", hot.frequency)
    } else {
        "disabled".to_string()
    };
    field(ctx.printer, "backups", value);
}

/// Padded before dimming: escape codes count toward a format width, so dimming first
/// would misalign every row by the length of the codes.
fn field(printer: Printer, label: &str, value: impl Display) {
    printer.event(format!("  {} {value}", format!("{label:<10}").dimmed()));
}
