//! The OAuth device authorization grant, RFC 8628.
//!
//! Recovered from the server's `OAuthClient`: a public client — there is no secret, and the
//! token request carries only `client_id` — talking to an Ory Hydra deployment whose
//! `/.well-known/openid-configuration` advertises the device endpoint. Driving it here
//! replaces scraping the device code out of the jar's console output.

use std::time::Duration;

use jiff::Timestamp;
use serde::Deserialize;

use crate::error::{Error, Result};

pub const CLIENT_ID: &str = "hytale-server";

/// `auth:server` is what the session service checks for; `offline` is what yields a refresh
/// token, without which a server could not survive its first access-token expiry.
pub const SCOPES: &str = "openid offline auth:server";

/// Where the flow talks to. Overridable so tests need not reach the internet.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub device_auth: String,
    pub token: String,
    pub profiles: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            device_auth: "https://oauth.accounts.hytale.com/oauth2/device/auth".into(),
            token: "https://oauth.accounts.hytale.com/oauth2/token".into(),
            profiles: "https://account-data.hytale.com/my-account/get-profiles".into(),
        }
    }
}

/// What the operator has to act on, and what polling needs.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceAuth {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    /// The same page with the code already filled in; not every server sends it.
    #[serde(default)]
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    /// RFC 8628 says to assume five seconds when the server does not say.
    #[serde(default = "default_interval")]
    pub interval: u64,
}

fn default_interval() -> u64 {
    5
}

impl DeviceAuth {
    /// The link to show: the pre-filled one when offered, since it saves typing the code.
    pub fn best_link(&self) -> &str {
        self.verification_uri_complete
            .as_deref()
            .unwrap_or(&self.verification_uri)
    }
}

/// OAuth tokens as the credential store keeps them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: Timestamp,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    expires_in: i64,
}

/// The `error` field of a failed token request.
#[derive(Debug, Deserialize)]
struct TokenError {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

/// What a poll attempt means for the loop.
#[derive(Debug, PartialEq, Eq)]
enum Pending {
    /// Keep waiting at the current interval.
    Waiting,
    /// Keep waiting, but the server says we are polling too fast.
    SlowDown,
}

/// Classify an OAuth error code. `Ok` means keep polling; `Err` ends the flow.
fn classify(code: &str, description: Option<&str>) -> Result<Pending> {
    match code {
        "authorization_pending" => Ok(Pending::Waiting),
        "slow_down" => Ok(Pending::SlowDown),
        "expired_token" => Err(Error::DeviceCodeExpired),
        "access_denied" => Err(Error::AuthorizationDenied),
        other => Err(Error::OAuth {
            code: other.to_owned(),
            description: description.map(str::to_owned),
        }),
    }
}

fn expires_at(seconds: i64) -> Timestamp {
    Timestamp::now() + Duration::from_secs(seconds.max(0) as u64)
}

/// A profile the account owns. `auth.enc` stores the uuid; the name is for showing.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Profile {
    pub uuid: String,
    #[serde(default)]
    pub username: String,
}

/// Pull the profile list out of whatever shape the endpoint returns.
///
/// A bare array and an object wrapping one under any key are both accepted: the response
/// shape is the one part of this flow not recoverable from the jar, so tolerating both
/// costs nothing and avoids a hard failure on a guess.
fn profiles_from(body: &serde_json::Value) -> Vec<Profile> {
    fn parse(array: &[serde_json::Value]) -> Vec<Profile> {
        array
            .iter()
            .filter_map(|entry| serde_json::from_value(entry.clone()).ok())
            .collect()
    }

    if let Some(array) = body.as_array() {
        return parse(array);
    }
    body.as_object()
        .into_iter()
        .flatten()
        .filter_map(|(_, value)| value.as_array())
        .map(|array| parse(array))
        .find(|found: &Vec<Profile>| !found.is_empty())
        .unwrap_or_default()
}

/// Drives the device flow against one set of endpoints.
#[derive(Debug, Clone)]
pub struct DeviceFlow {
    http: reqwest::Client,
    endpoints: Endpoints,
}

impl DeviceFlow {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            endpoints: Endpoints::default(),
        }
    }

    pub fn with_endpoints(http: reqwest::Client, endpoints: Endpoints) -> Self {
        Self { http, endpoints }
    }

    /// Ask for a device code. The operator has [`DeviceAuth::expires_in`] seconds to act.
    pub async fn start(&self) -> Result<DeviceAuth> {
        let response = self
            .http
            .post(&self.endpoints.device_auth)
            .form(&[("client_id", CLIENT_ID), ("scope", SCOPES)])
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(Error::Endpoint {
                what: "device authorization",
                status: response.status().as_u16(),
                body: response.text().await.unwrap_or_default(),
            });
        }
        Ok(response.json().await?)
    }

    /// Poll until the operator authorises, refuses, or the code expires.
    ///
    /// `tick` is called before each wait so a caller can show a countdown; the flow itself
    /// prints nothing.
    pub async fn wait(
        &self,
        auth: &DeviceAuth,
        mut tick: impl FnMut(u64),
    ) -> Result<Tokens> {
        let deadline = Timestamp::now() + Duration::from_secs(auth.expires_in);
        let mut interval = Duration::from_secs(auth.interval.max(1));

        loop {
            let remaining = deadline.duration_since(Timestamp::now());
            if remaining.is_negative() {
                return Err(Error::DeviceCodeExpired);
            }
            tick(remaining.as_secs().unsigned_abs());

            tokio::time::sleep(interval).await;

            match self.redeem(&auth.device_code).await? {
                Ok(tokens) => return Ok(tokens),
                Err(Pending::Waiting) => {}
                // RFC 8628: back off by five seconds and keep the longer interval.
                Err(Pending::SlowDown) => interval += Duration::from_secs(5),
            }
        }
    }

    /// One poll: tokens, or why we should keep waiting.
    async fn redeem(&self, device_code: &str) -> Result<std::result::Result<Tokens, Pending>> {
        let response = self
            .http
            .post(&self.endpoints.token)
            .form(&[
                ("client_id", CLIENT_ID),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", device_code),
            ])
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;

        if status.is_success() {
            let token: TokenResponse = serde_json::from_str(&body)?;
            if token.refresh_token.is_empty() {
                return Err(Error::NoRefreshToken);
            }
            return Ok(Ok(Tokens {
                access_token: token.access_token,
                refresh_token: token.refresh_token,
                expires_at: expires_at(token.expires_in),
            }));
        }

        let error: TokenError = serde_json::from_str(&body).map_err(|_| Error::Endpoint {
            what: "token",
            status: status.as_u16(),
            body: body.clone(),
        })?;
        classify(&error.error, error.error_description.as_deref()).map(Err)
    }

    /// Exchange a refresh token for a fresh access token.
    pub async fn refresh(&self, refresh_token: &str) -> Result<Tokens> {
        let response = self
            .http
            .post(&self.endpoints.token)
            .form(&[
                ("client_id", CLIENT_ID),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
            ])
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(Error::Endpoint {
                what: "token refresh",
                status: status.as_u16(),
                body,
            });
        }

        let token: TokenResponse = serde_json::from_str(&body)?;
        Ok(Tokens {
            access_token: token.access_token,
            // A refresh may or may not rotate the token; keeping the old one is correct when
            // it does not, and required when it does not send one back.
            refresh_token: if token.refresh_token.is_empty() {
                refresh_token.to_owned()
            } else {
                token.refresh_token
            },
            expires_at: expires_at(token.expires_in),
        })
    }

    /// The game profiles the authenticated account owns.
    pub async fn profiles(&self, access_token: &str) -> Result<Vec<Profile>> {
        let response = self
            .http
            .get(&self.endpoints.profiles)
            .bearer_auth(access_token)
            .header("Accept", "application/json")
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(Error::Endpoint {
                what: "game profiles",
                status: response.status().as_u16(),
                body: response.text().await.unwrap_or_default(),
            });
        }
        Ok(profiles_from(&response.json().await?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_codes_keep_the_loop_going() {
        assert_eq!(
            classify("authorization_pending", None).unwrap(),
            Pending::Waiting
        );
        assert_eq!(classify("slow_down", None).unwrap(), Pending::SlowDown);
    }

    #[test]
    fn terminal_codes_end_it() {
        assert!(matches!(
            classify("expired_token", None),
            Err(Error::DeviceCodeExpired)
        ));
        assert!(matches!(
            classify("access_denied", None),
            Err(Error::AuthorizationDenied)
        ));
    }

    /// An unknown code must not be mistaken for "keep waiting", or the flow would spin
    /// until the code expired instead of reporting what the server said.
    #[test]
    fn an_unknown_code_is_reported() {
        let err = classify("invalid_client", Some("no such client")).unwrap_err();
        assert!(matches!(err, Error::OAuth { .. }));
        assert!(err.to_string().contains("invalid_client"));
    }

    #[test]
    fn the_complete_link_is_preferred() {
        let mut auth = DeviceAuth {
            device_code: "d".into(),
            user_code: "ABC".into(),
            verification_uri: "https://verify".into(),
            verification_uri_complete: Some("https://verify?user_code=ABC".into()),
            expires_in: 600,
            interval: 5,
        };
        assert_eq!(auth.best_link(), "https://verify?user_code=ABC");
        auth.verification_uri_complete = None;
        assert_eq!(auth.best_link(), "https://verify");
    }

    #[test]
    fn a_missing_interval_defaults_to_five() {
        let auth: DeviceAuth = serde_json::from_str(
            r#"{"device_code":"d","user_code":"U","verification_uri":"https://v","expires_in":600}"#,
        )
        .unwrap();
        assert_eq!(auth.interval, 5);
        assert_eq!(auth.verification_uri_complete, None);
    }

    #[test]
    fn profiles_parse_from_a_bare_array() {
        let body = serde_json::json!([{"uuid": "u-1", "username": "someone"}]);
        assert_eq!(
            profiles_from(&body),
            vec![Profile {
                uuid: "u-1".into(),
                username: "someone".into()
            }]
        );
    }

    #[test]
    fn profiles_parse_from_a_wrapped_array() {
        let body = serde_json::json!({"profiles": [{"uuid": "u-2", "username": "other"}]});
        assert_eq!(profiles_from(&body)[0].uuid, "u-2");
    }

    #[test]
    fn an_unrecognisable_body_yields_no_profiles() {
        assert!(profiles_from(&serde_json::json!({"error": "nope"})).is_empty());
    }

    #[test]
    fn expiry_is_in_the_future() {
        assert!(expires_at(3600) > Timestamp::now());
        // A server that reports a negative lifetime must not produce a time in the past
        // that looks valid; clamping to now makes it expired, which is the safe reading.
        assert!(expires_at(-1) <= Timestamp::now() + Duration::from_secs(1));
    }
}
