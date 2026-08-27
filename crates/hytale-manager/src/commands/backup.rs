//! `hy backup` — snapshot, restore, and prune.

use anyhow::{Result, bail};
use hy_backup::store::{self, Origin};
use hy_backup::{CreateOptions, History, ops::Restrict};
use hy_cli::{BackupCreateArgs, BackupPruneArgs, BackupRestoreArgs};
use hy_instance::Instance;
use owo_colors::OwoColorize;

use crate::commands::Context;
use crate::printer::{Align, Table, bytes};

pub fn create(args: BackupCreateArgs, ctx: &Context) -> Result<()> {
    let instance = ctx.require_instance()?;
    refuse_if_running(instance, args.force, ctx)?;

    let history = History::read(&store::snapshot_dir(instance.layout()))?;
    let backup = take(instance, &history, None, ctx)?;

    ctx.printer.event(format!(
        "Created {} ({})",
        backup.id.bold(),
        bytes(backup.size)
    ));
    ctx.printer.detail(backup.path.display().to_string());
    Ok(())
}

pub fn list(ctx: &Context) -> Result<()> {
    let instance = ctx.require_instance()?;
    let layout = instance.layout();
    let history = History::read(&store::snapshot_dir(layout))?;
    let backups = store::list(layout, &history)?;

    if backups.is_empty() {
        ctx.printer.event("No backups".to_string());
        return Ok(());
    }

    let mut branch_noted = false;
    let mut table = Table::new([
        Align::Left,
        Align::Left,
        Align::Left,
        Align::Right,
        Align::Left,
    ]);
    for backup in &backups {
        let local = backup.created.to_zoned(jiff::tz::TimeZone::system());
        let lineage = if backup.is_current_lineage(&history) {
            String::new()
        } else {
            branch_noted = true;
            format!("(lineage {}, superseded)", backup.lineage)
        };

        // One local-time column for both origins: our ids are UTC and the server's are
        // local, so the names alone cannot be compared by eye.
        table.row([
            backup.id.clone(),
            local.strftime("%Y-%m-%d %H:%M:%S").to_string(),
            backup.origin.as_str().to_string(),
            bytes(backup.size),
            lineage,
        ]);
    }
    table.print(ctx.printer);

    if branch_noted {
        ctx.printer.detail(
            "superseded entries predate a restore — rolling one back discards everything since",
        );
    }
    Ok(())
}

pub fn restore(args: BackupRestoreArgs, ctx: &Context) -> Result<()> {
    let instance = ctx.require_instance()?;
    let layout = instance.layout();
    refuse_if_running(instance, args.force, ctx)?;

    let mut history = History::read(&store::snapshot_dir(layout))?;
    let Some(backup) = store::find(layout, &history, &args.id)? else {
        bail!("no backup with id `{}`; see `hy backup list`", args.id);
    };

    if !backup.is_current_lineage(&history) {
        ctx.printer.warn(format!(
            "{} predates a restore; everything since will be discarded",
            backup.id
        ));
    }

    // Taken before anything is touched, and left in the listing as an ordinary backup.
    let safety = take(instance, &history, Some(&backup.id), ctx)?;
    ctx.printer
        .event(format!("Saved current state as {}", safety.id.bold()));

    let restrict = restriction(&args);
    let restored = hy_backup::restore(layout, &mut history, &backup, &restrict)?;

    if restored.is_empty() {
        ctx.printer
            .warn("nothing in that backup matched what was asked for");
        return Ok(());
    }

    ctx.printer.event(format!(
        "Restored {} from {}",
        describe(&restored),
        backup.id.bold()
    ));
    ctx.printer
        .detail(format!("now on lineage {}", history.current()));
    if matches!(restrict, Restrict::World) && backup.origin == Origin::Snapshot {
        ctx.printer
            .detail("config, bans, and whitelist were left as they are — `--all` includes them");
    }
    Ok(())
}

pub fn prune(args: BackupPruneArgs, ctx: &Context) -> Result<()> {
    let instance = ctx.require_instance()?;
    let layout = instance.layout();
    let keep = args.keep.unwrap_or(instance.config().backup.keep);

    let history = History::read(&store::snapshot_dir(layout))?;
    let removed = hy_backup::prune(layout, &history, keep)?;

    if removed.is_empty() {
        ctx.printer
            .event(format!("Nothing to prune (keeping {keep})"));
        return Ok(());
    }
    for backup in &removed {
        ctx.printer.detail(format!("removed {}", backup.id));
    }
    ctx.printer.event(format!(
        "Pruned {} snapshot(s), keeping {keep}",
        removed.len()
    ));
    Ok(())
}

fn take(
    instance: &Instance,
    history: &History,
    before_restore_of: Option<&str>,
    ctx: &Context,
) -> Result<hy_backup::Backup> {
    let options = CreateOptions {
        include: &instance.config().backup.include,
        server_version: instance.config().server.version.as_deref(),
        before_restore_of,
    };
    let backup = hy_backup::create(instance.layout(), history, &options)?;
    let _ = ctx;
    Ok(backup)
}

/// A live server rewrites the world underneath us, so a snapshot taken now may be torn.
///
/// The run lock is the whole answer: anything `hy` starts holds it, and the `start.sh` we
/// generate is a call to `hy run`. A server launched some other way — the jar by hand — is
/// invisible here, which is what `--force` documents.
fn refuse_if_running(instance: &Instance, force: bool, ctx: &Context) -> Result<()> {
    if !hy_run::RunLock::is_held(instance.root()) {
        return Ok(());
    }

    if !force {
        bail!(
            "the server is running; stop it first, or pass --force to accept a possibly \
             torn copy"
        );
    }
    ctx.printer
        .warn("the server is running; this copy may be inconsistent");
    Ok(())
}

fn restriction(args: &BackupRestoreArgs) -> Restrict {
    if args.all {
        Restrict::Everything
    } else if !args.include.is_empty() {
        Restrict::Only(args.include.clone())
    } else {
        Restrict::World
    }
}

fn describe(entries: &[std::path::PathBuf]) -> String {
    entries
        .iter()
        .map(|e| e.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}
