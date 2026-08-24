//! `hy systemd` — write a unit that runs this instance.
//!
//! The unit goes to stdout so it can be redirected straight into place; everything the
//! operator has to read goes to stderr.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use hy_cli::{Scope, SystemdArgs};
use owo_colors::OwoColorize;

use crate::commands::Context;

pub fn systemd(args: SystemdArgs, ctx: &Context) -> Result<()> {
    let instance = ctx.require_instance()?;

    if args.scope == Scope::User && (args.user.is_some() || args.group.is_some()) {
        bail!("--user and --group are for `--scope system`; a user unit already runs as you");
    }

    let exec = match args.exec {
        Some(exec) => exec,
        None => std::env::current_exe().context("could not find the running `hy` binary")?,
    };

    let account = match args.scope {
        Scope::User => None,
        Scope::System => Some(
            args.user
                .or_else(current_user)
                .context("could not determine the current user; pass --user")?,
        ),
    };
    let group = match args.scope {
        Scope::User => None,
        Scope::System => args.group.or_else(current_group),
    };

    let unit = Unit {
        description: format!("Hytale server in {}", instance.root().display()),
        working_dir: instance.root().to_path_buf(),
        exec,
        user: account.clone(),
        group,
        scope: args.scope,
    };
    let output = args.output.as_deref().map(with_service_suffix);
    let name = unit_name(args.name.as_deref(), output.as_deref(), instance.root());
    let rendered = render(&unit);

    match &output {
        Some(path) => {
            std::fs::write(path, &rendered)
                .with_context(|| format!("could not write {}", path.display()))?;
            ctx.printer
                .event(format!("Wrote {}", path.display().bold()));
        }
        None => ctx.printer.stdout(rendered.trim_end()),
    }

    advise(ctx, &name, args.scope, output.is_some(), account.as_deref());
    Ok(())
}

struct Unit {
    description: String,
    working_dir: PathBuf,
    exec: PathBuf,
    user: Option<String>,
    group: Option<String>,
    scope: Scope,
}

fn render(unit: &Unit) -> String {
    let mut lines = vec![
        "[Unit]".to_string(),
        format!("Description={}", unit.description),
        "After=network-online.target".to_string(),
        "Wants=network-online.target".to_string(),
        String::new(),
        "[Service]".to_string(),
        "Type=exec".to_string(),
    ];

    if let Some(user) = &unit.user {
        lines.push(format!("User={user}"));
    }
    if let Some(group) = &unit.group {
        lines.push(format!("Group={group}"));
    }

    lines.extend([
        format!("WorkingDirectory={}", quote(&unit.working_dir)),
        format!("ExecStart={} run --no-tui", quote(&unit.exec)),
        "Restart=on-failure".to_string(),
        "RestartSec=10s".to_string(),
        String::new(),
        // Not a comment for its own sake: the default control-group kill would send the
        // JVM its own SIGTERM, and hy's whole shutdown path assumes it owns that timing.
        "# hy asks the server to save and waits for it; only hy should be signalled.".to_string(),
        "KillMode=mixed".to_string(),
        "TimeoutStopSec=120".to_string(),
        String::new(),
        "[Install]".to_string(),
        match unit.scope {
            Scope::System => "WantedBy=multi-user.target".to_string(),
            Scope::User => "WantedBy=default.target".to_string(),
        },
    ]);

    lines.join("\n") + "\n"
}

/// systemd splits these on whitespace and understands double quotes.
fn quote(path: &Path) -> String {
    let raw = path.to_string_lossy();
    if !raw.contains([' ', '\t', '"', '\'', '\\']) {
        return raw.into_owned();
    }
    let escaped = raw.replace('\\', r"\\").replace('"', r#"\""#);
    format!("\"{escaped}\"")
}

/// systemd loads a file only if its name ends in `.service`, so `-o hytale` gets the
/// suffix rather than silently producing a unit nothing will read.
fn with_service_suffix(path: &Path) -> PathBuf {
    let Some(name) = path.file_name().map(|name| name.to_string_lossy()) else {
        return path.to_path_buf();
    };
    if name.ends_with(".service") {
        return path.to_path_buf();
    }
    let suffixed = format!("{name}.service");
    path.with_file_name(suffixed)
}

/// Unit names may hold only alphanumerics and `:-_.\`, so anything else becomes a dash.
fn unit_name(given: Option<&str>, output: Option<&Path>, root: &Path) -> String {
    let from_output = output
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned());
    let from_root = root
        .file_name()
        .map(|name| format!("hy-{}", name.to_string_lossy()));

    let raw = given
        .map(str::to_owned)
        .or(from_output)
        .or(from_root)
        .unwrap_or_else(|| "hytale".to_string());
    let stem = raw.strip_suffix(".service").unwrap_or(&raw);

    let sanitised: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || ":-_.".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("{sanitised}.service")
}

/// Where to put it, how to start it, and the one thing that will otherwise bite: the
/// service account cannot authenticate an install, because that needs a device code typed
/// into a browser by a human.
fn advise(ctx: &Context, name: &str, scope: Scope, wrote_file: bool, user: Option<&str>) {
    let (dir, systemctl) = match scope {
        Scope::System => ("/etc/systemd/system", "sudo systemctl"),
        Scope::User => ("~/.config/systemd/user", "systemctl --user"),
    };

    if wrote_file {
        ctx.printer
            .detail(format!("move it to {dir}/{name}, then:"));
    } else {
        ctx.printer
            .detail(format!("redirect this into {dir}/{name}, then:"));
    }
    ctx.printer.detail(format!("{systemctl} daemon-reload"));
    ctx.printer
        .detail(format!("{systemctl} enable --now {name}"));

    if scope == Scope::User {
        ctx.printer
            .detail("loginctl enable-linger — or it stops when you log out");
    }

    if let Some(user) = user
        && current_user().as_deref() != Some(user)
    {
        ctx.printer.warn(format!(
            "{user} has its own Java store and server credentials"
        ));
        ctx.printer.detail(format!(
            "run `sudo -u {user} hy install` first: the service cannot authenticate on \
             its own"
        ));
    }
}

#[cfg(unix)]
fn current_user() -> Option<String> {
    // Safety: getpwuid returns null or a record owned by libc, copied out before anything
    // else can call it again.
    unsafe {
        let entry = libc::getpwuid(libc::getuid());
        if entry.is_null() {
            return None;
        }
        Some(read_c((*entry).pw_name))
    }
}

#[cfg(unix)]
fn current_group() -> Option<String> {
    // Safety: as above, for the group database.
    unsafe {
        let entry = libc::getgrgid(libc::getgid());
        if entry.is_null() {
            return None;
        }
        Some(read_c((*entry).gr_name))
    }
}

#[cfg(unix)]
unsafe fn read_c(ptr: *const libc::c_char) -> String {
    unsafe { std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned() }
}

#[cfg(not(unix))]
fn current_user() -> Option<String> {
    std::env::var("USERNAME")
        .ok()
        .filter(|name| !name.is_empty())
}

/// Nothing maps onto a systemd group here; omitting `Group=` leaves it at the account's
/// primary group, which is what we would have written anyway.
#[cfg(not(unix))]
fn current_group() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(scope: Scope) -> Unit {
        Unit {
            description: "Hytale server in /srv/hytale".to_string(),
            working_dir: PathBuf::from("/srv/hytale"),
            exec: PathBuf::from("/usr/local/bin/hy"),
            user: (scope == Scope::System).then(|| "hytale".to_string()),
            group: (scope == Scope::System).then(|| "hytale".to_string()),
            scope,
        }
    }

    #[test]
    fn system_unit_names_the_account() {
        let rendered = render(&unit(Scope::System));
        assert!(rendered.contains("\nUser=hytale\n"));
        assert!(rendered.contains("\nGroup=hytale\n"));
        assert!(rendered.contains("WantedBy=multi-user.target"));
        assert!(rendered.contains("ExecStart=/usr/local/bin/hy run --no-tui"));
    }

    #[test]
    fn user_unit_has_no_account() {
        let rendered = render(&unit(Scope::User));
        assert!(!rendered.contains("User="));
        assert!(!rendered.contains("Group="));
        assert!(rendered.contains("WantedBy=default.target"));
    }

    #[test]
    fn only_hy_is_signalled() {
        let rendered = render(&unit(Scope::System));
        assert!(rendered.contains("KillMode=mixed"));
    }

    #[test]
    fn paths_with_spaces_are_quoted() {
        let mut unit = unit(Scope::System);
        unit.working_dir = PathBuf::from("/srv/my server");
        let rendered = render(&unit);
        assert!(rendered.contains(r#"WorkingDirectory="/srv/my server""#));
    }

    #[test]
    fn plain_paths_are_left_alone() {
        assert_eq!(quote(Path::new("/srv/hytale")), "/srv/hytale");
    }

    #[test]
    fn name_defaults_to_the_directory() {
        assert_eq!(
            unit_name(None, None, Path::new("/srv/main")),
            "hy-main.service"
        );
    }

    #[test]
    fn name_is_sanitised_and_suffixed_once() {
        assert_eq!(
            unit_name(Some("hy main"), None, Path::new("/srv")),
            "hy-main.service"
        );
        assert_eq!(
            unit_name(Some("hy-main.service"), None, Path::new("/srv")),
            "hy-main.service"
        );
    }

    /// Otherwise `-o hytale` would write hytale.service and then tell the operator to
    /// install hy-main.service.
    #[test]
    fn the_output_file_names_the_unit() {
        assert_eq!(
            unit_name(
                None,
                Some(Path::new("/tmp/hytale.service")),
                Path::new("/srv/main")
            ),
            "hytale.service"
        );
    }

    #[test]
    fn an_explicit_name_still_wins_over_the_output_file() {
        assert_eq!(
            unit_name(
                Some("web"),
                Some(Path::new("/tmp/hytale")),
                Path::new("/srv")
            ),
            "web.service"
        );
    }

    #[test]
    fn the_service_suffix_is_added_once() {
        assert_eq!(
            with_service_suffix(Path::new("hytale")),
            Path::new("hytale.service")
        );
        assert_eq!(
            with_service_suffix(Path::new("/etc/systemd/system/hy-main.service")),
            Path::new("/etc/systemd/system/hy-main.service")
        );
    }

    /// A dot in the middle is part of the name, not an extension to replace.
    #[test]
    fn an_unrelated_dot_is_not_mistaken_for_a_suffix() {
        assert_eq!(
            with_service_suffix(Path::new("my.server")),
            Path::new("my.server.service")
        );
    }
}
