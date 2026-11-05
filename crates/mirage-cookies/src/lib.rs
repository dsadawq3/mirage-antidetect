//! Mirage cookie import/export.
//!
//! A Mirage profile is a Chromium `user-data-dir` (`--user-data-dir=X`). Its
//! cookies live under a profile subdirectory, `<X>/Default/Network/Cookies`
//! (see [`chrome_db::cookies_db_path`]), encrypted with the key in
//! `<X>/Local State` at the root. This crate moves cookies between that store
//! and two portable formats:
//!
//! * Netscape `cookies.txt` — [`parse_netscape`] / [`serialize_netscape`];
//! * JSON — [`parse_json`] / [`serialize_json`], the [`Cookie`] array shape.
//!
//! The high-level helpers [`import_netscape`] / [`import_json`] write into a
//! profile, and [`export_netscape`] / [`export_json`] read out of one. The
//! profile-facing functions are Windows-only because the key is protected
//! with DPAPI; the parsers and serializers work everywhere.

mod error;
pub mod model;
pub mod netscape;

#[cfg(windows)]
pub mod chrome_db;
#[cfg(windows)]
pub mod oscrypt;

pub use error::CookieError;
pub use model::Cookie;
pub use netscape::{parse_netscape, serialize_netscape};

#[cfg(windows)]
pub use chrome_db::ReadStats;

/// Parse a JSON array of [`Cookie`] objects (serde `camelCase`).
pub fn parse_json(input: &str) -> Result<Vec<Cookie>, CookieError> {
    Ok(serde_json::from_str(input)?)
}

/// Serialize cookies to a pretty-printed JSON array.
pub fn serialize_json(cookies: &[Cookie]) -> Result<String, CookieError> {
    Ok(serde_json::to_string_pretty(cookies)?)
}

/// Counters describing what an export emitted and skipped.
#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExportStats {
    /// Cookies written to the output (session cookies included, expired ones
    /// dropped).
    pub exported: usize,
    /// Cookies skipped because their value uses unsupported app-bound (`v20`)
    /// encryption.
    pub skipped_unsupported: usize,
}

/// Read, decrypt and export a profile's cookies as Netscape `cookies.txt`.
///
/// Expired cookies (a finite expiry in the past) are dropped; session
/// cookies are kept. Fails with [`CookieError::UnsupportedEncryption`] when
/// the profile has cookies but none can be read (all app-bound `v20`), so an
/// empty export is never mistaken for "no cookies".
#[cfg(windows)]
pub fn export_netscape(profile_dir: &std::path::Path) -> Result<String, CookieError> {
    Ok(export_netscape_with_stats(profile_dir)?.0)
}

/// [`export_netscape`] plus [`ExportStats`].
#[cfg(windows)]
pub fn export_netscape_with_stats(
    profile_dir: &std::path::Path,
) -> Result<(String, ExportStats), CookieError> {
    let (cookies, stats) = collect_export_cookies(profile_dir)?;
    Ok((serialize_netscape(&cookies), stats))
}

/// Read, decrypt and export a profile's cookies as JSON.
///
/// Expired cookies are dropped; session cookies are kept. Fails with
/// [`CookieError::UnsupportedEncryption`] when every cookie is app-bound
/// `v20`.
#[cfg(windows)]
pub fn export_json(profile_dir: &std::path::Path) -> Result<String, CookieError> {
    Ok(export_json_with_stats(profile_dir)?.0)
}

/// [`export_json`] plus [`ExportStats`].
#[cfg(windows)]
pub fn export_json_with_stats(
    profile_dir: &std::path::Path,
) -> Result<(String, ExportStats), CookieError> {
    let (cookies, stats) = collect_export_cookies(profile_dir)?;
    Ok((serialize_json(&cookies)?, stats))
}

/// Parse Netscape `cookies.txt` and upsert every cookie into the profile.
///
/// Returns the number of distinct cookies written.
#[cfg(windows)]
pub fn import_netscape(profile_dir: &std::path::Path, input: &str) -> Result<usize, CookieError> {
    let cookies = parse_netscape(input)?;
    chrome_db::write_cookies(profile_dir, &cookies)
}

/// Parse a JSON cookie array and upsert every cookie into the profile.
///
/// Returns the number of distinct cookies written.
#[cfg(windows)]
pub fn import_json(profile_dir: &std::path::Path, input: &str) -> Result<usize, CookieError> {
    let cookies = parse_json(input)?;
    chrome_db::write_cookies(profile_dir, &cookies)
}

/// Read the profile's cookies for an export and apply the shared policy:
/// drop expired cookies, and turn "all cookies unreadable" into an explicit
/// error rather than a silently empty document.
#[cfg(windows)]
fn collect_export_cookies(
    profile_dir: &std::path::Path,
) -> Result<(Vec<Cookie>, ExportStats), CookieError> {
    let (read, read_stats) = chrome_db::read_cookies_with_stats(profile_dir)?;

    // No readable cookie but some were skipped for unsupported encryption:
    // every value is app-bound `v20`. An empty file would be a lie.
    if read.is_empty() && read_stats.skipped_unsupported > 0 {
        return Err(CookieError::UnsupportedEncryption(format!(
            "{} cookies use app-bound encryption (v20); export is unavailable",
            read_stats.skipped_unsupported
        )));
    }

    let cookies = active_cookies(read);
    let stats = ExportStats {
        exported: cookies.len(),
        skipped_unsupported: read_stats.skipped_unsupported,
    };
    Ok((cookies, stats))
}

/// Drop cookies whose finite expiry is at or before `now` (Unix seconds).
#[cfg(windows)]
fn active_cookies(cookies: Vec<Cookie>) -> Vec<Cookie> {
    let now = chrome_db::now_unix_seconds();
    cookies
        .into_iter()
        .filter(|cookie| cookie.expiry.is_none_or(|expiry| expiry > now))
        .collect()
}
