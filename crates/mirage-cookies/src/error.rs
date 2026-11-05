//! Error types for the cookie import/export crate.

use std::path::PathBuf;

use thiserror::Error;

/// Everything that can go wrong while reading or writing cookies.
#[derive(Debug, Error)]
pub enum CookieError {
    /// The profile has no usable Chromium cookie key: `Local State` is
    /// missing, is not JSON, has no `os_crypt.encrypted_key`, or the value
    /// is not a `DPAPI`-prefixed blob. The profile never ran, or it is not
    /// a Chromium user-data-dir.
    #[error("profile has no usable cookie key: {0}")]
    NoProfileKey(String),

    /// The cookie database file does not exist under the profile directory.
    #[error("cookie database not found: {0}")]
    NoProfileDb(PathBuf),

    /// The cookie database exists but cannot be opened or locked: the
    /// browser is almost certainly still running against this profile.
    #[error("cookie database is locked (close the browser first): {0}")]
    DbLocked(String),

    /// A line of a Netscape cookies.txt file could not be parsed. Reported
    /// with the 1-based line number; the parser never panics on bad input.
    #[error("cookies.txt parse error on line {line}: {reason}")]
    Parse {
        /// 1-based line number of the offending line.
        line: usize,
        /// Human-readable reason.
        reason: String,
    },

    /// A cryptographic primitive failed: bad key length, failed DPAPI call,
    /// AES-GCM tag mismatch or non-UTF-8 plaintext.
    #[error("cryptographic operation failed: {0}")]
    Crypto(String),

    /// The stored value uses an encryption scheme this crate does not
    /// implement (Chromium app-bound encryption, the `v20` prefix). Such
    /// cookies are skipped on read; an export that would be empty because
    /// *every* cookie is unsupported fails with this error instead.
    #[error("unsupported cookie encryption: {0}")]
    UnsupportedEncryption(String),

    /// The cookie database was created by an older Chromium schema (before
    /// v24). Writing the v24 `SHA-256(domain)`-prefixed value format into it
    /// would corrupt the values when Chromium later migrates the database, so
    /// the write is refused rather than performed silently.
    #[error(
        "profile cookie database schema is v{found}, older than the required v24; \
         open the profile once with a current Chromium to migrate it before importing"
    )]
    SchemaTooOld {
        /// Version found in `meta.version` (0 when the row is absent).
        found: i64,
    },

    /// Underlying SQLite failure. `From` is implemented by hand below so
    /// lock/open failures can be reported as [`CookieError::DbLocked`].
    #[cfg(windows)]
    #[error("cookie database error: {0}")]
    Db(rusqlite::Error),

    /// A JSON payload was not a valid `Vec<Cookie>` or could not be encoded.
    #[error("cookie JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// Raw filesystem failure.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

/// Map a raw SQLite error to the most useful variant.
///
/// Lock and "cannot open" failures carry a distinct variant so callers can
/// tell "the browser is still holding the file" from real database damage.
#[cfg(windows)]
impl From<rusqlite::Error> for CookieError {
    fn from(error: rusqlite::Error) -> Self {
        use rusqlite::ErrorCode;

        match error.sqlite_error_code() {
            Some(ErrorCode::DatabaseBusy) | Some(ErrorCode::DatabaseLocked) => {
                CookieError::DbLocked(error.to_string())
            }
            // The file was confirmed to exist before opening, so a failure
            // to open it is almost always a sharing violation from a running
            // browser holding the database.
            Some(ErrorCode::CannotOpen) => CookieError::DbLocked(format!(
                "cannot open cookie database, it is held by another process: {error}"
            )),
            _ => CookieError::Db(error),
        }
    }
}
