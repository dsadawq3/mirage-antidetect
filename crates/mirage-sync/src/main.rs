//! `mirage-sync` server entry point.
//!
//! Configuration is environment-only:
//! - `MIRAGE_SYNC_DB`              — SQLite file path (default `mirage-sync.db`);
//! - `MIRAGE_SYNC_ADDR`            — listen address (default `127.0.0.1:8787`);
//! - `MIRAGE_SYNC_BOOTSTRAP_KEY`   — admin key to install on a brand-new
//!   database (never logged); when unset a key is generated and printed to
//!   **stderr** exactly once;
//! - `MIRAGE_SYNC_KEY`             — encrypt profile bodies at rest: a
//!   64-hex value is the raw key, anything else is a passphrase stretched
//!   with Argon2id; unset means plaintext bodies (a warning is printed);
//! - `MIRAGE_SYNC_ALLOW_REMOTE=1`  — allow binding a non-loopback address.
//!
//! The server speaks cleartext HTTP, so a non-loopback bind is refused unless
//! explicitly allowed; terminate TLS in front of it.

use std::net::SocketAddr;

use anyhow::Context;
use mirage_sync::{ensure_bind_allowed, router, AppState, Db};

const DEFAULT_DB: &str = "mirage-sync.db";
const DEFAULT_ADDR: &str = "127.0.0.1:8787";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let db_path = std::env::var("MIRAGE_SYNC_DB").unwrap_or_else(|_| DEFAULT_DB.to_owned());
    let addr_raw = std::env::var("MIRAGE_SYNC_ADDR").unwrap_or_else(|_| DEFAULT_ADDR.to_owned());
    let addr: SocketAddr = addr_raw
        .parse()
        .with_context(|| format!("MIRAGE_SYNC_ADDR is not a valid socket address: {addr_raw}"))?;

    // Refuse to expose the cleartext API on a network interface by accident.
    let allow_remote = std::env::var("MIRAGE_SYNC_ALLOW_REMOTE").as_deref() == Ok("1");
    ensure_bind_allowed(addr, allow_remote).map_err(|e| anyhow::anyhow!(e))?;
    if !addr.ip().is_loopback() {
        tracing::warn!(
            address = %addr,
            "binding a non-loopback address without TLS: the Bearer keys and profile bodies travel the network in cleartext"
        );
    }

    let db = Db::open(&db_path).with_context(|| format!("opening SQLite database at {db_path}"))?;

    if !db.is_encrypted() {
        tracing::warn!(
            database = %db_path,
            "MIRAGE_SYNC_KEY is not set: profile bodies are stored UNENCRYPTED; anyone \
             who reads the database file sees proxy credentials. Set MIRAGE_SYNC_KEY to \
             encrypt bodies at rest, or protect the file with disk/file permissions."
        );
    }

    // A provided bootstrap key is used verbatim and never echoed; only a
    // generated one is printed, and only to stderr so it stays out of any
    // stdout log pipeline.
    let provided = std::env::var("MIRAGE_SYNC_BOOTSTRAP_KEY")
        .ok()
        .filter(|key| !key.is_empty());
    if provided.is_some() {
        tracing::info!(
            "MIRAGE_SYNC_BOOTSTRAP_KEY is set; it will be used if the key table is empty (value not logged)"
        );
    }

    if let Some(key) = db
        .ensure_bootstrap(provided.as_deref())
        .context("bootstrapping the initial admin key")?
    {
        eprintln!(
            "\n==================================================================\n\
             \x20 MIRAGE-SYNC BOOTSTRAP ADMIN KEY (printed once, copy it now)\n\
             \x20 {key}\n\
             \x20 This is the only time it is shown; only its sha256 is stored.\n\
             ==================================================================\n"
        );
    }

    let state = AppState::new(db);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;

    tracing::info!(address = %addr, database = %db_path, "mirage-sync listening");
    axum::serve(listener, router(state))
        .await
        .context("HTTP server terminated with an error")?;
    Ok(())
}
