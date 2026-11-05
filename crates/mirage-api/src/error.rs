//! Native API error type mapped onto the frozen error envelope from
//! `docs/API.md`:
//!
//! ```json
//! { "error": { "code": "<code>", "message": "<text>" } }
//! ```
//!
//! The code vocabulary is fixed by the contract:
//! `invalid_request`, `not_found`, `already_running`, `not_running`,
//! `vault_locked`, `io`, `internal`.

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// Everything a native handler can fail with.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// Malformed request or a body that violates the fingerprint contract.
    #[error("{0}")]
    InvalidRequest(String),

    /// No profile (or resource) with the requested id.
    #[error("{0}")]
    NotFound(String),

    /// The profile is already running (launch is not idempotent).
    #[error("{0}")]
    AlreadyRunning(String),

    /// `stop` was called for a profile that is not running.
    #[error("{0}")]
    NotRunning(String),

    /// The vault is locked: profile endpoints require an unlocked vault.
    #[error("vault is locked; unlock it with the master password first")]
    VaultLocked,

    /// A wrong master password was supplied to `POST /vault/unlock`.
    #[error("invalid vault password")]
    UnlockDenied,

    /// Missing or unusable on-disk resource (e.g. a cookie store for a
    /// profile that never launched).
    #[error("{0}")]
    Io(String),

    /// Unexpected internal failure.
    #[error("{0}")]
    Internal(String),
}

impl ApiError {
    /// The frozen error `code` for this failure.
    pub fn code(&self) -> &'static str {
        match self {
            ApiError::InvalidRequest(_) => "invalid_request",
            ApiError::NotFound(_) => "not_found",
            ApiError::AlreadyRunning(_) => "already_running",
            ApiError::NotRunning(_) => "not_running",
            ApiError::VaultLocked => "vault_locked",
            ApiError::UnlockDenied => "vault_locked",
            ApiError::Io(_) => "io",
            ApiError::Internal(_) => "internal",
        }
    }

    /// HTTP status for this failure.
    pub fn status(&self) -> StatusCode {
        match self {
            ApiError::InvalidRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::AlreadyRunning(_) | ApiError::NotRunning(_) => StatusCode::CONFLICT,
            ApiError::VaultLocked => StatusCode::UNAUTHORIZED,
            ApiError::UnlockDenied => StatusCode::UNAUTHORIZED,
            // `io` marks a client-visible missing on-disk resource (a profile
            // that never launched); unexpected storage failures use `internal`.
            ApiError::Io(_) => StatusCode::BAD_REQUEST,
            ApiError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// The contract envelope for a code + message at a fixed status.
pub fn error_response(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({ "error": { "code": code, "message": message } })),
    )
        .into_response()
}

/// Rewrite a bare `413 Payload Too Large` (raised by the body-size limit
/// extractor, which is not contract-shaped) into the native error envelope.
///
/// Any other status is passed through untouched, so this layer only ever
/// normalizes the payload-too-large case.
pub async fn contract_body_limit(request: Request, next: Next) -> Response {
    let response = next.run(request).await;
    if response.status() != StatusCode::PAYLOAD_TOO_LARGE {
        return response;
    }
    error_response(
        StatusCode::PAYLOAD_TOO_LARGE,
        "invalid_request",
        "request body exceeds the configured size limit",
    )
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        error_response(self.status(), self.code(), &self.message_text())
    }
}

impl ApiError {
    /// The human-readable message for this error.
    fn message_text(&self) -> String {
        self.to_string()
    }
}

/// A 401 for a missing or wrong Bearer token. The contract has no dedicated
/// auth code, so the closest frozen code (`invalid_request`) is used and the
/// status carries the meaning.
pub fn token_unauthorized() -> Response {
    error_response(
        StatusCode::UNAUTHORIZED,
        "invalid_request",
        "missing or invalid API token",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_and_statuses_match_the_contract() {
        assert_eq!(
            ApiError::InvalidRequest(String::new()).code(),
            "invalid_request"
        );
        assert_eq!(ApiError::NotFound(String::new()).code(), "not_found");
        assert_eq!(
            ApiError::AlreadyRunning(String::new()).code(),
            "already_running"
        );
        assert_eq!(ApiError::NotRunning(String::new()).code(), "not_running");
        assert_eq!(ApiError::VaultLocked.code(), "vault_locked");
        assert_eq!(ApiError::Io(String::new()).code(), "io");
        assert_eq!(ApiError::Internal(String::new()).code(), "internal");

        assert_eq!(ApiError::VaultLocked.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            ApiError::NotFound(String::new()).status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            ApiError::AlreadyRunning(String::new()).status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            ApiError::NotRunning(String::new()).status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            ApiError::InvalidRequest(String::new()).status(),
            StatusCode::BAD_REQUEST
        );
    }
}
