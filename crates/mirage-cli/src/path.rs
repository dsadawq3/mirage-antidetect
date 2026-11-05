//! Default data-directory resolution.
//!
//! The on-disk layout is fixed by `docs/CONTRACT.md`:
//! `<data-dir>/vault/<id>.mprof`, `<data-dir>/vault/.vault.salt` and
//! `<data-dir>/profiles/<id>/`. The default data dir is
//! `%LOCALAPPDATA%\Mirage` on Windows; the environment variable
//! `MIRAGE_DATA_DIR` overrides it (used by tests and by portable installs).

use std::path::PathBuf;

/// Environment variable that overrides the data directory.
pub const DATA_DIR_ENV: &str = "MIRAGE_DATA_DIR";

/// Resolve the default data directory.
///
/// Precedence: `MIRAGE_DATA_DIR`, then `%LOCALAPPDATA%\Mirage` (Windows),
/// then `$HOME/.local/share/Mirage`, then `<temp>/Mirage` as a last resort.
pub fn default_data_dir() -> PathBuf {
    if let Some(dir) = data_dir_override() {
        return dir;
    }
    if let Some(local) = env_path("LOCALAPPDATA") {
        return local.join("Mirage");
    }
    if let Some(home) = env_path("HOME") {
        return home.join(".local").join("share").join("Mirage");
    }
    std::env::temp_dir().join("Mirage")
}

/// The `MIRAGE_DATA_DIR` override, when set to a non-empty path.
fn data_dir_override() -> Option<PathBuf> {
    std::env::var_os(DATA_DIR_ENV)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

/// Read a non-empty path from an environment variable.
fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}
