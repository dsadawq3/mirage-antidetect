//! `mirage profile ...`: profile CRUD backed by [`mirage_vault`].

use serde::Serialize;
use serde_json::Value;

use mirage_core::{generate, Profile, ProxyScheme};

use crate::cli::ProfileCreateArgs;
use crate::context::Ctx;
use crate::error::CliError;
use crate::output::{render_table, to_json_value, Render};
use crate::proxy_url::parse_proxy_url;

/// One row of `profile list`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSummary {
    /// Profile id.
    pub id: String,
    /// Profile name.
    pub name: String,
    /// Tags.
    pub tags: Vec<String>,
    /// Generation seed recorded on the profile (`0` = unknown/legacy).
    pub seed: u64,
    /// `scheme://host:port`, or `None` for a direct connection.
    pub proxy: Option<String>,
    /// RFC 3339 creation timestamp.
    pub created_at: String,
    /// RFC 3339 last-update timestamp.
    pub updated_at: String,
}

/// Result of `profile list`.
#[derive(Debug, Clone, Serialize)]
pub struct ProfileList {
    /// The profiles, sorted by id.
    pub profiles: Vec<ProfileSummary>,
}

/// Result of `profile create` / `profile duplicate`.
#[derive(Debug, Clone)]
pub struct CreatedProfile {
    /// The freshly stored profile.
    pub profile: Profile,
}

/// Result of `profile show`.
#[derive(Debug, Clone)]
pub struct ProfileView {
    /// The profile.
    pub profile: Profile,
}

/// Result of `profile delete`.
#[derive(Debug, Clone, Serialize)]
pub struct DeletedProfile {
    /// The id that was deleted.
    pub deleted: String,
}

/// List every profile in the vault.
pub fn run_profile_list(ctx: &Ctx) -> Result<ProfileList, CliError> {
    let vault = ctx.open_vault()?;
    let mut profiles = Vec::new();
    for id in vault.list()? {
        match vault.load(&id) {
            Ok(profile) => profiles.push(summary(&profile)),
            // The id raced away between list and load: skip it.
            Err(mirage_vault::VaultError::NotFound { .. }) => continue,
            Err(error) => return Err(CliError::Vault(error)),
        }
    }
    Ok(ProfileList { profiles })
}

/// Load and return the full profile with `id`.
pub fn run_profile_show(ctx: &Ctx, id: &str) -> Result<ProfileView, CliError> {
    let vault = ctx.open_vault()?;
    let profile = vault
        .load(id)
        .map_err(|error| CliError::vault_lookup(error, id))?;
    Ok(ProfileView { profile })
}

/// Create a profile from `args` and store it.
pub fn run_profile_create(ctx: &Ctx, args: &ProfileCreateArgs) -> Result<CreatedProfile, CliError> {
    let seed = resolve_seed(args.seed.as_deref())?;
    let os = args.os.map(|arg| arg.to_core());
    let fingerprint = generate(seed, os)?;

    let proxy = match args.proxy.as_deref() {
        Some(url) => Some(parse_proxy_url(url)?.to_core()),
        None => None,
    };

    let mut profile = Profile::new(args.name.clone(), fingerprint, proxy).with_seed(seed);
    profile.tags = args.tags.clone();

    let vault = ctx.open_vault()?;
    vault.save(&profile)?;
    Ok(CreatedProfile { profile })
}

/// Delete the profile with `id`.
pub fn run_profile_delete(ctx: &Ctx, id: &str) -> Result<DeletedProfile, CliError> {
    let vault = ctx.open_vault()?;
    vault
        .delete(id)
        .map_err(|error| CliError::vault_lookup(error, id))?;
    Ok(DeletedProfile {
        deleted: id.to_string(),
    })
}

/// Duplicate the profile with `args.id` under `args.name` and a fresh id.
pub fn run_profile_duplicate(
    ctx: &Ctx,
    args: &crate::cli::ProfileDuplicateArgs,
) -> Result<CreatedProfile, CliError> {
    let vault = ctx.open_vault()?;
    let original = vault
        .load(&args.id)
        .map_err(|error| CliError::vault_lookup(error, &args.id))?;

    let mut copy = Profile::new(
        args.name.clone(),
        original.fingerprint.clone(),
        original.proxy.clone(),
    );
    // The clone keeps the exact fingerprint, so it keeps its generation seed.
    copy.seed = original.seed;
    copy.tags = original.tags.clone();
    copy.notes = original.notes.clone();

    vault.save(&copy)?;
    Ok(CreatedProfile { profile: copy })
}

/// Reduce a stored profile to its list row.
fn summary(profile: &Profile) -> ProfileSummary {
    ProfileSummary {
        id: profile.id.clone(),
        name: profile.name.clone(),
        tags: profile.tags.clone(),
        seed: profile.seed,
        proxy: profile.proxy.as_ref().map(|proxy| {
            format!(
                "{}://{}:{}",
                scheme_str(proxy.scheme),
                proxy.host,
                proxy.port
            )
        }),
        created_at: profile.created_at.clone(),
        updated_at: profile.updated_at.clone(),
    }
}

/// Wire name of a core proxy scheme.
fn scheme_str(scheme: ProxyScheme) -> &'static str {
    match scheme {
        ProxyScheme::Http => "http",
        ProxyScheme::Socks5 => "socks5",
    }
}

/// Turn the raw `--seed` value into a concrete seed: absent or empty means a
/// random one, otherwise it must parse as an unsigned integer.
fn resolve_seed(raw: Option<&str>) -> Result<u64, CliError> {
    match raw {
        None => Ok(rand::random()),
        Some(text) if text.trim().is_empty() => Ok(rand::random()),
        Some(text) => text.trim().parse::<u64>().map_err(|_| {
            CliError::Message(format!(
                "invalid --seed `{text}`: expected an unsigned integer"
            ))
        }),
    }
}

impl Render for ProfileList {
    fn human(&self) -> String {
        if self.profiles.is_empty() {
            return String::from("no profiles");
        }
        let headers = ["ID", "NAME", "TAGS", "SEED", "PROXY", "CREATED", "UPDATED"];
        let rows = self
            .profiles
            .iter()
            .map(|profile| {
                vec![
                    profile.id.clone(),
                    profile.name.clone(),
                    profile.tags.join(","),
                    profile.seed.to_string(),
                    profile
                        .proxy
                        .clone()
                        .unwrap_or_else(|| String::from("direct")),
                    profile.created_at.clone(),
                    profile.updated_at.clone(),
                ]
            })
            .collect::<Vec<_>>();
        render_table(&headers, &rows)
    }

    fn json(&self) -> Value {
        to_json_value(self)
    }
}

impl Render for CreatedProfile {
    fn human(&self) -> String {
        self.profile.id.clone()
    }

    fn json(&self) -> Value {
        to_json_value(&self.profile)
    }
}

impl Render for ProfileView {
    fn human(&self) -> String {
        serde_json::to_string_pretty(&self.profile)
            .unwrap_or_else(|error| format!("<failed to render profile: {error}>"))
    }

    fn json(&self) -> Value {
        to_json_value(&self.profile)
    }
}

impl Render for DeletedProfile {
    fn human(&self) -> String {
        format!("deleted {}", self.deleted)
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

    fn create_args(name: &str) -> ProfileCreateArgs {
        ProfileCreateArgs {
            name: name.into(),
            os: Some(crate::cli::OsArg::Windows),
            seed: Some("42".into()),
            tags: vec!["work".into()],
            proxy: None,
        }
    }

    #[test]
    fn create_list_show_duplicate_delete_lifecycle() {
        let dir = TempDir::new().expect("tempdir");
        let ctx = ctx(&dir);

        let created = run_profile_create(&ctx, &create_args("Shop-01")).expect("create");
        let id = created.profile.id.clone();
        assert_eq!(created.profile.name, "Shop-01");
        assert_eq!(created.profile.tags, vec!["work".to_string()]);
        assert_eq!(created.profile.seed, 42);

        let list = run_profile_list(&ctx).expect("list");
        assert_eq!(list.profiles.len(), 1);
        assert_eq!(list.profiles[0].id, id);

        let shown = run_profile_show(&ctx, &id).expect("show");
        assert_eq!(shown.profile, created.profile);

        let dup = run_profile_duplicate(
            &ctx,
            &crate::cli::ProfileDuplicateArgs {
                id: id.clone(),
                name: "Shop-02".into(),
            },
        )
        .expect("duplicate");
        assert_ne!(dup.profile.id, id);
        assert_eq!(dup.profile.fingerprint, created.profile.fingerprint);
        assert_eq!(dup.profile.tags, created.profile.tags);
        assert_eq!(dup.profile.seed, created.profile.seed);

        let list = run_profile_list(&ctx).expect("list");
        assert_eq!(list.profiles.len(), 2);

        run_profile_delete(&ctx, &id).expect("delete");
        let list = run_profile_list(&ctx).expect("list");
        assert_eq!(list.profiles.len(), 1);
        assert_eq!(list.profiles[0].id, dup.profile.id);
    }

    #[test]
    fn seed_is_reproducible_and_random_when_absent() {
        let dir = TempDir::new().expect("tempdir");
        let ctx = ctx(&dir);

        let first = run_profile_create(&ctx, &create_args("A")).expect("a");
        let second = run_profile_create(&ctx, &create_args("B")).expect("b");
        // Same explicit seed -> identical fingerprint regardless of name.
        assert_eq!(first.profile.fingerprint, second.profile.fingerprint);
        assert_eq!(first.profile.seed, 42);

        let mut args = create_args("C");
        args.seed = None;
        let random = run_profile_create(&ctx, &args).expect("c");
        assert_ne!(random.profile.fingerprint, first.profile.fingerprint);
    }

    #[test]
    fn delete_missing_profile_is_not_found() {
        let dir = TempDir::new().expect("tempdir");
        let ctx = ctx(&dir);
        let missing = "00000000-0000-4000-8000-000000000000";
        let error = run_profile_delete(&ctx, missing).expect_err("missing");
        assert!(matches!(error, CliError::ProfileNotFound(_)));
        assert_eq!(error.exit_code(), crate::error::exit::NOT_FOUND);
    }

    #[test]
    fn create_with_proxy_parses_and_stores() {
        let dir = TempDir::new().expect("tempdir");
        let ctx = ctx(&dir);
        let mut args = create_args("P");
        args.proxy = Some("socks5://u:p@1.2.3.4:1080".into());
        let created = run_profile_create(&ctx, &args).expect("create");
        let proxy = created.profile.proxy.expect("proxy");
        assert_eq!(proxy.host, "1.2.3.4");
        assert_eq!(proxy.port, 1080);
        assert_eq!(proxy.username.as_deref(), Some("u"));
    }

    #[test]
    fn human_list_marks_direct_and_renders_table() {
        let dir = TempDir::new().expect("tempdir");
        let ctx = ctx(&dir);
        run_profile_create(&ctx, &create_args("Shop-01")).expect("create");
        let rendered = run_profile_list(&ctx).expect("list").human();
        assert!(rendered.contains("Shop-01"));
        assert!(rendered.contains("direct"));
        assert!(rendered.contains("ID"));
    }

    #[test]
    fn missing_password_is_reported() {
        let dir = TempDir::new().expect("tempdir");
        let ctx = Ctx {
            data_dir: dir.path().to_path_buf(),
            password: None,
            json: false,
        };
        // Ensure the env fallback is not accidentally satisfied by the runner.
        if std::env::var(crate::context::VAULT_PASSWORD_ENV).is_err() {
            let error = run_profile_list(&ctx).expect_err("no password");
            assert!(matches!(error, CliError::PasswordMissing));
            assert_eq!(error.exit_code(), crate::error::exit::VAULT);
        }
    }
}
