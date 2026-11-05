//! Mirage local REST API.
//!
//! A single HTTP surface over the whole Mirage core: profile CRUD, fingerprint
//! generation, the encrypted vault, browser launch/stop, proxy checks, cookie
//! and competitor import — plus an AdsPower-compatible alias layer so their
//! scripts move over without a rewrite.
//!
//! The wire contract is frozen in `docs/API.md`; this crate implements it
//! verbatim. Module map:
//! - [`config`] — environment configuration and data directories;
//! - [`state`] — [`AppState`]: directories, the session manager, the vault;
//! - [`routes`] — native `/api/v1/*` handlers;
//! - [`adspower`] — AdsPower aliases and their always-200 envelope;
//! - [`cors`] / [`auth`] — the CORS layer and the Bearer guard;
//! - [`error`] — the native error envelope and its code vocabulary.

pub mod adspower;
pub mod auth;
pub mod config;
pub mod cors;
pub mod error;
pub mod models;
pub mod routes;
pub mod state;

pub use config::{Config, DEFAULT_PORT};
pub use error::ApiError;
pub use state::AppState;

use axum::extract::DefaultBodyLimit;
use axum::middleware;
use axum::Router;

/// Maximum request body accepted by any endpoint. Larger bodies are rejected
/// with a contract-shaped `413` (see [`error::contract_body_limit`]).
const DEFAULT_BODY_LIMIT: usize = 2 * 1024 * 1024;

/// Build the full router: native routes + AdsPower aliases, then the CORS and
/// token layers, then the shared state.
///
/// Layer order puts CORS outermost so preflights are answered before auth and
/// every response — errors included — carries the CORS headers. The body-limit
/// contract layer sits just inside CORS so it can rewrite an over-limit `413`
/// into the native envelope before CORS stamps the response.
pub fn router(state: AppState) -> Router {
    let native = routes::native_router();
    let aliases = adspower::adspower_router();

    native
        .merge(aliases)
        .layer(DefaultBodyLimit::max(DEFAULT_BODY_LIMIT))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_token,
        ))
        .layer(middleware::from_fn(error::contract_body_limit))
        .layer(middleware::from_fn(cors::cors_middleware))
        .with_state(state)
}
