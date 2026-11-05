//! Shared application state: data directories, the single session manager,
//! and the vault behind an `RwLock`.
//!
//! The vault starts locked. It is opened either at construction (when
//! `MIRAGE_VAULT_PASSWORD` is set) or later through `POST /vault/unlock`;
//! until then profile endpoints answer `401 vault_locked`.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use mirage_launcher::SessionManager;
use mirage_vault::{Vault, VaultError};

use crate::config::{profiles_dir, vault_dir, Config};
use crate::error::ApiError;

/// Why an unlock attempt failed.
#[derive(Debug)]
pub enum UnlockError {
    /// The password does not match the existing vault contents.
    WrongPassword,
    /// The vault could not be read (filesystem / corruption).
    Io(String),
}

impl std::fmt::Display for UnlockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnlockError::WrongPassword => f.write_str("invalid vault password"),
            UnlockError::Io(detail) => write!(f, "vault i/o error: {detail}"),
        }
    }
}

/// Interior state shared by every handler.
struct Inner {
    data_dir: PathBuf,
    vault_dir: PathBuf,
    profiles_dir: PathBuf,
    session: SessionManager,
    /// `None` while locked; `Some` after a successful unlock.
    vault: RwLock<Option<Arc<Vault>>>,
    token: Option<String>,
}

/// Cheap, cloneable handle to the application state.
#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

impl AppState {
    /// Create the state and its directories; unlock from `config` if a startup
    /// password is present. A failed startup unlock leaves the vault locked
    /// (the operator can retry through `POST /vault/unlock`).
    pub fn new(config: Config) -> Self {
        let profiles_dir = profiles_dir(&config.data_dir);
        let vault_dir = vault_dir(&config.data_dir);

        // Directory creation is best effort here; the vault and launcher both
        // create what they need again on first use.
        let _ = std::fs::create_dir_all(&config.data_dir);
        let _ = std::fs::create_dir_all(&vault_dir);
        let _ = std::fs::create_dir_all(&profiles_dir);

        let state = AppState {
            inner: Arc::new(Inner {
                data_dir: config.data_dir,
                vault_dir,
                profiles_dir,
                session: SessionManager::new(),
                vault: RwLock::new(None),
                token: config.token,
            }),
        };

        if let Some(password) = config.vault_password {
            // Errors are intentionally ignored: the state simply stays locked
            // and the operator unlocks through the endpoint.
            let _ = state.unlock(&password);
        }

        state
    }

    /// Root data directory.
    pub fn data_dir(&self) -> &PathBuf {
        &self.inner.data_dir
    }

    /// Vault directory.
    pub fn vault_dir(&self) -> &PathBuf {
        &self.inner.vault_dir
    }

    /// Chromium user-data-dir root.
    pub fn profiles_dir(&self) -> &PathBuf {
        &self.inner.profiles_dir
    }

    /// Per-profile Chromium user-data-dir: `<data_dir>\profiles\<id>`.
    pub fn profile_dir(&self, id: &str) -> PathBuf {
        self.inner.profiles_dir.join(id)
    }

    /// The configured API token, if any.
    pub fn token(&self) -> Option<&str> {
        self.inner.token.as_deref()
    }

    /// The session manager (running profiles).
    pub fn session(&self) -> &SessionManager {
        &self.inner.session
    }

    /// Snapshot of the vault, `None` while locked. A poisoned lock is treated
    /// as locked rather than panicking.
    pub fn vault(&self) -> Option<Arc<Vault>> {
        self.inner.vault.read().ok().and_then(|guard| guard.clone())
    }

    /// Whether the vault is currently unlocked.
    pub fn is_unlocked(&self) -> bool {
        self.vault().is_some()
    }

    /// The vault or `401 vault_locked`.
    pub fn require_vault(&self) -> Result<Arc<Vault>, ApiError> {
        self.vault().ok_or(ApiError::VaultLocked)
    }

    /// Open and verify the vault with `password`, replacing the current one on
    /// success.
    pub fn unlock(&self, password: &str) -> Result<(), UnlockError> {
        let vault = open_and_verify(&self.inner.vault_dir, password)?;
        let mut guard = self
            .inner
            .vault
            .write()
            .map_err(|_| UnlockError::Io("vault lock poisoned".to_string()))?;
        *guard = Some(vault);
        Ok(())
    }
}

/// Open the vault and prove the password is right.
///
/// `Vault::open` only derives a key, so a wrong password is invisible until a
/// profile is decrypted. When the vault already holds profiles, the first one
/// is loaded: a tag failure means the password is wrong. An empty vault has no
/// secret to protect, so any password is accepted and simply defines the key.
fn open_and_verify(dir: &PathBuf, password: &str) -> Result<Arc<Vault>, UnlockError> {
    let vault = Vault::open(dir, password).map_err(classify_open_error)?;

    if let Some(first) = vault
        .list()
        .map_err(classify_open_error)?
        .into_iter()
        .next()
    {
        match vault.load(&first) {
            Ok(_) => {}
            Err(VaultError::WrongKey { .. }) => return Err(UnlockError::WrongPassword),
            Err(error) => return Err(classify_open_error(error)),
        }
    }

    Ok(Arc::new(vault))
}

/// Map a vault failure during open/verify onto an unlock error.
fn classify_open_error(error: VaultError) -> UnlockError {
    match error {
        VaultError::WrongKey { .. } => UnlockError::WrongPassword,
        other => UnlockError::Io(other.to_string()),
    }
}

/// Map a vault failure from a normal CRUD operation onto a native [`ApiError`].
///
/// A missing — or syntactically invalid — id reads as `not_found` so a UI
/// asking about an unknown profile gets a 404, not a 400. A tag failure means
/// the current key does not match the stored data: the vault is effectively
/// locked for this profile.
pub fn vault_error(error: VaultError) -> ApiError {
    match error {
        VaultError::NotFound { id } => ApiError::NotFound(format!("profile `{id}` not found")),
        VaultError::InvalidId { id } => ApiError::NotFound(format!("profile `{id}` not found")),
        VaultError::WrongKey { .. } => ApiError::VaultLocked,
        VaultError::Serde { source } => {
            ApiError::Internal(format!("profile serialization failed: {source}"))
        }
        VaultError::Corrupted { path, reason } => {
            ApiError::Internal(format!("vault file {path:?} is corrupted: {reason}"))
        }
        VaultError::Io { source } => ApiError::Internal(format!("vault i/o error: {source}")),
        VaultError::Crypto => ApiError::Internal("cryptographic operation failed".to_string()),
    }
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("data_dir", &self.inner.data_dir)
            .field("unlocked", &self.is_unlocked())
            .field("token", &self.inner.token.is_some())
            .finish()
    }
}
