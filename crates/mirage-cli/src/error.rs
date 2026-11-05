//! CLI error type and process exit codes.
//!
//! Every fallible operation of the CLI returns [`CliError`]. The variant
//! decides the process exit code (see [`exit`]) and, for `--json` runs, the
//! message printed to stderr (stdout carries JSON data only).

use mirage_cookies::CookieError;
use mirage_core::GenerateError;
use mirage_import::ImportError;
use mirage_proxy::ProxyError;
use mirage_vault::VaultError;
use std::path::PathBuf;
use thiserror::Error;

/// Process exit codes used by the `mirage` binary. Documented in `README.md`.
pub mod exit {
    /// Success.
    pub const OK: u8 = 0;
    /// Unclassified runtime failure.
    pub const GENERAL: u8 = 1;
    /// Command-line usage error (clap's own default).
    pub const USAGE: u8 = 2;
    /// Vault password missing or wrong, or the vault could not be opened.
    pub const VAULT: u8 = 3;
    /// The requested profile or session does not exist.
    pub const NOT_FOUND: u8 = 4;
    /// Conflicting state (a profile is already running, or similar).
    pub const CONFLICT: u8 = 5;
    /// The proxy check failed (bad URL, unreachable proxy, no exit IP).
    pub const PROXY: u8 = 6;
}

/// Everything that can go wrong in the CLI.
#[derive(Debug, Error)]
pub enum CliError {
    /// A vault-backed command was invoked without a password.
    #[error("vault password required: pass --password or set MIRAGE_VAULT_PASSWORD")]
    PasswordMissing,

    /// No profile with the given id exists.
    #[error("profile `{0}` not found")]
    ProfileNotFound(String),

    /// No running session with the given id is registered.
    #[error("session `{0}` not found")]
    SessionNotFound(String),

    /// A running-session descriptor exists but could not be parsed.
    #[error("session descriptor `{path}` is corrupt: {reason}")]
    SessionDescriptor {
        /// Path of the offending descriptor.
        path: PathBuf,
        /// Why it could not be parsed.
        reason: String,
    },

    /// The `--proxy` value is not a valid `scheme://[user:pass@]host:port` URL.
    #[error("invalid proxy url `{url}`: {reason}")]
    ProxyUrl {
        /// The offending value, verbatim.
        url: String,
        /// Why it could not be parsed.
        reason: String,
    },

    /// The profile is already running in another launcher process.
    #[error("profile `{0}` is already running")]
    AlreadyRunning(String),

    /// Free-form runtime error with a ready message.
    #[error("{0}")]
    Message(String),

    /// The command exists but its native backend is not available on this OS.
    #[error("operation not supported on this platform: {0}")]
    Unsupported(&'static str),

    /// A launcher failure, already formatted with its context chain.
    #[error("{0}")]
    Launch(String),

    /// Vault failure (I/O, corrupted file, wrong key).
    #[error(transparent)]
    Vault(#[from] VaultError),

    /// Raw filesystem failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// JSON (de)serialization failure.
    #[error(transparent)]
    Json(#[from] serde_json::Error),

    /// Fingerprint generation failure (embedded fp-db is broken).
    #[error(transparent)]
    Generate(#[from] GenerateError),

    /// Competitor-profile import failure.
    #[error(transparent)]
    Import(#[from] ImportError),

    /// Cookie import/export failure.
    #[error(transparent)]
    Cookie(#[from] CookieError),

    /// Proxy dialing / handshake failure.
    #[error(transparent)]
    Proxy(#[from] ProxyError),
}

impl CliError {
    /// The process exit code this error maps to.
    pub fn exit_code(&self) -> u8 {
        match self {
            CliError::PasswordMissing => exit::VAULT,
            CliError::Vault(VaultError::WrongKey { .. } | VaultError::Corrupted { .. }) => {
                exit::VAULT
            }
            CliError::ProfileNotFound(_) | CliError::SessionNotFound(_) => exit::NOT_FOUND,
            CliError::AlreadyRunning(_) => exit::CONFLICT,
            CliError::Proxy(_) | CliError::ProxyUrl { .. } => exit::PROXY,
            _ => exit::GENERAL,
        }
    }

    /// Translate a vault "not found" (or an id that cannot name any profile)
    /// into the CLI's own not-found variant so callers get a clean message and
    /// the `NOT_FOUND` exit code.
    ///
    /// A non-uuid id is rejected by the vault before any path is built; it can
    /// never name an existing profile, so it is reported as not found (exit 4)
    /// rather than as a vault failure (exit 3).
    pub fn vault_lookup(error: VaultError, id: &str) -> Self {
        match error {
            VaultError::NotFound { .. } | VaultError::InvalidId { .. } => {
                CliError::ProfileNotFound(id.to_string())
            }
            other => CliError::Vault(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_found_variants_map_to_exit_4() {
        assert_eq!(
            CliError::ProfileNotFound("x".into()).exit_code(),
            exit::NOT_FOUND
        );
        assert_eq!(
            CliError::SessionNotFound("x".into()).exit_code(),
            exit::NOT_FOUND
        );
    }

    #[test]
    fn conflicting_state_maps_to_exit_5() {
        assert_eq!(
            CliError::AlreadyRunning("x".into()).exit_code(),
            exit::CONFLICT
        );
    }

    #[test]
    fn corrupt_descriptor_maps_to_general() {
        let error = CliError::SessionDescriptor {
            path: PathBuf::from("run/x.json"),
            reason: "bad json".into(),
        };
        assert_eq!(error.exit_code(), exit::GENERAL);
    }

    #[test]
    fn vault_lookup_maps_notfound_and_invalid_id_to_not_found() {
        let missing = CliError::vault_lookup(VaultError::NotFound { id: "x".into() }, "x");
        assert!(matches!(missing, CliError::ProfileNotFound(_)));
        assert_eq!(missing.exit_code(), exit::NOT_FOUND);

        // A non-uuid id can never name a profile: exit 4, not the vault code.
        let invalid = CliError::vault_lookup(
            VaultError::InvalidId {
                id: "not-a-uuid".into(),
            },
            "not-a-uuid",
        );
        assert!(matches!(invalid, CliError::ProfileNotFound(_)));
        assert_eq!(invalid.exit_code(), exit::NOT_FOUND);
    }

    #[test]
    fn vault_lookup_keeps_genuine_vault_failures() {
        let wrong = CliError::vault_lookup(VaultError::WrongKey { id: "x".into() }, "x");
        assert!(matches!(wrong, CliError::Vault(_)));
        assert_eq!(wrong.exit_code(), exit::VAULT);
    }
}
