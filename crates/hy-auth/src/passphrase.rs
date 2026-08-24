//! Choosing the passphrase that `auth.enc` is encrypted under.
//!
//! Deliberately the same chain the server's `EncryptedAuthCredentialStore` uses, so a store
//! `hy` writes is readable by a server started any way at all — including `start.sh` or a
//! unit `hy` did not generate. Getting this wrong is not a visible error: the server would
//! quietly re-encrypt under its own key and `hy` could never read the file again.

use std::path::{Path, PathBuf};

use rand::Rng;

use crate::error::{Error, Result};

pub const ENV_KEY: &str = "HYTALE_AUTH_KEY";
pub const ENV_KEY_FILE: &str = "HYTALE_AUTH_KEY_FILE";

/// The passphrases in play: one to encrypt with, several that may decrypt.
///
/// `read` holds every candidate, mirroring the server, so a store written under any of them
/// still opens — which is what lets `hy` adopt an instance the jar installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub write: String,
    pub read: Vec<String>,
}

/// Where the environment is read from. Injected so tests need not mutate process state,
/// which is global and would make them order-dependent.
pub trait Env {
    fn var(&self, name: &str) -> Option<String>;
}

pub struct RealEnv;

impl Env for RealEnv {
    fn var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

/// The machine identity the server derives its default passphrase from.
///
/// Verified byte-for-byte against `HardwareUtil.getUUID()`: `/etc/machine-id` is 32 hex
/// digits, and the server parses it as a UUID, so the dashes have to go back in.
#[cfg(target_os = "linux")]
pub fn hardware_uuid() -> Option<String> {
    ["/etc/machine-id", "/var/lib/dbus/machine-id"]
        .iter()
        .find_map(|path| std::fs::read_to_string(path).ok())
        .and_then(|raw| dashed(raw.trim()))
}

/// Windows has no `/etc/machine-id`; the server reads `MachineGuid` from the registry, so
/// does this. Unlike the Linux id it is already dashed.
#[cfg(target_os = "windows")]
pub fn hardware_uuid() -> Option<String> {
    let output = std::process::Command::new("reg")
        .args([
            "query",
            r"HKLM\SOFTWARE\Microsoft\Cryptography",
            "/v",
            "MachineGuid",
        ])
        .output()
        .ok()?;
    machine_guid(&String::from_utf8_lossy(&output.stdout))
}

/// macOS would need `ioreg`; until that is implemented and checked against the jar, falling
/// through to the key file is better than guessing wrong.
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn hardware_uuid() -> Option<String> {
    None
}

/// Pull the value out of `reg query` output, which pads it as
/// `    MachineGuid    REG_SZ    <uuid>`.
///
/// Compiled on every platform so its tests run here rather than only on Windows.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn machine_guid(output: &str) -> Option<String> {
    output
        .lines()
        .find(|line| line.contains("MachineGuid"))
        .and_then(|line| line.split_whitespace().next_back())
        .map(str::to_ascii_lowercase)
        .filter(|value| {
            value.len() == 36 && value.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
        })
}

/// `00000000111122223333444444444444` → `00000000-1111-2222-3333-444444444444`.
///
/// Compiled off Linux too, so its tests run on every platform.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn dashed(hex: &str) -> Option<String> {
    if hex.len() != 32 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let hex = hex.to_ascii_lowercase();
    Some(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

/// Resolve the chain: `HYTALE_AUTH_KEY_FILE`, `HYTALE_AUTH_KEY`, the machine id, then the
/// key file beside `auth.enc` — generating that last one only if nothing else was found.
pub fn resolve(key_file: &Path, env: &dyn Env) -> Result<Resolved> {
    let mut candidates = Vec::new();

    if let Some(path) = env
        .var(ENV_KEY_FILE)
        .filter(|value| !value.trim().is_empty())
    {
        let path = PathBuf::from(path);
        let raw = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
        let trimmed = raw.trim().to_owned();
        if trimmed.is_empty() {
            return Err(Error::EmptyKeyFile(path));
        }
        candidates.push(trimmed);
    }

    if let Some(key) = env.var(ENV_KEY).filter(|value| !value.trim().is_empty()) {
        candidates.push(key.trim().to_owned());
    }

    candidates.extend(hardware_uuid());
    candidates.extend(read_key_file(key_file)?);

    if candidates.is_empty() {
        candidates.push(generate_key_file(key_file)?);
    }

    Ok(Resolved {
        write: candidates[0].clone(),
        read: candidates,
    })
}

fn read_key_file(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                Err(Error::EmptyKeyFile(path.to_path_buf()))
            } else {
                Ok(Some(trimmed.to_owned()))
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(Error::io(path, err)),
    }
}

fn generate_key_file(path: &Path) -> Result<String> {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let passphrase = bytes.iter().fold(String::new(), |mut out, byte| {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
        out
    });
    crate::store::write_private(path, passphrase.as_bytes())?;
    Ok(passphrase)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(Vec<(&'static str, String)>);

    impl Env for Fake {
        fn var(&self, name: &str) -> Option<String> {
            self.0
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.clone())
        }
    }

    fn empty() -> Fake {
        Fake(Vec::new())
    }

    #[test]
    fn machine_ids_gain_dashes_in_uuid_positions() {
        assert_eq!(
            dashed("00000000111122223333444444444444").unwrap(),
            "00000000-1111-2222-3333-444444444444"
        );
    }

    #[test]
    fn a_registry_query_yields_the_guid() {
        let output = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Cryptography\r\n    \
                      MachineGuid    REG_SZ    00000000-1111-2222-3333-444444444444\r\n";
        assert_eq!(
            machine_guid(output).unwrap(),
            "00000000-1111-2222-3333-444444444444"
        );
    }

    /// `reg` prints an error to stdout rather than failing, so a missing key must not be
    /// mistaken for a passphrase.
    #[test]
    fn registry_output_without_the_value_is_refused() {
        assert_eq!(machine_guid("ERROR: The system was unable to find"), None);
        assert_eq!(
            machine_guid("    MachineGuid    REG_SZ    not-a-guid"),
            None
        );
    }

    #[test]
    fn a_machine_id_is_lowercased() {
        assert_eq!(
            dashed("AAAAAAAABBBBCCCCDDDDEEEEEEEEEEEE").unwrap(),
            "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
        );
    }

    /// A short or non-hex file is not a machine id; treating it as one would silently
    /// produce a passphrase the server does not agree with.
    #[test]
    fn a_malformed_machine_id_is_refused() {
        assert_eq!(dashed("too-short"), None);
        assert_eq!(dashed(&"z".repeat(32)), None);
    }

    #[test]
    fn the_env_literal_outranks_the_key_file() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("auth.key");
        std::fs::write(&key, "from-file").unwrap();

        let env = Fake(vec![(ENV_KEY, "from-env".into())]);
        let resolved = resolve(&key, &env).unwrap();
        assert_eq!(resolved.write, "from-env");
        // The file is still a decryption candidate, so an older store still opens.
        assert!(resolved.read.contains(&"from-file".to_string()));
    }

    #[test]
    fn the_env_file_outranks_the_literal() {
        let dir = tempfile::tempdir().unwrap();
        let named = dir.path().join("named.key");
        std::fs::write(&named, "  from-named-file \n").unwrap();

        let env = Fake(vec![
            (ENV_KEY_FILE, named.to_string_lossy().into_owned()),
            (ENV_KEY, "from-env".into()),
        ]);
        let resolved = resolve(&dir.path().join("auth.key"), &env).unwrap();
        assert_eq!(resolved.write, "from-named-file");
    }

    #[test]
    fn an_unreadable_env_file_is_an_error_not_a_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let env = Fake(vec![(ENV_KEY_FILE, "/nonexistent/key".into())]);
        assert!(resolve(&dir.path().join("auth.key"), &env).is_err());
    }

    /// On Linux the machine id must win over the key file, because that is the order the
    /// server uses — writing under the key file instead would let the server migrate the
    /// store out from under us.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_machine_id_outranks_the_key_file() {
        let Some(hardware) = hardware_uuid() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("auth.key");
        std::fs::write(&key, "from-file").unwrap();

        let resolved = resolve(&key, &empty()).unwrap();
        assert_eq!(resolved.write, hardware);
        assert_eq!(resolved.read.last().unwrap(), "from-file");
    }

    /// Only when there is nothing else: a generated key is the least portable option.
    #[test]
    fn a_key_file_is_generated_only_as_a_last_resort() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("auth.key");
        let resolved = resolve(&key, &empty()).unwrap();

        if hardware_uuid().is_some() {
            assert!(
                !key.exists(),
                "no key file is needed when a machine id exists"
            );
        } else {
            assert!(key.is_file());
            assert_eq!(resolved.write.len(), 64);
        }
    }
}
