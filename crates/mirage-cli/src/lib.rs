//! Mirage command-line interface.
//!
//! Drives the Mirage crates directly, with no background server:
//! [`mirage_vault`] for encrypted profiles, [`mirage_core`] for
//! fingerprints, [`mirage_launcher`] for masked browser sessions,
//! [`mirage_proxy`] for exit-IP checks, [`mirage_cookies`] for cookie
//! import/export and [`mirage_import`] for competitor migrations.
//!
//! There is no long-running daemon, but the CLI is not memoryless: a foreground
//! `mirage launch` records a running-session descriptor under `<data-dir>/run/`
//! so later, separate `mirage status` / `mirage stop` processes can act on it
//! (see [`session_store`]).
//!
//! Structure: [`cli`] holds the clap grammar; [`commands`] holds the `run_*`
//! functions, each taking parsed arguments plus a [`context::Ctx`] and
//! returning a [`output::Render`] value. [`run`] dispatches and prints, so the
//! grammar and the commands are both testable without spawning a process.
//!
//! Exit codes are documented in `README.md` and defined in [`error::exit`].
#![forbid(unsafe_code)]

pub mod cli;
pub mod commands;
pub mod context;
pub mod error;
pub mod output;
pub mod path;
pub mod proxy_url;
pub mod session_store;

pub use cli::{Cli, Command};
pub use context::Ctx;
pub use error::CliError;

use std::io::Write;
use std::process::ExitCode;

use cli::{CookiesCommand, FingerprintCommand, ProfileCommand, ProxyCommand};
use commands::{cookies, fingerprint, import, profile, proxy, session};
use error::exit;
use output::{emit, Render};

/// Parse-and-run entry point used by the `mirage` binary: dispatches the
/// parsed command, prints its result and maps errors to exit codes.
pub async fn run(cli: Cli) -> ExitCode {
    match dispatch(&cli).await {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(error.exit_code())
        }
    }
}

/// Execute the parsed command and print its result.
async fn dispatch(cli: &Cli) -> Result<ExitCode, CliError> {
    let ctx = Ctx {
        data_dir: cli.data_dir.clone(),
        password: cli.password.clone(),
        json: cli.json,
    };
    let json = cli.json;

    match &cli.command {
        Command::Profile(command) => dispatch_profile(&ctx, command, json)?,
        Command::Launch(args) => {
            let live = session::launch_session(&ctx, &args.id).await?;
            print_result(&session::LaunchInfo::from_record(&live.record), json)?;
            let stopped = session::wait_and_stop(&ctx, live).await?;
            print_result(&stopped, json)?;
        }
        Command::Stop(args) => {
            // clap's ArgGroup on StopArgs makes one of these mandatory, so the
            // last arm is defence in depth rather than a reachable path.
            let target = match (&args.id, args.all) {
                (Some(id), _) => session::StopTarget::One(id.clone()),
                (None, true) => session::StopTarget::All,
                (None, false) => {
                    return Err(CliError::Message(
                        "stop needs a profile id or --all".to_string(),
                    ))
                }
            };
            let report = session::run_stop(&ctx, &target).await?;
            let any_failed = !report.failed.is_empty();
            print_result(&report, json)?;
            if any_failed {
                // A stop that could not close a still-running browser is a
                // failure: report it while still printing which sessions stopped.
                return Ok(ExitCode::from(exit::GENERAL));
            }
        }
        Command::Status => print_result(&session::run_status(&ctx).await?, json)?,
        Command::Proxy(command) => return dispatch_proxy(command, json).await,
        Command::Cookies(command) => dispatch_cookies(&ctx, command, json)?,
        Command::Import(args) => print_result(&import::run_import(&ctx, args)?, json)?,
        Command::Fingerprint(command) => match command {
            FingerprintCommand::Show(args) => {
                print_result(&fingerprint::run_fingerprint_show(args)?, json)?
            }
        },
    }

    Ok(ExitCode::from(exit::OK))
}

/// Profile subcommands.
fn dispatch_profile(ctx: &Ctx, command: &ProfileCommand, json: bool) -> Result<(), CliError> {
    match command {
        ProfileCommand::List => print_result(&profile::run_profile_list(ctx)?, json),
        ProfileCommand::Show(args) => {
            print_result(&profile::run_profile_show(ctx, &args.id)?, json)
        }
        ProfileCommand::Create(args) => {
            print_result(&profile::run_profile_create(ctx, args)?, json)
        }
        ProfileCommand::Delete(args) => {
            print_result(&profile::run_profile_delete(ctx, &args.id)?, json)
        }
        ProfileCommand::Duplicate(args) => {
            print_result(&profile::run_profile_duplicate(ctx, args)?, json)
        }
    }
}

/// Proxy subcommands. `proxy check` maps a failed check to a non-zero exit
/// code while still printing the report.
async fn dispatch_proxy(command: &ProxyCommand, json: bool) -> Result<ExitCode, CliError> {
    match command {
        ProxyCommand::Check(args) => {
            let report = proxy::run_proxy_check(args).await?;
            let ok = report.ok;
            print_result(&report, json)?;
            Ok(if ok {
                ExitCode::from(exit::OK)
            } else {
                ExitCode::from(exit::PROXY)
            })
        }
    }
}

/// Cookie subcommands. Export without `--out` writes the raw payload to
/// stdout (both formats), which is the machine-friendly contract.
fn dispatch_cookies(ctx: &Ctx, command: &CookiesCommand, json: bool) -> Result<(), CliError> {
    match command {
        CookiesCommand::Import(args) => {
            let report = cookies::run_cookies_import(ctx, &args.id, args.format, &args.file)?;
            print_result(&report, json)
        }
        CookiesCommand::Export(args) => {
            let data = cookies::run_cookies_export(ctx, &args.id, args.format)?;
            match &args.out {
                Some(path) => {
                    std::fs::write(path, data.as_bytes())?;
                    let info = cookies::CookieExportInfo {
                        profile_id: args.id.clone(),
                        format: args.format.as_str().to_string(),
                        out: path.display().to_string(),
                        bytes: data.len(),
                    };
                    print_result(&info, json)
                }
                None => write_raw(&data),
            }
        }
    }
}

/// Print one rendered value to stdout.
fn print_result<T: Render>(value: &T, json: bool) -> Result<(), CliError> {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    emit(value, json, &mut lock)?;
    lock.flush()?;
    Ok(())
}

/// Write a raw payload (cookie export) to stdout, ensuring a trailing newline.
fn write_raw(text: &str) -> Result<(), CliError> {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    lock.write_all(text.as_bytes())?;
    if !text.ends_with('\n') {
        lock.write_all(b"\n")?;
    }
    lock.flush()?;
    Ok(())
}
