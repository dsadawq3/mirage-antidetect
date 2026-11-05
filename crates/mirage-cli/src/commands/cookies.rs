//! `mirage cookies ...`: move cookies between portable files and a profile's
//! Chromium cookie store.
//!
//! The Chromium store is encrypted with a DPAPI-protected key on Windows, so
//! the profile-facing functions are Windows-only. On other platforms the
//! commands compile but return [`CliError::Unsupported`].

use serde::Serialize;
use serde_json::Value;

use crate::cli::CookieFormat;
use crate::context::Ctx;
use crate::error::CliError;
use crate::output::{to_json_value, Render};

/// Result of `cookies import`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CookieImportReport {
    /// Target profile id.
    pub profile_id: String,
    /// Source format used.
    pub format: String,
    /// Number of cookies written.
    pub imported: usize,
}

/// Result of `cookies export --out <path>`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CookieExportInfo {
    /// Source profile id.
    pub profile_id: String,
    /// Output format used.
    pub format: String,
    /// File the cookies were written to.
    pub out: String,
    /// Number of bytes written.
    pub bytes: usize,
}

/// Import cookies from `args.file` into the profile's Chromium store.
#[cfg(windows)]
pub fn run_cookies_import(
    ctx: &Ctx,
    id: &str,
    format: CookieFormat,
    file: &std::path::Path,
) -> Result<CookieImportReport, CliError> {
    // Confirm the profile exists before touching its user-data-dir.
    let vault = ctx.open_vault()?;
    vault
        .load(id)
        .map_err(|error| CliError::vault_lookup(error, id))?;

    let input = std::fs::read_to_string(file)?;
    let profile_dir = ctx.profile_dir(id);
    let imported = match format {
        CookieFormat::Netscape => mirage_cookies::import_netscape(&profile_dir, &input)?,
        CookieFormat::Json => mirage_cookies::import_json(&profile_dir, &input)?,
    };

    Ok(CookieImportReport {
        profile_id: id.to_string(),
        format: format.as_str().to_string(),
        imported,
    })
}

/// Non-Windows stub: the Chromium cookie store is not reachable.
#[cfg(not(windows))]
pub fn run_cookies_import(
    _ctx: &Ctx,
    _id: &str,
    _format: CookieFormat,
    _file: &std::path::Path,
) -> Result<CookieImportReport, CliError> {
    Err(CliError::Unsupported(
        "cookie import needs the Chromium store (Windows DPAPI)",
    ))
}

/// Export the profile's cookies as a string (Netscape or JSON).
#[cfg(windows)]
pub fn run_cookies_export(ctx: &Ctx, id: &str, format: CookieFormat) -> Result<String, CliError> {
    // Confirm the profile exists before reading its user-data-dir.
    let vault = ctx.open_vault()?;
    vault
        .load(id)
        .map_err(|error| CliError::vault_lookup(error, id))?;

    let profile_dir = ctx.profile_dir(id);
    let data = match format {
        CookieFormat::Netscape => mirage_cookies::export_netscape(&profile_dir)?,
        CookieFormat::Json => mirage_cookies::export_json(&profile_dir)?,
    };
    Ok(data)
}

/// Non-Windows stub: the Chromium cookie store is not reachable.
#[cfg(not(windows))]
pub fn run_cookies_export(
    _ctx: &Ctx,
    _id: &str,
    _format: CookieFormat,
) -> Result<String, CliError> {
    Err(CliError::Unsupported(
        "cookie export needs the Chromium store (Windows DPAPI)",
    ))
}

impl Render for CookieImportReport {
    fn human(&self) -> String {
        format!(
            "imported {} cookie(s) into {} from {}",
            self.imported, self.profile_id, self.format
        )
    }

    fn json(&self) -> Value {
        to_json_value(self)
    }
}

impl Render for CookieExportInfo {
    fn human(&self) -> String {
        format!(
            "wrote {} byte(s) of {} cookies for {} to {}",
            self.bytes, self.format, self.profile_id, self.out
        )
    }

    fn json(&self) -> Value {
        to_json_value(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn ctx(dir: &TempDir) -> Ctx {
        Ctx {
            data_dir: dir.path().to_path_buf(),
            password: Some("pw".into()),
            json: false,
        }
    }

    #[test]
    fn import_missing_profile_is_not_found() {
        let dir = TempDir::new().expect("tempdir");
        let ctx = ctx(&dir);
        let file = dir.path().join("cookies.txt");
        std::fs::write(&file, "# Netscape HTTP Cookie File\n").expect("write");
        let result = run_cookies_import(
            &ctx,
            "00000000-0000-4000-8000-000000000000",
            CookieFormat::Netscape,
            &file,
        );
        // On Windows the vault load fails first; elsewhere it is unsupported.
        match result {
            Err(CliError::ProfileNotFound(_)) | Err(CliError::Unsupported(_)) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }
}
