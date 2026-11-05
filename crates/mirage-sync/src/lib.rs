//! Mirage sync server: self-hosted profile synchronization for Mirage profiles.
//!
//! The server keeps team profiles in a single SQLite file and protects them
//! with API keys (no users, no passwords — a key *is* the subject). A profile
//! body is treated as opaque JSON: this crate does not depend on `mirage-core`
//! and never validates the fingerprint shape; the client does that before it
//! pushes a profile.
//!
//! Layers:
//! - [`db`] — SQLite storage behind a mutex, migrations and all queries;
//! - [`auth`] — Bearer key authentication and the `admin`/`member` roles;
//! - [`routes`] — the axum router and handlers;
//! - [`error`] — the API error type mapped onto HTTP status + JSON bodies.

pub mod auth;
pub mod crypto;
pub mod db;
pub mod error;
pub mod models;
pub mod routes;
pub mod secrets;

use std::net::SocketAddr;
use std::sync::Arc;

pub use auth::{authenticate, KeyInfo, Role};
pub use crypto::{BodyCipher, KeyConfig};
pub use db::{now_rfc3339, Db, DbConfig, DbError};
pub use error::ApiError;

/// Shared application state: one database, shared by every handler.
#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Db>,
}

impl AppState {
    pub fn new(db: Db) -> Self {
        Self { db: Arc::new(db) }
    }
}

/// Refuse to bind a non-loopback address unless the operator opted in.
///
/// The API authenticates with a Bearer key and, on the wire, speaks cleartext
/// HTTP; exposing it on a network interface without TLS would send keys and
/// profile bodies (including proxy credentials) in the clear. Returns an error
/// (not a panic) so `main` can print a precise message.
pub fn ensure_bind_allowed(addr: SocketAddr, allow_remote: bool) -> Result<(), String> {
    if addr.ip().is_loopback() || allow_remote {
        Ok(())
    } else {
        Err(format!(
            "refusing to bind non-loopback address {addr}: set MIRAGE_SYNC_ALLOW_REMOTE=1 \
             to override, and terminate TLS in front of the server — the API speaks cleartext"
        ))
    }
}

/// Build the full router with all routes and shared state attached.
pub fn router(state: AppState) -> axum::Router {
    routes::router(state)
}
