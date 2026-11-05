//! `mirage import`: import profiles from a competing antidetect browser.

use serde::Serialize;
use serde_json::Value;

use mirage_import::{
    import_ads_power_all, import_dolphin_all, import_gologin_all, ImportResult, ImportWarning,
};

use crate::cli::{ImportArgs, ImportSource};
use crate::context::Ctx;
use crate::error::CliError;
use crate::output::{to_json_value, Render};

/// One imported profile with its field-level warnings.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedProfile {
    /// New Mirage profile id.
    pub id: String,
    /// Profile name.
    pub name: String,
    /// Human-readable warnings (`mapped: proxy`, `defaulted: fonts`, ...).
    pub warnings: Vec<String>,
}

/// Result of `mirage import`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    /// Source format name.
    pub source: String,
    /// Whether the profiles were written to the vault.
    pub saved: bool,
    /// The imported profiles.
    pub profiles: Vec<ImportedProfile>,
    /// Total warning count across all profiles.
    pub warnings: usize,
}

/// Import profiles from `args.file` and optionally store them in the vault.
pub fn run_import(ctx: &Ctx, args: &ImportArgs) -> Result<ImportReport, CliError> {
    let input = std::fs::read_to_string(&args.file)?;

    let results: Vec<ImportResult> = match args.source {
        ImportSource::Adspower => import_ads_power_all(&input)?,
        ImportSource::Dolphin => import_dolphin_all(&input)?,
        ImportSource::Gologin => import_gologin_all(&input)?,
    };

    let save = args.effective_save();
    let vault = if save { Some(ctx.open_vault()?) } else { None };

    let mut profiles = Vec::with_capacity(results.len());
    let mut total_warnings = 0usize;
    for result in results {
        if let Some(vault) = &vault {
            vault.save(&result.profile)?;
        }
        let warnings: Vec<String> = result.warnings.iter().map(format_warning).collect();
        total_warnings += warnings.len();
        profiles.push(ImportedProfile {
            id: result.profile.id,
            name: result.profile.name,
            warnings,
        });
    }

    Ok(ImportReport {
        source: args.source.as_str().to_string(),
        saved: save,
        profiles,
        warnings: total_warnings,
    })
}

/// Render one [`ImportWarning`] as `kind: field`.
fn format_warning(warning: &ImportWarning) -> String {
    let kind = if warning.is_mapped() {
        "mapped"
    } else if warning.is_defaulted() {
        "defaulted"
    } else {
        "ignored"
    };
    format!("{kind}: {}", warning.field())
}

impl Render for ImportReport {
    fn human(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "imported {} profile(s) from {}{}\n",
            self.profiles.len(),
            self.source,
            if self.saved { " (saved to vault)" } else { "" }
        ));
        for profile in &self.profiles {
            out.push_str(&format!("- {}  {}\n", profile.id, profile.name));
            for warning in &profile.warnings {
                out.push_str(&format!("    {warning}\n"));
            }
        }
        out.push_str(&format!("{} warning(s)", self.warnings));
        out
    }

    fn json(&self) -> Value {
        to_json_value(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::profile::run_profile_list;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn ctx(dir: &TempDir) -> Ctx {
        Ctx {
            data_dir: dir.path().to_path_buf(),
            password: Some("pw".into()),
            json: false,
        }
    }

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("mirage-import")
            .join("tests")
            .join("fixtures")
            .join("adspower_list.json")
    }

    #[test]
    fn import_saves_profiles_and_reports_warnings() {
        let dir = TempDir::new().expect("tempdir");
        let ctx = ctx(&dir);
        let args = ImportArgs {
            source: ImportSource::Adspower,
            file: fixture(),
            save: false,
            no_save: false,
        };
        let report = run_import(&ctx, &args).expect("import");
        assert_eq!(report.profiles.len(), 1);
        assert_eq!(report.profiles[0].name, "Shop-01");
        assert!(report.saved);
        assert!(report.warnings > 0);

        let list = run_profile_list(&ctx).expect("list");
        assert_eq!(list.profiles.len(), 1);
        assert_eq!(list.profiles[0].id, report.profiles[0].id);
    }

    #[test]
    fn no_save_leaves_vault_empty() {
        let dir = TempDir::new().expect("tempdir");
        let ctx = ctx(&dir);
        let args = ImportArgs {
            source: ImportSource::Adspower,
            file: fixture(),
            save: false,
            no_save: true,
        };
        let report = run_import(&ctx, &args).expect("import");
        assert!(!report.saved);
        assert_eq!(report.profiles.len(), 1);

        let list = run_profile_list(&ctx).expect("list");
        assert!(list.profiles.is_empty());
    }

    #[test]
    fn bad_json_surfaces_import_error() {
        let dir = TempDir::new().expect("tempdir");
        let ctx = ctx(&dir);
        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, "{ not json").expect("write");
        let args = ImportArgs {
            source: ImportSource::Gologin,
            file: bad,
            save: false,
            no_save: false,
        };
        let error = run_import(&ctx, &args).expect_err("bad json");
        assert!(matches!(error, CliError::Import(_)));
    }
}
