//! Process configuration: data directories and the environment overrides
//! documented in `docs/API.md`.

use std::path::{Path, PathBuf};

/// Default listening port (`MIRAGE_API_PORT`).
pub const DEFAULT_PORT: u16 = 35500;

/// Startup configuration resolved from the environment.
///
/// Fields are public so tests and embedders can construct a config directly
/// instead of mutating the process environment.
#[derive(Debug, Clone)]
pub struct Config {
    /// Root data directory. `vault\` and `profiles\` live inside it.
    pub data_dir: PathBuf,
    /// TCP port to bind on `127.0.0.1`.
    pub port: u16,
    /// Optional API token. When set, every `/api/v1/*` request except
    /// `/api/v1/health` must carry `Authorization: Bearer <token>`.
    pub token: Option<String>,
    /// Optional master password used to unlock the vault at startup.
    pub vault_password: Option<String>,
}

impl Config {
    /// Resolve the configuration from the process environment.
    ///
    /// * `MIRAGE_DATA_DIR` — root directory (default [`default_data_dir`]).
    /// * `MIRAGE_API_PORT` — listening port (default [`DEFAULT_PORT`]).
    /// * `MIRAGE_API_TOKEN` — Bearer token (empty value = disabled).
    /// * `MIRAGE_VAULT_PASSWORD` — startup vault password (empty = locked).
    pub fn from_env() -> Self {
        let data_dir = std::env::var_os("MIRAGE_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(default_data_dir);

        let port = std::env::var("MIRAGE_API_PORT")
            .ok()
            .and_then(|value| value.trim().parse::<u16>().ok())
            .unwrap_or(DEFAULT_PORT);

        let token = non_empty(std::env::var("MIRAGE_API_TOKEN").ok());
        let vault_password = non_empty(std::env::var("MIRAGE_VAULT_PASSWORD").ok());

        Config {
            data_dir,
            port,
            token,
            vault_password,
        }
    }

    /// Build a config for a specific data directory (tests, embedders).
    pub fn for_data_dir(data_dir: impl Into<PathBuf>) -> Self {
        Config {
            data_dir: data_dir.into(),
            port: DEFAULT_PORT,
            token: None,
            vault_password: None,
        }
    }

    /// The vault directory, `<data_dir>\vault`.
    pub fn vault_dir(&self) -> PathBuf {
        vault_dir(&self.data_dir)
    }

    /// The Chromium user-data-dir root, `<data_dir>\profiles`.
    pub fn profiles_dir(&self) -> PathBuf {
        profiles_dir(&self.data_dir)
    }
}

/// `<data_dir>\vault`.
pub fn vault_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("vault")
}

/// `<data_dir>\profiles`.
pub fn profiles_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("profiles")
}

/// Default data directory: `%LOCALAPPDATA%\Mirage` on Windows; a `.mirage`
/// folder under `$HOME` elsewhere; a relative `Mirage` folder as the last
/// resort so the server always has a place to write.
pub fn default_data_dir() -> PathBuf {
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local).join("Mirage");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".mirage");
    }
    PathBuf::from("Mirage")
}

/// `Some(value)` when the variable is present and not blank, else `None`.
fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_helpers_compose() {
        let config = Config::for_data_dir("C:/tmp/mirage");
        assert!(config.vault_dir().ends_with("vault"));
        assert!(config.profiles_dir().ends_with("profiles"));
        assert_eq!(config.port, DEFAULT_PORT);
    }

    #[test]
    fn non_empty_filters_blank() {
        assert_eq!(non_empty(None), None);
        assert_eq!(non_empty(Some(String::new())), None);
        assert_eq!(non_empty(Some("   ".to_string())), None);
        assert_eq!(non_empty(Some("abc".to_string())), Some("abc".to_string()));
    }
}
