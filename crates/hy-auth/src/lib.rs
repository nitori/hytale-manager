//! Reading and writing a Hytale server's `auth.enc` credential store natively, so `hy`
//! can install and refresh an authenticated server without running the jar's bootstrap.
//!
//! The file format is not ours: it is fixed by the server's `EncryptedAuthCredentialStore`,
//! recovered from `HytaleServer.jar`. See [`store`] for the constants and why each is what
//! it is.

mod bson;
mod device;
mod error;
mod passphrase;
mod store;

pub use device::{CLIENT_ID, DeviceAuth, DeviceFlow, Endpoints, Profile, SCOPES, Tokens};
pub use error::{Error, Result};
pub use passphrase::hardware_uuid;
pub use store::{CredentialStore, Credentials};
