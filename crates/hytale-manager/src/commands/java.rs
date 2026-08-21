//! `hy java` — inspect and provision Java runtimes.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use hy_cli::{
    JavaFindArgs, JavaInstallArgs, JavaListArgs, JavaPinArgs, JavaSelector, JavaUninstallArgs,
};
use hy_instance::Instance;
use hy_java::{
    InstallKey, JavaSource, RequestOrigin, ResolvedJava, Resolver, Store, VersionRequest,
    discovery, pin, requested,
};
use owo_colors::OwoColorize;

use crate::commands::Context;
use crate::printer::bytes;
use crate::progress::BarReporter;

/// `hy java install [VERSION]`
pub async fn install(args: JavaInstallArgs, ctx: &Context) -> Result<()> {
    let resolver = Resolver::new(Store::from_env()?)?;
    let request = match args.request.as_deref() {
        Some(raw) => raw.parse::<VersionRequest>()?,
        None => stage_a(None, ctx)?.0,
    };

    let asset = resolver
        .find_asset(&request)
        .await
        .with_context(|| format!("no Adoptium release satisfies `{request}`"))?;

    let key = InstallKey::new(
        request.distribution.unwrap_or_default(),
        asset.version.clone(),
        resolver.os(),
        resolver.arch(),
    );

    if !args.force && resolver.store().find(&key)?.is_some() {
        ctx.printer
            .event(format!("{} is already installed", key.to_string().bold()));
        ctx.printer.detail(format!(
            "{}",
            resolver.store().java_dir().join(key.to_string()).display()
        ));
        return Ok(());
    }

    if args.force {
        resolver.store().uninstall(&key)?;
    }

    ctx.printer.event(format!(
        "Installing {} ({})",
        key.to_string().bold(),
        bytes(asset.size)
    ));

    let reporter = BarReporter::new(ctx.progress);
    let install = resolver
        .install_asset(&asset, &key, &reporter)
        .await
        .with_context(|| format!("failed to install {key}"))?;

    ctx.printer.event(format!(
        "Installed {} to {}",
        key.to_string().bold(),
        install.java_home().display()
    ));

    maybe_write_pin(
        ctx,
        request.distribution.unwrap_or_default(),
        &asset.version,
    )?;
    Ok(())
}

/// `hy java list`
pub async fn list(args: JavaListArgs, ctx: &Context) -> Result<()> {
    let store = Store::from_env()?;

    if !args.only_system {
        let installs = store.installs()?;
        ctx.printer.event("Managed runtimes".bold().to_string());
        if installs.is_empty() {
            ctx.printer.detail("none installed");
        }
        for install in installs {
            ctx.printer.stdout(format!(
                "{:<38} {}",
                install.key.to_string(),
                install.java_home().display()
            ));
        }
    }

    if !args.only_managed {
        let os = hy_java::Os::current().context("unsupported platform")?;
        let found = tokio::task::spawn_blocking(move || discovery::discover(os)).await?;
        ctx.printer.event("System runtimes".bold().to_string());
        if found.is_empty() {
            ctx.printer.detail("none found");
        }
        for java in found {
            let vendor = java.vendor.as_deref().unwrap_or("unknown vendor");
            ctx.printer.stdout(format!(
                "{:<38} {}  ({vendor})",
                java.version.to_string(),
                java.home.display()
            ));
        }
    }

    Ok(())
}

/// `hy java find` — run the full resolution and report what would be used.
pub async fn find(args: JavaFindArgs, ctx: &Context) -> Result<()> {
    // An explicit path bypasses resolution entirely.
    if let Some(raw) = args.selector.java.as_deref()
        && looks_like_path(raw)
    {
        let resolved = probe_path(raw)?;
        if args.executable {
            ctx.printer.stdout(resolved.executable.display());
        } else {
            ctx.printer
                .event(format!("Java {} — explicit path", resolved.version.bold()));
            ctx.printer.stdout(resolved.home.display());
        }
        return Ok(());
    }

    let (request, origin) = stage_a(Some(&args.selector), ctx)?;
    let resolver = Resolver::new(Store::from_env()?)?;
    let reporter = BarReporter::new(ctx.progress);

    let resolved = resolver
        .resolve(&request, ctx.options, &reporter)
        .await
        .with_context(|| format!("could not satisfy Java requirement `{request}`"))?;

    if args.executable {
        ctx.printer.stdout(resolved.executable.display());
        return Ok(());
    }

    report(ctx, &request, &origin, &resolved);
    ctx.printer.stdout(resolved.home.display());
    Ok(())
}

/// `hy java pin VERSION`
pub async fn pin(args: JavaPinArgs, ctx: &Context) -> Result<()> {
    let request = args.request.parse::<VersionRequest>()?;

    let value = if args.no_resolve {
        args.request.clone()
    } else {
        let resolver = Resolver::new(Store::from_env()?)?;
        let reporter = BarReporter::new(ctx.progress);
        let resolved = resolver.resolve(&request, ctx.options, &reporter).await?;
        hy_java::pin_for(&resolved)
    };

    // A pin that contradicts the instance's requirement is refused here rather than at the
    // next run, so the error arrives while the operator is still thinking about it.
    if let Some((raw, file)) = ctx.instance.as_ref().and_then(Instance::java_requirement) {
        let requirement = raw
            .parse::<VersionRequest>()
            .with_context(|| format!("invalid `[java] version` in {}: `{raw}`", file.display()))?;
        let pinned: VersionRequest = value.parse()?;
        if !hy_java::pin_satisfies(&pinned, &requirement) {
            bail!(
                "the pin `{value}` does not satisfy the requirement `{requirement}` in {}",
                file.display()
            );
        }
    }

    let dir = ctx.pin_dir();
    pin::write(dir, &value)?;
    ctx.printer
        .event(format!("Pinned {} to {}", dir.display(), value.bold()));
    Ok(())
}

/// `hy java uninstall KEY`
pub async fn uninstall(args: JavaUninstallArgs, ctx: &Context) -> Result<()> {
    let key: InstallKey = args
        .key
        .parse()
        .map_err(|()| anyhow::anyhow!("`{}` is not a valid install key", args.key))?;

    let store = Store::from_env()?;
    if store.uninstall(&key)? {
        ctx.printer
            .event(format!("Removed {}", key.to_string().bold()));
    } else {
        bail!("{key} is not installed");
    }
    Ok(())
}

/// `hy java dir`
pub fn dir(ctx: &Context) -> Result<()> {
    let store = Store::from_env()?;
    ctx.printer.stdout(store.java_dir().display());
    Ok(())
}

/// Stage A of the resolution: decide which version is wanted.
pub(crate) fn stage_a(
    selector: Option<&JavaSelector>,
    ctx: &Context,
) -> Result<(VersionRequest, RequestOrigin)> {
    let cli = match selector.and_then(|s| s.java.as_deref()) {
        Some(raw) if !looks_like_path(raw) => Some(raw.parse::<VersionRequest>()?),
        _ => None,
    };

    let config = match ctx.instance.as_ref().and_then(Instance::java_requirement) {
        Some((raw, path)) => {
            let request = raw.parse::<VersionRequest>().with_context(|| {
                format!("invalid `[java] version` in {}: `{raw}`", path.display())
            })?;
            Some((request, path))
        }
        None => None,
    };

    Ok(requested(cli, config, ctx.pin_dir())?)
}

/// A selector is a path if it names one, rather than a version.
pub(crate) fn looks_like_path(raw: &str) -> bool {
    raw.contains(std::path::MAIN_SEPARATOR)
        || raw.contains('/')
        || raw.starts_with('~')
        || Path::new(raw).exists()
}

/// Probe an explicitly supplied JDK path.
pub fn probe_path(raw: &str) -> Result<ResolvedJava> {
    let path = PathBuf::from(raw);
    let executable = if path.is_dir() {
        let os = hy_java::Os::current().context("unsupported platform")?;
        path.join("bin").join(os.java_executable())
    } else {
        path.clone()
    };
    let java = discovery::probe(&executable)?;
    Ok(ResolvedJava {
        home: java.home,
        executable: java.executable,
        version: java.version,
        distribution: java.distribution,
        source: JavaSource::System {
            vendor: java.vendor,
        },
        rejected: Vec::new(),
    })
}

/// Explain what was chosen and why.
fn report(ctx: &Context, request: &VersionRequest, origin: &RequestOrigin, java: &ResolvedJava) {
    // A system JDK that exists but is too old is called out, so an automatic download is
    // never mysterious.
    for rejected in &java.rejected {
        ctx.printer.warn(format!(
            "ignoring Java {} at {}: does not satisfy `{request}`",
            rejected.version,
            rejected.home.display()
        ));
    }

    let source = match &java.source {
        JavaSource::Managed { key, fresh: true } => format!("installed {key}"),
        JavaSource::Managed { key, fresh: false } => format!("managed {key}"),
        JavaSource::System { vendor } => match vendor {
            Some(v) => format!("system ({v})"),
            None => "system".to_string(),
        },
    };

    ctx.printer.event(format!(
        "Java {} — {source}",
        java.version.to_string().bold()
    ));
    ctx.printer
        .detail(format!("requirement `{request}` from {origin}"));
}

fn maybe_write_pin(
    ctx: &Context,
    distribution: hy_java::JavaDistribution,
    version: &hy_java::JavaVersion,
) -> Result<()> {
    maybe_pin(ctx, &pin::value(distribution, version))
}

/// Record what an instance ended up using, so later runs are reproducible.
///
/// Only inside an instance: `hy java install` from an arbitrary directory should not
/// litter it with a `.java-version`.
pub(crate) fn maybe_pin(ctx: &Context, value: &str) -> Result<()> {
    if ctx.instance.is_none() {
        return Ok(());
    }
    if pin::write_if_absent(ctx.pin_dir(), value)? {
        ctx.printer
            .detail(format!("pinned {} to {value}", pin::PIN_FILE));
    }
    Ok(())
}

/// Resolve a JVM for a command that is about to use one, honouring an explicit path.
pub(crate) async fn resolve_for(selector: &JavaSelector, ctx: &Context) -> Result<ResolvedJava> {
    if let Some(raw) = selector.java.as_deref()
        && looks_like_path(raw)
    {
        return probe_path(raw);
    }

    let (request, origin) = stage_a(Some(selector), ctx)?;
    let resolver = Resolver::new(Store::from_env()?)?;
    let reporter = BarReporter::new(ctx.progress);
    let resolved = resolver
        .resolve(&request, ctx.options, &reporter)
        .await
        .with_context(|| format!("could not satisfy Java requirement `{request}`"))?;

    report(ctx, &request, &origin, &resolved);
    maybe_pin(ctx, &hy_java::pin_for(&resolved))?;
    Ok(resolved)
}
