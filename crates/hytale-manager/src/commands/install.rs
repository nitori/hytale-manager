//! `hy install` — fetch and unpack a server, without running the jar.
//!
//! `Assets.zip` is 3.3 GB and is not on maven, but it is not out of reach either: the asset
//! service hands out a signed URL for an archive carrying the whole layout. Authenticate
//! (see [`super::auth`]), read the patchline's manifest, download, verify, unpack. No JVM is
//! involved, so installing does not provision Java — only running does.

use std::io::IsTerminal;
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use hy_cli::InstallArgs;
use hy_dist::{DistClient, maven};
use hy_instance::Instance;
use owo_colors::OwoColorize;

use crate::commands::Context;
use crate::printer::Printer;
use crate::progress::BarReporter;

pub async fn install(args: InstallArgs, ctx: &Context) -> Result<()> {
    let root = match &args.dir {
        Some(dir) => dir.clone(),
        None => ctx.dir.clone(),
    };

    // `hy install` in a bare directory should just work, so scaffold rather than refuse.
    let instance = match &ctx.instance {
        Some(instance) if args.dir.is_none() => instance.clone(),
        _ => match Instance::at(&root) {
            Ok(instance) => instance,
            Err(_) => {
                let created = hy_instance::init(&root, ">=25")?;
                ctx.printer
                    .event(format!("Initialised {}", root.display().bold()));
                created.instance
            }
        },
    };

    if instance.layout().is_server_install() && !args.force {
        ctx.printer.event(format!(
            "A server is already installed in {}",
            instance.root().display().bold()
        ));
        ctx.printer.detail("pass --force to reinstall");
        return Ok(());
    }

    provision(
        &instance,
        args.version.as_deref(),
        args.patchline.as_deref(),
        ctx,
    )
    .await?;
    Ok(())
}

/// Fetch, bootstrap, and authenticate a server into `instance`. Returns it reloaded.
pub async fn provision(
    instance: &Instance,
    version: Option<&str>,
    patchline: Option<&str>,
    ctx: &Context,
) -> Result<Instance> {
    let layout = instance.layout();
    let root = layout.root().to_path_buf();

    let patchline = patchline
        .map(str::to_string)
        .unwrap_or_else(|| instance.config().server.patchline.as_str().to_string());
    hy_dist::validate_patchline(&patchline)?;

    // Checked against maven before anything is downloaded, so a mistyped version fails at
    // once — and because the asset service will not say what else exists.
    let requested = match version {
        Some(version) => {
            let client = DistClient::new()?;
            let metadata = client
                .metadata(&patchline)
                .await
                .with_context(|| format!("could not list published `{patchline}` versions"))?;
            Some(maven::select(&metadata, &patchline, Some(version))?)
        }
        None => None,
    };

    // Credentials first: every asset request is authenticated.
    super::auth::ensure(instance, false, ctx).await?;
    let credentials = hy_auth::CredentialStore::new(&layout.server_dir())
        .read()?
        .ok_or_else(|| anyhow::anyhow!("no credentials after authenticating"))?;

    let http = reqwest::Client::builder()
        .user_agent(concat!("hy/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let assets = hy_dist::PayloadClient::new(http, credentials.access_token);

    let published = assets
        .manifest(&patchline)
        .await
        .with_context(|| format!("could not read the `{patchline}` version manifest"))?;
    // The service publishes exactly one build per patchline, so an older version cannot be
    // asked for — better to say so than to install something other than what was named.
    if let Some(requested) = &requested
        && *requested != published.version
    {
        bail!(
            "`{patchline}` currently publishes {}, not {requested}; \
             only the newest build of a patchline can be installed",
            published.version
        );
    }

    ctx.printer.event(format!(
        "Installing Hytale server {} ({patchline})",
        published.version.bold()
    ));
    ctx.printer
        .event("Downloading the server payload — this takes a while".to_string());
    let reporter = BarReporter::new(ctx.progress);
    let archive = assets
        .download(&published, &cache_dir()?, &reporter)
        .await
        .with_context(|| format!("could not download server {}", published.version))?;

    let size = std::fs::metadata(&archive).map(|meta| meta.len()).ok();
    ctx.printer.event(match size {
        Some(size) => format!("Downloaded {}", crate::printer::bytes(size)),
        None => "Downloaded the server payload".to_string(),
    });

    // Unpacking several gigabytes is slow enough that silence reads as a hang.
    ctx.printer.event("Unpacking".to_string());
    std::fs::create_dir_all(&root)?;
    hy_dist::payload::extract_into(&archive, &root)
        .with_context(|| format!("could not unpack the payload into {}", root.display()))?;
    write_launcher_scripts(&root, ctx.printer)?;

    let reloaded = Instance::at(&root)?;
    stamp_version(&reloaded, &published.version, &patchline, ctx)?;

    let findings = reloaded.layout().validate();
    for finding in &findings {
        ctx.printer.warn(finding.to_string());
    }
    if findings.is_empty() {
        ctx.printer.event(format!(
            "Installed {} to {}",
            published.version.bold(),
            root.display()
        ));
    }

    Instance::at(&root).map_err(Into::into)
}

/// Write `start.sh` and `start.bat`.
///
/// The server refuses to enable its own update checker unless a launcher sits beside
/// `Assets.zip` — so these have to exist. Since `hy run` is what supervises the exit-8
/// restart loop the shipped scripts implement, they delegate to it rather than duplicating
/// it, and an operator who runs them by hand gets the same behaviour as `hy run`.
fn write_launcher_scripts(root: &Path, printer: Printer) -> Result<()> {
    const SH: &str = "#!/bin/sh\n\
        # Generated by hy. The server looks for this file to enable its update checker.\n\
        exec hy run --dir \"$(dirname \"$0\")\" -- \"$@\"\n";
    const BAT: &str = "@echo off\r\n\
        rem Generated by hy. The server looks for this file to enable its update checker.\r\n\
        hy run --dir \"%~dp0\" -- %*\r\n";

    for (name, body) in [("start.sh", SH), ("start.bat", BAT)] {
        let path = root.join(name);
        std::fs::write(&path, body)
            .with_context(|| format!("could not write {}", path.display()))?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = root.join("start.sh");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("could not mark {} executable", path.display()))?;
    }
    printer.detail("wrote start.sh and start.bat");
    Ok(())
}

fn stamp_version(instance: &Instance, version: &str, patchline: &str, ctx: &Context) -> Result<()> {
    let mut document = instance.document()?;
    document.set_server_version(version);
    document.save()?;
    ctx.printer
        .detail(format!("recorded version {version} ({patchline})"));
    Ok(())
}

fn cache_dir() -> Result<std::path::PathBuf> {
    Ok(hy_java::Store::from_env()?.cache_dir())
}

/// Whether the device-code prompt could actually be answered.
pub fn can_authenticate_interactively() -> bool {
    std::io::stderr().is_terminal() && std::io::stdin().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The server disables its own update checker unless a launcher sits beside
    /// `Assets.zip`, so these are load-bearing rather than decoration.
    #[test]
    fn launcher_scripts_delegate_to_hy_run() {
        let dir = tempfile::tempdir().unwrap();
        write_launcher_scripts(dir.path(), Printer::new(true, 0)).unwrap();

        let sh = std::fs::read_to_string(dir.path().join("start.sh")).unwrap();
        assert!(sh.starts_with("#!/bin/sh"), "{sh}");
        assert!(sh.contains("hy run"), "{sh}");
        assert!(dir.path().join("start.bat").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn the_shell_launcher_is_executable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        write_launcher_scripts(dir.path(), Printer::new(true, 0)).unwrap();

        let mode = std::fs::metadata(dir.path().join("start.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "mode {mode:o}");
    }
}
