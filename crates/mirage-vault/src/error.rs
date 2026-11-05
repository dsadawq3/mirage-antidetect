//! Vault error types.

use std::path::PathBuf;

use thiserror::Error;

/// Everything that can go wrong inside the vault.
#[derive(Debug, Error)]
pub enum VaultError {
    /// No profile with this id exists in the vault.
    #[error("profile `{id}` not found in vault")]
    NotFound { id: String },

    /// A stored file exists but its contents are not a valid `.mprof`
    /// payload: bad magic, unknown version, truncated data, undecodable
    /// JSON or an id that does not match the file name.
    #[error("vault file `{path:?}` is corrupted: {reason}")]
    Corrupted { path: PathBuf, reason: String },

    /// The id is not a canonical lowercase uuid. Rejected before any path
    /// is built, so traversal payloads never reach the filesystem.
    #[error("invalid profile id `{id}` (expected lowercase uuid: hex digits and dashes)")]
    InvalidId { id: String },

    /// AES-GCM could not open the file: the master password differs from
    /// the one used when the file was written (or the payload was tampered
    /// with — AEAD cannot distinguish the two cases).
    #[error("wrong master password (or tampered payload) for profile `{id}`")]
    WrongKey { id: String },

    /// Raw filesystem failure inside the vault directory.
    #[error("vault i/o error: {source}")]
    Io {
        #[from]
        source: std::io::Error,
    },

    /// A profile could not be serialized to JSON.
    #[error("profile serialization failed: {source}")]
    Serde {
        #[from]
        source: serde_json::Error,
    },

    /// An encryption primitive failed. Impossible with the fixed parameter
    /// set used by the vault; kept for API completeness.
    #[error("cryptographic operation failed")]
    Crypto,
}
