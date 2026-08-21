//! `hy install` — fetch a server and authenticate it.
//!
//! `Assets.zip` is 3.3 GB and is not published on maven, so it cannot simply be downloaded.
//! The only way in is the server's own bootstrap mode: fetch the jar, run it with
//! `--bootstrap`, drive `/auth login device` and `/update setup` over its stdin, and let it
//! extract the payload and shut itself down.

use std::io::IsTerminal;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use hy_cli::{InstallArgs, JavaSelector};
use hy_dist::{DistClient, Signal, bootstrap, maven};
use hy_instance::Instance;
use hy_run::Session;
use owo_colors::OwoColorize;

use crate::commands::{Context, java};
use crate::printer::Printer;
use crate::progress::BarReporter;

/// A booted server announces itself in under two seconds; this only bounds a hang.
const BOOT_TIMEOUT: Duration = Duration::from_secs(120);
/// How long to wait for the device code after asking for it.
const CODE_TIMEOUT: Duration = Duration::from_secs(60);
/// The server states a 900 s expiry for the code; allow a little past it.
const AUTH_TIMEOUT: Duration = Duration::from_secs(960);
/// Extracting a 3.3 GB payload is slow, but total silence still means something broke.
const PAYLOAD_TIMEOUT: Duration = Duration::from_secs(1800);

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

    provision(&instance, &args.selector, args.version.as_deref(), args.patchline.as_deref(), ctx)
        .await?;
    Ok(())
}

/// Fetch, bootstrap, and authenticate a server into `instance`. Returns it reloaded.
pub async fn provision(
    instance: &Instance,
    selector: &JavaSelector,
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

    // Settle what to install before provisioning a JDK, so a mistyped version does not cost
    // a 140 MB download first.
    let client = DistClient::new()?;
    let metadata = client
        .metadata(&patchline)
        .await
        .with_context(|| format!("could not list published `{patchline}` versions"))?;
    let version = maven::select(&metadata, &patchline, version)?;

    ctx.printer.event(format!(
        "Installing Hytale server {} ({patchline})",
        version.bold()
    ));

    let resolved = java::resolve_for(selector, ctx).await?;

    let reporter = BarReporter::new(ctx.progress);
    let cached = client
        .download_jar(&patchline, &version, &cache_dir()?, &reporter)
        .await
        .with_context(|| format!("could not download server {version}"))?;

    // The bootstrap jar must sit in the instance root: it extracts the payload into its
    // working directory and creates `Server/` there.
    let installer = root.join("HytaleServer.jar");
    std::fs::create_dir_all(&root)?;
    std::fs::copy(&cached, &installer)
        .with_context(|| format!("could not place the installer at {}", installer.display()))?;

    bootstrap_server(&resolved.executable, &root, layout, ctx).await?;

    // The server deletes the installer on unix but cannot on Windows while it is running.
    if installer.is_file() && !layout.jar().is_file() {
        bail!(
            "the bootstrap finished but {} was not created",
            layout.jar().display()
        );
    }
    if installer.is_file() {
        let _ = std::fs::remove_file(&installer);
    }

    let reloaded = Instance::at(&root)?;
    stamp_version(&reloaded, &version, &patchline, ctx)?;

    let findings = reloaded.layout().validate();
    for finding in &findings {
        ctx.printer.warn(finding.to_string());
    }
    if findings.is_empty() {
        ctx.printer
            .event(format!("Installed {} to {}", version.bold(), root.display()));
    }

    Instance::at(&root).map_err(Into::into)
}

/// Run the jar in bootstrap mode, driving the console.
async fn bootstrap_server(
    java: &Path,
    root: &Path,
    layout: &hy_instance::Layout,
    ctx: &Context,
) -> Result<()> {
    let args = ["-jar", "HytaleServer.jar", "--bootstrap"].map(Into::into);
    let mut session = Session::spawn(java, &args, root)?;

    // Credentials already present mean the device flow is unnecessary.
    let needs_auth = !layout.server_dir().join("auth.enc").is_file();

    // Let the server finish its startup chatter before typing at it, so the command is not
    // swallowed by a console that is not reading yet.
    settle(&mut session, ctx.printer).await;

    if needs_auth {
        ctx.printer.event("Authenticating".bold().to_string());
        session.send("/auth login device").await?;
        await_authentication(&mut session, ctx).await?;
    }

    // `/update setup` only extracts the wrapper scripts. The jar's own `--bootstrap` help
    // names `/update download` as what populates Assets.zip and the Server/ layout.
    session.send("/update download").await?;
    ctx.printer
        .event("Downloading the server payload — this takes a while".to_string());

    // The server shuts itself down once the payload is extracted.
    loop {
        match session.next_line_within(PAYLOAD_TIMEOUT).await {
            Some(Some(line)) => echo(ctx.printer, &line),
            Some(None) => break,
            None => {
                let _ = session.kill().await;
                bail!("the server stopped producing output while extracting the payload");
            }
        }
    }

    let status = session.finish().await?;
    if !status.success() {
        bail!("the bootstrap server exited with {status}");
    }
    Ok(())
}

/// Surface the device code, then wait for the operator to complete authorisation.
async fn await_authentication(session: &mut Session, ctx: &Context) -> Result<()> {
    let mut deadline = CODE_TIMEOUT;
    let mut code_seen = false;

    loop {
        let Some(next) = session.next_line_within(deadline).await else {
            let _ = session.kill().await;
            if code_seen {
                bail!("timed out waiting for the authorisation to be completed");
            }
            bail!(
                "the server did not print a device code within {}s",
                CODE_TIMEOUT.as_secs()
            );
        };
        let Some(line) = next else {
            bail!("the server exited before authentication completed");
        };

        match bootstrap::classify(&line) {
            Signal::Code(code) => {
                code_seen = true;
                deadline = AUTH_TIMEOUT;
                ctx.printer
                    .event(format!("  Enter code:  {}", code.bold().green()));
            }
            Signal::Visit(url) | Signal::DirectLink(url) => {
                ctx.printer.event(format!("  Open:        {}", url.cyan()));
            }
            Signal::Waiting { seconds } => {
                if let Some(seconds) = seconds {
                    ctx.printer
                        .detail(format!("waiting for authorisation ({seconds}s to complete)"));
                }
            }
            Signal::Authenticated => {
                ctx.printer.event("Authenticated".to_string());
                return Ok(());
            }
            Signal::AuthFailed => bail!("the server reported an authentication failure: {line}"),
            Signal::Other => echo(ctx.printer, &line),
        }
    }
}

/// Drain output until the server reports it has booted, falling back to a lull.
///
/// Typing at the console before it is reading loses the command, and boot chatter can pause
/// for longer than any safe silence threshold, so the explicit marker is what we wait for.
async fn settle(session: &mut Session, printer: Printer) {
    while let Some(Some(line)) = session.next_line_within(BOOT_TIMEOUT).await {
        echo(printer, &line);
        if bootstrap::is_booted(&line) {
            return;
        }
    }
}

fn echo(printer: Printer, line: &str) {
    let line = bootstrap::strip_ansi(line);
    if !line.trim().is_empty() && !bootstrap::is_divider(&line) {
        printer.detail(line);
    }
}

fn stamp_version(
    instance: &Instance,
    version: &str,
    patchline: &str,
    ctx: &Context,
) -> Result<()> {
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
