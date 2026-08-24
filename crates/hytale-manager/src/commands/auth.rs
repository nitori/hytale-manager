//! `hy auth` — authenticate an instance without running the server.
//!
//! The device grant is driven against the OAuth endpoints directly, so nothing here depends
//! on the jar's console wording. What lands on disk is the same `auth.enc` the jar writes.

use anyhow::{Context as _, Result, bail};
use hy_auth::{CredentialStore, Credentials, DeviceFlow};
use hy_cli::AuthArgs;
use owo_colors::OwoColorize;

use crate::commands::Context;

pub async fn auth(args: AuthArgs, ctx: &Context) -> Result<()> {
    let instance = ctx.require_instance()?;
    ensure(instance, args.force, ctx).await.map(|_| ())
}

/// Make sure the instance has credentials, authenticating if it does not.
///
/// Shared with `hy install`, which needs the same guarantee before it can fetch a payload.
/// Returns whether an authorisation actually happened.
pub async fn ensure(instance: &hy_instance::Instance, force: bool, ctx: &Context) -> Result<bool> {
    let server_dir = instance.layout().server_dir();
    // A fresh instance has no `Server/` until the payload is extracted, but the payload
    // request is itself authenticated — so the store has to exist before it.
    std::fs::create_dir_all(&server_dir)
        .with_context(|| format!("creating {}", server_dir.display()))?;
    let store = CredentialStore::new(&server_dir);

    if store.exists() && !force {
        match store.read() {
            Ok(Some(credentials)) => {
                ctx.printer.event(format!(
                    "Already authenticated as {}",
                    credentials.profile.bold()
                ));
                return Ok(false);
            }
            // Present but unreadable is worth saying out loud rather than silently
            // replacing: it usually means the server re-encrypted it under its own key.
            Err(err) => ctx.printer.warn(format!(
                "{}; authenticating again ({err})",
                store.path().display()
            )),
            Ok(None) => {}
        }
    }

    if ctx.options.offline {
        bail!("authentication needs network access, but `--offline` is set");
    }

    let credentials = authenticate(ctx).await?;
    let profile = credentials.profile.clone();
    store
        .write(&credentials)
        .with_context(|| format!("writing {}", store.path().display()))?;

    ctx.printer
        .event(format!("Authenticated as {}", profile.bold()));
    ctx.printer
        .detail(format!("credentials in {}", store.path().display()));
    Ok(true)
}

/// The device flow itself, from code to a profile.
async fn authenticate(ctx: &Context) -> Result<Credentials> {
    let http = reqwest::Client::builder()
        .user_agent(concat!("hy/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let flow = DeviceFlow::new(http);

    let device = flow
        .start()
        .await
        .context("requesting a device code from the Hytale account service")?;

    ctx.printer.event("Authenticating".bold().to_string());
    ctx.printer
        .event(format!("  Open:        {}", device.verification_uri.cyan()));
    ctx.printer.event(format!(
        "  Enter code:  {}",
        device.user_code.bold().green()
    ));
    if device.verification_uri_complete.is_some() {
        ctx.printer
            .event(format!("  Or open:     {}", device.best_link().cyan()));
    }

    // Only every fourth poll, so a ten-minute wait does not scroll the terminal.
    let mut ticks = 0u32;
    let tokens = flow
        .wait(&device, |remaining| {
            if ticks.is_multiple_of(4) {
                ctx.printer.detail(format!(
                    "waiting for authorisation ({remaining}s to complete)"
                ));
            }
            ticks += 1;
        })
        .await?;

    let profiles = flow
        .profiles(&tokens.access_token)
        .await
        .context("fetching the account's game profiles")?;
    let Some(profile) = profiles.first() else {
        return Err(hy_auth::Error::NoProfile.into());
    };
    if profiles.len() > 1 {
        ctx.printer.detail(format!(
            "account owns {} profiles; using {}",
            profiles.len(),
            profile.username
        ));
    }

    Ok(Credentials {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_at: tokens.expires_at,
        profile: profile.uuid.clone(),
    })
}
