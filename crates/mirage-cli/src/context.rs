//! Shared command context: data directory, vault password and output mode.
//!
//! Every `run_*` command takes a [`Ctx`] and derives all its paths from it, so
//! the commands stay testable against a temporary data directory.

use std::path::PathBuf;

use mirage_vault::Vault;

use crate::error::CliError;

/// Environment variable that supplies the vault password.
pub const VAULT_PASSWORD_ENV: &str = "MIRAGE_VAULT_PASSWORD";

/// Everything a command needs besides its own arguments.
#[derive(Debug, Clone)]
pub struct Ctx {
    /// Root data directory (`%LOCALAPPDATA%\Mirage` by default).
    pub data_dir: PathBuf,
    /// Vault master password, if provided on the command line or via env.
    pub password: Option<String>,
    /// Print JSON instead of human text.
    pub json: bool,
}

impl Ctx {
    /// The encrypted-profile directory: `<data-dir>/vault`.
    pub fn vault_dir(&self) -> PathBuf {
        self.data_dir.join("vault")
    }

    /// The Chromium user-data-dir root: `<data-dir>/profiles`.
    pub fn profiles_dir(&self) -> PathBuf {
        self.data_dir.join("profiles")
    }

    /// The user-data-dir of one profile: `<data-dir>/profiles/<id>`.
    pub fn profile_dir(&self, id: &str) -> PathBuf {
        self.profiles_dir().join(id)
    }

    /// The running-session registry directory: `<data-dir>/run`.
    pub fn run_dir(&self) -> PathBuf {
        self.data_dir.join("run")
    }

    /// Resolve the vault password, or fail with a hint about the two sources.
    pub fn resolve_password(&self) -> Result<String, CliError> {
        if let Some(password) = self.password.as_ref().filter(|value| !value.is_empty()) {
            return Ok(password.clone());
        }
        if let Ok(password) = std::env::var(VAULT_PASSWORD_ENV) {
            if !password.is_empty() {
                return Ok(password);
            }
        }
        Err(CliError::PasswordMissing)
    }

    /// Open (creating if needed) the vault this context points at.
    pub fn open_vault(&self) -> Result<Vault, CliError> {
        let password = self.resolve_password()?;
        Vault::open(self.vault_dir(), &password).map_err(CliError::Vault)
    }
}
