//! Reading and writing the server's `auth.enc`.
//!
//! Every constant here is dictated by the server's `EncryptedAuthCredentialStore`, which
//! reads the file we write; none of them are ours to choose.
//!
//! The passphrase comes from [`crate::passphrase`], which follows the same chain the server
//! does. That is what makes the store readable by a server started any way at all — and it
//! is not optional: encrypting under a key the server ranks lower makes it re-encrypt the
//! file under its own, after which `hy` cannot read it. Observed, before the chain matched.

use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use jiff::Timestamp;
use rand::Rng;

use crate::bson;
use crate::error::{Error, Result};
use crate::passphrase::{self, RealEnv};

const SALT: &[u8] = b"HytaleAuthCredentialStore";
const ITERATIONS: u32 = 100_000;
const NONCE_LEN: usize = 12;

/// What the server keeps between runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: Timestamp,
    pub profile: String,
}

/// The `auth.enc` / `auth.key` pair inside a `Server/` directory.
#[derive(Debug, Clone)]
pub struct CredentialStore {
    encrypted: PathBuf,
    key: PathBuf,
}

impl CredentialStore {
    /// `server_dir` is the instance's `Server/`, the working directory the jar runs in.
    pub fn new(server_dir: &Path) -> Self {
        let encrypted = server_dir.join("auth.enc");
        Self {
            key: key_path(&encrypted),
            encrypted,
        }
    }

    pub fn path(&self) -> &Path {
        &self.encrypted
    }

    pub fn exists(&self) -> bool {
        self.encrypted.is_file()
    }

    /// Encrypt `credentials` under the instance's passphrase, generating one if absent.
    pub fn write(&self, credentials: &Credentials) -> Result<()> {
        let resolved = passphrase::resolve(&self.key, &RealEnv)?;
        let passphrase = resolved.write;
        let plaintext = bson::encode(&[
            ("AccessToken", credentials.access_token.as_str()),
            ("RefreshToken", credentials.refresh_token.as_str()),
            ("ExpiresAt", &credentials.expires_at.to_string()),
            ("ProfileUuid", credentials.profile.as_str()),
        ]);

        let mut nonce = [0u8; NONCE_LEN];
        rand::rng().fill_bytes(&mut nonce);

        let cipher = Aes256Gcm::new(&derive_key(&passphrase));
        let sealed = cipher
            .encrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: &plaintext,
                    aad: &[],
                },
            )
            .expect("AES-GCM only fails on inputs far larger than a token");

        let mut file = nonce.to_vec();
        file.extend_from_slice(&sealed);
        write_private(&self.encrypted, &file)
    }

    /// `None` when no credentials have been stored yet.
    pub fn read(&self) -> Result<Option<Credentials>> {
        if !self.encrypted.is_file() {
            return Ok(None);
        }
        let file = std::fs::read(&self.encrypted).map_err(|e| Error::io(&self.encrypted, e))?;
        if file.len() <= NONCE_LEN {
            return Err(Error::Undecryptable(self.encrypted.clone()));
        }
        let (nonce, sealed) = file.split_at(NONCE_LEN);
        let nonce = Nonce::try_from(nonce).expect("split at exactly NONCE_LEN");

        // Every candidate is tried, as the server does, so a store written under a
        // different link in the chain still opens instead of looking corrupt.
        let plaintext = passphrase::resolve(&self.key, &RealEnv)?
            .read
            .iter()
            .find_map(|candidate| {
                Aes256Gcm::new(&derive_key(candidate))
                    .decrypt(
                        &nonce,
                        Payload {
                            msg: sealed,
                            aad: &[],
                        },
                    )
                    .ok()
            })
            .ok_or_else(|| Error::Undecryptable(self.encrypted.clone()))?;

        let mut fields = bson::decode(&plaintext)?;
        let mut take = |name: &'static str| fields.remove(name).ok_or(Error::MissingField(name));
        let access_token = take("AccessToken")?;
        let refresh_token = take("RefreshToken")?;
        let expires_at = take("ExpiresAt")?;
        let profile = take("ProfileUuid")?;

        Ok(Some(Credentials {
            access_token,
            refresh_token,
            expires_at: expires_at
                .parse()
                .map_err(|_| Error::BadTimestamp(expires_at))?,
            profile,
        }))
    }

    /// The last-resort passphrase file, written only when the machine has no usable id.
    pub fn key_file(&self) -> &Path {
        &self.key
    }
}

/// The server derives this path by replacing the extension, so `auth.enc` pairs with
/// `auth.key`.
fn key_path(encrypted: &Path) -> PathBuf {
    let name = encrypted
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = match name.rfind('.') {
        Some(dot) => &name[..dot],
        None => &name,
    };
    encrypted.with_file_name(format!("{stem}.key"))
}

fn derive_key(passphrase: &str) -> Key<Aes256Gcm> {
    let mut bytes = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(passphrase.as_bytes(), SALT, ITERATIONS, &mut bytes);
    Key::<Aes256Gcm>::from(bytes)
}

/// Both files are secrets, so neither is group- or world-readable.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes).map_err(|e| Error::io(path, e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| Error::io(path, e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Credentials {
        Credentials {
            access_token: "access".into(),
            refresh_token: "refresh".into(),
            expires_at: "2026-08-23T18:04:05Z".parse().unwrap(),
            profile: "6ba7b810-9dad-11d1-80b4-00c04fd430c8".into(),
        }
    }

    #[test]
    fn round_trips_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path());
        store.write(&sample()).unwrap();
        assert_eq!(store.read().unwrap(), Some(sample()));
    }

    #[test]
    fn no_file_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(CredentialStore::new(dir.path()).read().unwrap(), None);
    }

    /// Two writes must stay mutually readable: the passphrase is resolved per call, so a
    /// chain that returned something different each time would strand the first store.
    #[test]
    fn writing_twice_stays_readable() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path());
        store.write(&sample()).unwrap();
        store.write(&sample()).unwrap();
        assert_eq!(store.read().unwrap(), Some(sample()));
    }

    #[test]
    fn the_key_file_sits_beside_the_encrypted_one() {
        let store = CredentialStore::new(Path::new("/srv/Server"));
        assert_eq!(store.key_file(), Path::new("/srv/Server/auth.key"));
    }

    /// A store written under a passphrase outside the chain is reported as undecryptable
    /// rather than silently mistaken for corruption.
    #[test]
    fn a_passphrase_outside_the_chain_cannot_read_it() {
        use aes_gcm::aead::{Aead, KeyInit, Payload};

        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path());
        let sealed = Aes256Gcm::new(&derive_key("not-in-the-chain"))
            .encrypt(
                &Nonce::from([0u8; NONCE_LEN]),
                Payload {
                    msg: b"whatever",
                    aad: &[],
                },
            )
            .unwrap();
        let mut file = vec![0u8; NONCE_LEN];
        file.extend_from_slice(&sealed);
        std::fs::write(store.path(), &file).unwrap();

        assert!(matches!(store.read(), Err(Error::Undecryptable(_))));
    }

    #[test]
    fn a_tampered_ciphertext_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path());
        store.write(&sample()).unwrap();

        let mut bytes = std::fs::read(store.path()).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        std::fs::write(store.path(), &bytes).unwrap();

        assert!(matches!(store.read(), Err(Error::Undecryptable(_))));
    }

    #[test]
    fn the_nonce_differs_between_writes() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path());
        store.write(&sample()).unwrap();
        let first = std::fs::read(store.path()).unwrap();
        store.write(&sample()).unwrap();
        let second = std::fs::read(store.path()).unwrap();
        assert_ne!(first[..NONCE_LEN], second[..NONCE_LEN]);
    }

    #[test]
    fn an_empty_key_file_is_named_in_the_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = CredentialStore::new(dir.path());
        store.write(&sample()).unwrap();
        std::fs::write(store.key_file(), "   \n").unwrap();
        assert!(matches!(store.read(), Err(Error::EmptyKeyFile(_))));
    }
}
