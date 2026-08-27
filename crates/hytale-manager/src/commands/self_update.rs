//! `hy self update` — replace this binary with the newest release.
//!
//! Version discovery reads two plain files from the release rather than the GitHub API:
//! `latest/download/…` is a redirect GitHub maintains, so there is no rate limit, no
//! required `User-Agent`, and no JSON schema we do not control.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};
use hy_cli::SelfUpdateArgs;
use hy_fetch::{Checksum, Digester};
use semver::Version;
use tokio::io::AsyncWriteExt;

use crate::commands::Context;

const LATEST: &str = "https://github.com/nitori/hytale-manager/releases/latest/download";
const RELEASES: &str = "https://github.com/nitori/hytale-manager/releases/latest";

/// Where the three release files are read from. Overridable so a mirror can serve them, and
/// so the download-and-swap path can be exercised against a local server.
fn base_url() -> String {
    std::env::var("HY_UPDATE_BASE_URL")
        .map(|base| base.trim_end_matches('/').to_owned())
        .unwrap_or_else(|_| LATEST.to_owned())
}

/// Static musl, so it also serves a locally built `-gnu` binary updating itself.
const LINUX_X86_64: &str = "hy-x86_64-unknown-linux-musl";

pub async fn update(args: SelfUpdateArgs, ctx: &Context) -> Result<()> {
    if ctx.options.offline {
        bail!("`hy self update` needs network access, but `--offline` is set");
    }

    let asset = asset_for(env!("HY_TARGET")).ok_or_else(|| {
        anyhow!(
            "no release artifact is published for {}; download one from {RELEASES}",
            env!("HY_TARGET")
        )
    })?;

    let http = reqwest::Client::builder()
        .user_agent(concat!("hy/", env!("CARGO_PKG_VERSION")))
        .build()?;

    let base = base_url();
    let current = Version::parse(env!("CARGO_PKG_VERSION"))?;
    let published = fetch_text(&http, &format!("{base}/VERSION")).await?;
    let latest = Version::parse(published.trim())
        .with_context(|| format!("the release publishes an unparsable version: {published:?}"))?;

    if latest <= current {
        ctx.printer
            .event(format!("hy {current} is already the newest release"));
        return Ok(());
    }
    if args.check {
        ctx.printer
            .event(format!("hy {latest} is available; this is {current}"));
        return Ok(());
    }

    let exe = std::env::current_exe()?
        .canonicalize()
        .context("resolving the path of the running binary")?;
    let staged = staging_path(&exe);

    ctx.printer
        .event(format!("Updating hy {current} to {latest}"));

    let result = install(&http, &base, asset, &staged, &exe, ctx).await;
    if result.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    result?;

    ctx.printer.event(format!("Updated hy to {latest}"));
    Ok(())
}

/// Download, verify, and swap. The staged file is left for the caller to clean up.
async fn install(
    http: &reqwest::Client,
    base: &str,
    asset: &str,
    staged: &Path,
    exe: &Path,
    ctx: &Context,
) -> Result<()> {
    // Opened before the download so an install the user cannot write to fails in a second
    // rather than after 12 MB.
    let mut file = tokio::fs::File::create(staged).await.map_err(|err| {
        anyhow!(
            "cannot write to {}: {err} — if this `hy` came from a package manager, update it there",
            exe.parent().unwrap_or(exe).display()
        )
    })?;

    let sums = fetch_text(http, &format!("{base}/SHA256SUMS")).await?;
    let expected = Checksum::Sha256(expected_digest(&sums, asset)?.to_owned());

    ctx.printer.detail(format!("{base}/{asset}"));
    let actual = download(http, &format!("{base}/{asset}"), &mut file).await?;
    drop(file);

    if !expected.matches(&actual) {
        bail!(
            "checksum mismatch for {asset}: expected {}, got {actual}",
            expected.expected()
        );
    }

    make_executable(staged)?;

    // Rename rather than write in place: a running executable cannot be written to, and the
    // old inode stays alive for any `hy run` currently supervising a server.
    std::fs::rename(staged, exe).with_context(|| format!("replacing {}", exe.display()))
}

fn asset_for(target: &str) -> Option<&'static str> {
    match target {
        "x86_64-unknown-linux-musl" | "x86_64-unknown-linux-gnu" => Some(LINUX_X86_64),
        _ => None,
    }
}

/// Beside the target, because `rename` is only atomic within one filesystem.
fn staging_path(exe: &Path) -> PathBuf {
    let name = exe.file_name().unwrap_or_else(|| OsStr::new("hy"));
    exe.with_file_name(format!(
        ".{}.new-{}",
        name.to_string_lossy(),
        std::process::id()
    ))
}

fn expected_digest<'a>(sums: &'a str, asset: &str) -> Result<&'a str> {
    sums.lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            let digest = fields.next()?;
            let name = fields.next()?.trim_start_matches('*');
            (name == asset).then_some(digest)
        })
        .with_context(|| format!("the release lists no checksum for {asset}"))
}

async fn fetch_text(http: &reqwest::Client, url: &str) -> Result<String> {
    let response = http
        .get(url)
        .send()
        .await
        .with_context(|| format!("fetching {url}"))?
        .error_for_status()
        .with_context(|| format!("fetching {url}"))?;
    Ok(response.text().await?)
}

async fn download(http: &reqwest::Client, url: &str, file: &mut tokio::fs::File) -> Result<String> {
    let mut response = http
        .get(url)
        .send()
        .await
        .with_context(|| format!("fetching {url}"))?
        .error_for_status()
        .with_context(|| format!("fetching {url}"))?;

    let mut digester = Digester::sha256();
    while let Some(chunk) = response.chunk().await? {
        digester.update(&chunk);
        file.write_all(&chunk).await?;
    }
    file.sync_all().await?;
    Ok(digester.finish())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .with_context(|| format!("marking {} executable", path.display()))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gnu_and_musl_share_the_static_artifact() {
        assert_eq!(asset_for("x86_64-unknown-linux-musl"), Some(LINUX_X86_64));
        assert_eq!(asset_for("x86_64-unknown-linux-gnu"), Some(LINUX_X86_64));
    }

    #[test]
    fn unpublished_targets_have_no_asset() {
        assert_eq!(asset_for("x86_64-pc-windows-msvc"), None);
        assert_eq!(asset_for("aarch64-unknown-linux-musl"), None);
    }

    #[test]
    fn digest_is_read_from_the_matching_line() {
        let sums = "aaa  hy-aarch64-unknown-linux-musl\nbbb  hy-x86_64-unknown-linux-musl\n";
        assert_eq!(expected_digest(sums, LINUX_X86_64).unwrap(), "bbb");
    }

    #[test]
    fn a_missing_entry_is_an_error() {
        assert!(expected_digest("aaa  hy-other\n", LINUX_X86_64).is_err());
    }

    #[test]
    fn binary_mode_markers_are_tolerated() {
        let sums = format!("ccc *{LINUX_X86_64}\n");
        assert_eq!(expected_digest(&sums, LINUX_X86_64).unwrap(), "ccc");
    }

    #[test]
    fn staging_stays_in_the_target_directory() {
        let staged = staging_path(Path::new("/home/u/.local/bin/hy"));
        assert_eq!(staged.parent(), Some(Path::new("/home/u/.local/bin")));
        assert_ne!(staged.file_name(), Some(OsStr::new("hy")));
    }
}
