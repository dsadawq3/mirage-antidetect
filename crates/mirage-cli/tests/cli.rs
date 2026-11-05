//! Integration tests for the Mirage CLI.
//!
//! These exercise the two halves of the crate through its public API only:
//!
//! * argument parsing, via `clap`'s `try_parse_from` on every command, so a
//!   grammar change breaks here;
//! * the `run_*` command functions against a temporary data directory, so the
//!   vault/profile/import/fingerprint flows are checked without a process, a
//!   terminal or a browser.
//!
//! `launch`/`stop`/`status` need Chrome and are not driven end to end here;
//! only the browser-free paths (registry bookkeeping, not-found errors) are.

use std::path::{Path, PathBuf};

use clap::Parser;
use tempfile::TempDir;

use mirage_cli::cli::{
    Cli, Command, CookieFormat, CookiesCommand, FingerprintCommand, ImportSource, OsArg,
    ProfileCommand, ProxyCommand,
};
use mirage_cli::cli::{ImportArgs, ProfileCreateArgs};
use mirage_cli::commands::fingerprint::run_fingerprint_show;
use mirage_cli::commands::import::run_import;
use mirage_cli::commands::profile::{
    run_profile_create, run_profile_delete, run_profile_list, run_profile_show,
};
use mirage_cli::commands::session::{run_status, run_stop, StopTarget};
use mirage_cli::context::Ctx;
use mirage_cli::proxy_url::parse_proxy_url;

/// A context rooted at a fresh temp dir with a fixed password.
fn temp_ctx(dir: &TempDir) -> Ctx {
    Ctx {
        data_dir: dir.path().to_path_buf(),
        password: Some("integration-password".into()),
        json: false,
    }
}

/// Path to the shared mirage-import AdsPower fixture (reused, never copied).
fn adspower_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("mirage-import")
        .join("tests")
        .join("fixtures")
        .join("adspower_list.json")
}

fn create_args(name: &str) -> ProfileCreateArgs {
    ProfileCreateArgs {
        name: name.into(),
        os: Some(OsArg::Windows),
        seed: Some("20261103".into()),
        tags: vec!["work".into()],
        proxy: None,
    }
}

// --- argument parsing -------------------------------------------------------

#[test]
fn parses_profile_commands() {
    let list = Cli::try_parse_from(["mirage", "profile", "list"]).expect("list");
    assert!(matches!(
        list.command,
        Command::Profile(ProfileCommand::List)
    ));

    let show = Cli::try_parse_from(["mirage", "profile", "show", "abc"]).expect("show");
    match show.command {
        Command::Profile(ProfileCommand::Show(args)) => assert_eq!(args.id, "abc"),
        other => panic!("expected show, got {other:?}"),
    }

    let create = Cli::try_parse_from([
        "mirage",
        "profile",
        "create",
        "--name",
        "Shop-01",
        "--os",
        "macos",
        "--seed",
        "7",
        "--tag",
        "work",
        "--tag",
        "eu",
        "--proxy",
        "socks5://u:p@1.2.3.4:1080",
    ])
    .expect("create");
    match create.command {
        Command::Profile(ProfileCommand::Create(args)) => {
            assert_eq!(args.name, "Shop-01");
            assert_eq!(args.os, Some(OsArg::Macos));
            assert_eq!(args.seed.as_deref(), Some("7"));
            assert_eq!(args.tags, vec!["work".to_string(), "eu".to_string()]);
            assert_eq!(args.proxy.as_deref(), Some("socks5://u:p@1.2.3.4:1080"));
        }
        other => panic!("expected create, got {other:?}"),
    }

    // `--seed` with no value collapses to an empty marker (random seed).
    let create_random =
        Cli::try_parse_from(["mirage", "profile", "create", "--name", "X", "--seed"])
            .expect("create random seed");
    match create_random.command {
        Command::Profile(ProfileCommand::Create(args)) => {
            assert_eq!(args.seed.as_deref(), Some(""));
        }
        other => panic!("expected create, got {other:?}"),
    }

    let delete = Cli::try_parse_from(["mirage", "profile", "delete", "xyz"]).expect("delete");
    match delete.command {
        Command::Profile(ProfileCommand::Delete(args)) => assert_eq!(args.id, "xyz"),
        other => panic!("expected delete, got {other:?}"),
    }

    let duplicate =
        Cli::try_parse_from(["mirage", "profile", "duplicate", "abc", "--name", "Copy"])
            .expect("duplicate");
    match duplicate.command {
        Command::Profile(ProfileCommand::Duplicate(args)) => {
            assert_eq!(args.id, "abc");
            assert_eq!(args.name, "Copy");
        }
        other => panic!("expected duplicate, got {other:?}"),
    }
}

#[test]
fn parses_launch_stop_status() {
    let launch = Cli::try_parse_from(["mirage", "launch", "abc"]).expect("launch");
    match launch.command {
        Command::Launch(args) => assert_eq!(args.id, "abc"),
        other => panic!("expected launch, got {other:?}"),
    }

    let stop_one = Cli::try_parse_from(["mirage", "stop", "abc"]).expect("stop one");
    match stop_one.command {
        Command::Stop(args) => {
            assert_eq!(args.id.as_deref(), Some("abc"));
            assert!(!args.all);
        }
        other => panic!("expected stop, got {other:?}"),
    }

    let stop_all = Cli::try_parse_from(["mirage", "stop", "--all"]).expect("stop all");
    match stop_all.command {
        Command::Stop(args) => {
            assert!(args.id.is_none());
            assert!(args.all);
        }
        other => panic!("expected stop, got {other:?}"),
    }

    let status = Cli::try_parse_from(["mirage", "status"]).expect("status");
    assert!(matches!(status.command, Command::Status));
}

#[test]
fn parses_proxy_check() {
    let parsed = Cli::try_parse_from([
        "mirage",
        "proxy",
        "check",
        "socks5://1.2.3.4:1080",
        "--timeout-ms",
        "5000",
    ])
    .expect("proxy check");
    match parsed.command {
        Command::Proxy(ProxyCommand::Check(args)) => {
            assert_eq!(args.url, "socks5://1.2.3.4:1080");
            assert_eq!(args.timeout_ms, 5000);
        }
        other => panic!("expected proxy check, got {other:?}"),
    }
}

#[test]
fn parses_cookies_commands() {
    let import = Cli::try_parse_from([
        "mirage", "cookies", "import", "abc", "--format", "netscape", "--file", "c.txt",
    ])
    .expect("cookies import");
    match import.command {
        Command::Cookies(CookiesCommand::Import(args)) => {
            assert_eq!(args.id, "abc");
            assert_eq!(args.format, CookieFormat::Netscape);
            assert_eq!(args.file, PathBuf::from("c.txt"));
        }
        other => panic!("expected cookies import, got {other:?}"),
    }

    let export = Cli::try_parse_from([
        "mirage", "cookies", "export", "abc", "--format", "json", "--out", "out.json",
    ])
    .expect("cookies export");
    match export.command {
        Command::Cookies(CookiesCommand::Export(args)) => {
            assert_eq!(args.id, "abc");
            assert_eq!(args.format, CookieFormat::Json);
            assert_eq!(args.out, Some(PathBuf::from("out.json")));
        }
        other => panic!("expected cookies export, got {other:?}"),
    }

    let export_stdout =
        Cli::try_parse_from(["mirage", "cookies", "export", "abc", "--format", "json"])
            .expect("cookies export stdout");
    match export_stdout.command {
        Command::Cookies(CookiesCommand::Export(args)) => assert!(args.out.is_none()),
        other => panic!("expected cookies export, got {other:?}"),
    }
}

#[test]
fn parses_import_and_fingerprint() {
    let import = Cli::try_parse_from([
        "mirage", "import", "--source", "gologin", "--file", "g.json",
    ])
    .expect("import");
    match import.command {
        Command::Import(args) => {
            assert_eq!(args.source, ImportSource::Gologin);
            assert_eq!(args.file, PathBuf::from("g.json"));
            assert!(args.effective_save(), "saving is the default");
        }
        other => panic!("expected import, got {other:?}"),
    }

    let fingerprint =
        Cli::try_parse_from(["mirage", "fingerprint", "show", "--seed", "42"]).expect("fp");
    match fingerprint.command {
        Command::Fingerprint(FingerprintCommand::Show(args)) => {
            assert_eq!(args.seed, 42);
            assert!(args.os.is_none());
        }
        other => panic!("expected fingerprint show, got {other:?}"),
    }
}

#[test]
fn global_args_parse_before_and_after_subcommand() {
    // `--json` and `--data-dir` are global and may follow the subcommand.
    let cli = Cli::try_parse_from([
        "mirage",
        "profile",
        "list",
        "--json",
        "--data-dir",
        "/tmp/mirage",
    ])
    .expect("global args");
    assert!(cli.json);
    assert_eq!(cli.data_dir, PathBuf::from("/tmp/mirage"));

    // Password on the command line.
    let cli = Cli::try_parse_from(["mirage", "--password", "s3cret", "status"]).expect("password");
    assert_eq!(cli.password.as_deref(), Some("s3cret"));
}

#[test]
fn missing_subcommand_is_a_usage_error() {
    assert!(Cli::try_parse_from(["mirage"]).is_err());
    assert!(Cli::try_parse_from(["mirage", "profile"]).is_err());
}

// --- command execution (temp vault) -----------------------------------------

#[test]
fn profile_crud_on_temp_vault() {
    let dir = TempDir::new().expect("tempdir");
    let ctx = temp_ctx(&dir);

    let created = run_profile_create(&ctx, &create_args("Shop-01")).expect("create");
    let id = created.profile.id.clone();

    let list = run_profile_list(&ctx).expect("list");
    assert_eq!(list.profiles.len(), 1);
    assert_eq!(list.profiles[0].id, id);
    assert_eq!(list.profiles[0].name, "Shop-01");
    assert_eq!(list.profiles[0].tags, vec!["work".to_string()]);

    let shown = run_profile_show(&ctx, &id).expect("show");
    assert_eq!(shown.profile, created.profile);

    run_profile_delete(&ctx, &id).expect("delete");
    assert!(run_profile_list(&ctx).expect("list").profiles.is_empty());
}

#[test]
fn import_reuses_the_import_fixture() {
    let dir = TempDir::new().expect("tempdir");
    let ctx = temp_ctx(&dir);

    let args = ImportArgs {
        source: ImportSource::Adspower,
        file: adspower_fixture(),
        save: false,
        no_save: false,
    };
    let report = run_import(&ctx, &args).expect("import");
    assert_eq!(report.profiles.len(), 1);
    assert_eq!(report.profiles[0].name, "Shop-01");
    assert!(report.saved);
    assert!(report.warnings > 0, "the fixture has unmapped fields");

    // The imported profile landed in the vault.
    let list = run_profile_list(&ctx).expect("list");
    assert_eq!(list.profiles.len(), 1);
    assert_eq!(list.profiles[0].id, report.profiles[0].id);
}

#[test]
fn fingerprint_is_deterministic_for_a_fixed_seed() {
    let args = mirage_cli::cli::FingerprintShowArgs {
        seed: 20261103,
        os: Some(OsArg::Linux),
    };
    let first = run_fingerprint_show(&args).expect("first");
    let second = run_fingerprint_show(&args).expect("second");
    assert_eq!(first.fingerprint, second.fingerprint);
    assert_eq!(
        first.fingerprint.canvas_seed,
        second.fingerprint.canvas_seed
    );
}

#[tokio::test]
async fn status_is_empty_on_a_fresh_data_dir() {
    let dir = TempDir::new().expect("tempdir");
    let ctx = temp_ctx(&dir);
    let report = run_status(&ctx).await.expect("status");
    assert!(report.sessions.is_empty());
}

#[tokio::test]
async fn stop_unknown_session_maps_to_not_found() {
    let dir = TempDir::new().expect("tempdir");
    let ctx = temp_ctx(&dir);
    let error = run_stop(
        &ctx,
        &StopTarget::One("00000000-0000-4000-8000-000000000000".into()),
    )
    .await
    .expect_err("unknown session");
    assert_eq!(error.exit_code(), mirage_cli::error::exit::NOT_FOUND);
}

// --- proxy URL conversion ----------------------------------------------------

#[test]
fn proxy_url_conversion() {
    let url = parse_proxy_url("socks5://user:pass@1.2.3.4:1080").expect("valid");
    let core = url.to_core();
    assert_eq!(core.host, "1.2.3.4");
    assert_eq!(core.port, 1080);
    assert_eq!(core.username.as_deref(), Some("user"));
    assert_eq!(core.password.as_deref(), Some("pass"));

    let client = url.to_client();
    assert_eq!(client.endpoint(), "1.2.3.4:1080");

    let http = parse_proxy_url("http://1.2.3.4:8080").expect("http");
    assert_eq!(http.endpoint(), "http://1.2.3.4:8080");
    assert_eq!(http.username, None);

    assert!(parse_proxy_url("garbage").is_err());
}

#[tokio::test]
async fn proxy_check_reports_failure_for_a_dead_proxy() {
    use mirage_cli::cli::ProxyCheckArgs;
    use mirage_cli::commands::proxy::run_proxy_check;

    // 127.0.0.1:1 is a closed port: the check fails fast and must report it
    // rather than error out, so the process can still print the JSON report.
    let args = ProxyCheckArgs {
        url: "socks5://127.0.0.1:1".into(),
        timeout_ms: 2000,
    };
    let report = run_proxy_check(&args).await.expect("check runs");
    assert!(!report.ok);
    assert!(report.ip.is_none());
    assert!(report.error.is_some());
    assert_eq!(report.scheme, "socks5");
}
