//! Mirage vault: encrypted local storage for browser profiles.
//!
//! Every profile is stored as `<dir>/<id>.mprof` with the layout
//! `MAGIC "MGPF" || u32 LE version || nonce[12] || AES-256-GCM(json Profile)`.
//! The master password is never persisted; the AES key is derived with
//! Argon2id over a random 16-byte salt kept in `<dir>/.vault.salt`.

pub mod crypto;
mod error;
mod vault;

pub use crypto::{derive_key, MAGIC, VERSION};
pub use error::VaultError;
pub use vault::Vault;
