use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("`{0}` is empty; delete it to have a new key generated")]
    EmptyKeyFile(PathBuf),

    /// Wrong passphrase and damaged ciphertext are indistinguishable under GCM.
    #[error("could not decrypt `{0}`; it belongs to a different key or is damaged")]
    Undecryptable(PathBuf),

    #[error("stored credentials are malformed: {0}")]
    Corrupt(&'static str),

    #[error("stored credentials are missing the `{0}` field")]
    MissingField(&'static str),

    #[error("stored credentials hold an unparsable timestamp `{0}`")]
    BadTimestamp(String),

    #[error("the device code expired before the authorisation was completed")]
    DeviceCodeExpired,

    #[error("the authorisation was denied")]
    AuthorizationDenied,

    #[error("the account service rejected the request: {code}{}", .description.as_deref().map(|d| format!(" ({d})")).unwrap_or_default())]
    OAuth {
        code: String,
        description: Option<String>,
    },

    /// Without one the server could not outlive its first access-token expiry, so an
    /// authorisation that yields no refresh token is a failure rather than a partial win.
    #[error("the account service returned no refresh token")]
    NoRefreshToken,

    #[error("the {what} request failed with HTTP {status}: {body}")]
    Endpoint {
        what: &'static str,
        status: u16,
        body: String,
    },

    #[error("this account owns no game profile; create one before running a server")]
    NoProfile,

    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error("the account service sent a malformed response: {0}")]
    Malformed(#[from] serde_json::Error),
}

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
