//! Command-line grammar (clap derive).
//!
//! This module is the whole of the CLI's parsing surface: it defines the
//! global options and every command with its arguments. The `run_*` functions
//! in [`crate::commands`] merely consume these parsed structures, which keeps
//! them testable without spawning a process.

use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};

use crate::path::default_data_dir;

/// Mirage antidetect browser CLI — drives the crates directly, no server.
#[derive(Debug, Parser)]
#[command(
    name = "mirage",
    version,
    about = "Mirage antidetect browser CLI (direct crate access, no server)",
    long_about = "Manage encrypted Mirage profiles, launch masked browsers, check \
                  proxies, move cookies and import profiles from competing \
                  antidetect browsers. Everything runs locally against the \
                  mirage-* crates; there is no background server.",
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Data directory (default: %LOCALAPPDATA%\Mirage).
    #[arg(long, global = true, value_name = "DIR", default_value_os_t = default_data_dir())]
    pub data_dir: PathBuf,

    /// Vault master password (or set MIRAGE_VAULT_PASSWORD).
    #[arg(
        long,
        global = true,
        value_name = "PASS",
        env = "MIRAGE_VAULT_PASSWORD"
    )]
    pub password: Option<String>,

    /// Machine-readable JSON on stdout instead of human text.
    #[arg(long, global = true)]
    pub json: bool,

    /// The command to run.
    #[command(subcommand)]
    pub command: Command,
}

/// Top-level commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Manage profiles.
    #[command(subcommand)]
    Profile(ProfileCommand),

    /// Launch a profile and keep the browser in the foreground.
    Launch(LaunchArgs),

    /// Stop running sessions.
    Stop(StopArgs),

    /// List running sessions.
    Status,

    /// Proxy utilities.
    #[command(subcommand)]
    Proxy(ProxyCommand),

    /// Cookie import and export.
    #[command(subcommand)]
    Cookies(CookiesCommand),

    /// Import profiles from a competing antidetect browser.
    Import(ImportArgs),

    /// Fingerprint utilities.
    #[command(subcommand)]
    Fingerprint(FingerprintCommand),
}

/// Profile operations.
#[derive(Debug, Subcommand)]
pub enum ProfileCommand {
    /// List profiles (id, name, tags, proxy, dates).
    List,

    /// Print a profile's full JSON.
    Show(ProfileShowArgs),

    /// Create a profile from a seed.
    Create(ProfileCreateArgs),

    /// Delete a profile.
    Delete(ProfileDeleteArgs),

    /// Duplicate a profile under a new id.
    Duplicate(ProfileDuplicateArgs),
}

/// `profile show <id>`.
#[derive(Debug, Args)]
pub struct ProfileShowArgs {
    /// Profile id.
    pub id: String,
}

/// `profile create ...`.
#[derive(Debug, Args)]
pub struct ProfileCreateArgs {
    /// Profile display name.
    #[arg(long, value_name = "NAME")]
    pub name: String,

    /// Target platform of the legend.
    #[arg(long, value_enum)]
    pub os: Option<OsArg>,

    /// Fingerprint seed; omit the value (or the flag) for a random seed.
    #[arg(long, value_name = "N", num_args = 0..=1, default_missing_value = "")]
    pub seed: Option<String>,

    /// Attach a tag (repeatable).
    #[arg(long = "tag", value_name = "TAG")]
    pub tags: Vec<String>,

    /// Proxy as scheme://[user:pass@]host:port.
    #[arg(long, value_name = "URL")]
    pub proxy: Option<String>,
}

/// `profile delete <id>`.
#[derive(Debug, Args)]
pub struct ProfileDeleteArgs {
    /// Profile id.
    pub id: String,
}

/// `profile duplicate <id> --name <n>`.
#[derive(Debug, Args)]
pub struct ProfileDuplicateArgs {
    /// Source profile id.
    pub id: String,

    /// Name for the copy.
    #[arg(long, value_name = "NAME")]
    pub name: String,
}

/// `launch <id>`.
#[derive(Debug, Args)]
pub struct LaunchArgs {
    /// Profile id to launch.
    pub id: String,
}

/// `stop [<id>] [--all]`.
///
/// Exactly one of `<id>` / `--all` is required: clap rejects `stop` with no
/// target as a usage error (exit 2) before any command runs.
#[derive(Debug, Args)]
#[command(group(
    clap::ArgGroup::new("target")
        .required(true)
        .args(["id", "all"])
))]
pub struct StopArgs {
    /// Profile id to stop.
    pub id: Option<String>,

    /// Stop every running session.
    #[arg(long)]
    pub all: bool,
}

/// Proxy operations.
#[derive(Debug, Subcommand)]
pub enum ProxyCommand {
    /// Resolve the exit IP and latency through a proxy.
    Check(ProxyCheckArgs),
}

/// `proxy check <url>`.
#[derive(Debug, Args)]
pub struct ProxyCheckArgs {
    /// Proxy as scheme://[user:pass@]host:port.
    pub url: String,

    /// Overall check timeout in milliseconds.
    #[arg(long, value_name = "MS", default_value_t = 15000)]
    pub timeout_ms: u64,
}

/// Cookie operations.
#[derive(Debug, Subcommand)]
pub enum CookiesCommand {
    /// Import cookies into a profile's Chromium store.
    Import(CookiesImportArgs),

    /// Export cookies from a profile's Chromium store.
    Export(CookiesExportArgs),
}

/// `cookies import <id> --format .. --file ..`.
#[derive(Debug, Args)]
pub struct CookiesImportArgs {
    /// Profile id.
    pub id: String,

    /// Source format.
    #[arg(long, value_enum)]
    pub format: CookieFormat,

    /// File to read the cookies from.
    #[arg(long, value_name = "PATH")]
    pub file: PathBuf,
}

/// `cookies export <id> --format .. [--out ..]`.
#[derive(Debug, Args)]
pub struct CookiesExportArgs {
    /// Profile id.
    pub id: String,

    /// Output format.
    #[arg(long, value_enum)]
    pub format: CookieFormat,

    /// Write to this file instead of stdout.
    #[arg(long, value_name = "PATH")]
    pub out: Option<PathBuf>,
}

/// `import --source .. --file .. [--save] [--no-save]`.
#[derive(Debug, Args)]
pub struct ImportArgs {
    /// Source format.
    #[arg(long, value_enum)]
    pub source: ImportSource,

    /// JSON export file to import.
    #[arg(long, value_name = "PATH")]
    pub file: PathBuf,

    /// Save imported profiles into the vault (already the default).
    #[arg(long, action = ArgAction::SetTrue)]
    pub save: bool,

    /// Do not write imported profiles to the vault.
    #[arg(long, action = ArgAction::SetTrue)]
    pub no_save: bool,
}

impl ImportArgs {
    /// Effective save flag: on by default, `--no-save` turns it off, `--save`
    /// turns it back on.
    pub fn effective_save(&self) -> bool {
        self.save || !self.no_save
    }
}

/// Fingerprint operations.
#[derive(Debug, Subcommand)]
pub enum FingerprintCommand {
    /// Generate and print a fingerprint for a seed (legend debugging).
    Show(FingerprintShowArgs),
}

/// `fingerprint show --seed N [--os ..]`.
#[derive(Debug, Args)]
pub struct FingerprintShowArgs {
    /// Generation seed.
    #[arg(long, value_name = "N")]
    pub seed: u64,

    /// Target platform (derived from the seed when omitted).
    #[arg(long, value_enum)]
    pub os: Option<OsArg>,
}

/// Operating system choice on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OsArg {
    /// Windows.
    Windows,
    /// macOS.
    Macos,
    /// Linux.
    Linux,
}

impl OsArg {
    /// Map to the core [`mirage_core::Os`].
    pub fn to_core(self) -> mirage_core::Os {
        match self {
            OsArg::Windows => mirage_core::Os::Windows,
            OsArg::Macos => mirage_core::Os::Macos,
            OsArg::Linux => mirage_core::Os::Linux,
        }
    }
}

/// Cookie file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CookieFormat {
    /// Netscape `cookies.txt`.
    Netscape,
    /// JSON array of cookie objects.
    Json,
}

impl CookieFormat {
    /// Lowercase wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            CookieFormat::Netscape => "netscape",
            CookieFormat::Json => "json",
        }
    }
}

impl std::fmt::Display for CookieFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Competitor browser to import profiles from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ImportSource {
    /// AdsPower.
    Adspower,
    /// Dolphin {anty}.
    Dolphin,
    /// GoLogin.
    Gologin,
}

impl ImportSource {
    /// Lowercase wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            ImportSource::Adspower => "adspower",
            ImportSource::Dolphin => "dolphin",
            ImportSource::Gologin => "gologin",
        }
    }
}

impl std::fmt::Display for ImportSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn os_arg_maps_to_core() {
        assert_eq!(OsArg::Windows.to_core(), mirage_core::Os::Windows);
        assert_eq!(OsArg::Macos.to_core(), mirage_core::Os::Macos);
        assert_eq!(OsArg::Linux.to_core(), mirage_core::Os::Linux);
    }

    #[test]
    fn stop_requires_a_target() {
        // Neither an id nor --all: a usage error (clap), never a silent no-op.
        assert!(Cli::try_parse_from(["mirage", "stop"]).is_err());
        assert!(Cli::try_parse_from(["mirage", "stop", "abc"]).is_ok());
        assert!(Cli::try_parse_from(["mirage", "stop", "--all"]).is_ok());
    }

    #[test]
    fn import_save_defaults_and_opt_out() {
        let all = |extra: &[&str]| {
            let mut args = vec![
                "mirage", "import", "--source", "adspower", "--file", "x.json",
            ];
            args.extend_from_slice(extra);
            Cli::try_parse_from(args).expect("parse")
        };
        let default = all(&[]);
        match default.command {
            Command::Import(args) => assert!(args.effective_save()),
            other => panic!("expected import, got {other:?}"),
        }
        let no_save = all(&["--no-save"]);
        match no_save.command {
            Command::Import(args) => assert!(!args.effective_save()),
            other => panic!("expected import, got {other:?}"),
        }
        let re_enabled = all(&["--no-save", "--save"]);
        match re_enabled.command {
            Command::Import(args) => assert!(args.effective_save()),
            other => panic!("expected import, got {other:?}"),
        }
    }
}
