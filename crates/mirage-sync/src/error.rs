//! API error type mapped onto HTTP status codes with JSON bodies.
//!
//! Every error the service can produce reaches the wire as
//! `{"error": "<human readable>"}`; the revision conflict additionally carries
//! `current_revision` so clients can rebase and retry.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use crate::db::DbError;

/// Everything a handler can fail with.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("unauthorized: {0}")]
    Unauthorized(&'static str),

    #[error("forbidden: admin role required for this operation")]
    Forbidden,

    #[error("not found: {0}")]
    NotFound(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("profile revision conflict: current revision is {current_revision}")]
    RevisionConflict { current_revision: i64 },

    /// Internal failure. The carried detail is logged but never sent to the
    /// client — internals do not belong in responses.
    #[error("internal error: {0}")]
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self {
            ApiError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            ApiError::Forbidden => StatusCode::FORBIDDEN,
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Conflict(_) | ApiError::RevisionConflict { .. } => StatusCode::CONFLICT,
            ApiError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };

        // 4xx are routine client mistakes and stay at debug level; 5xx means
        // the operator needs the full story in the log.
        if status.is_server_error() {
            tracing::error!(error = %self, "request failed");
        } else {
            tracing::debug!(error = %self, "request rejected");
        }

        let body = match &self {
            ApiError::Internal(_) => json!({ "error": "internal error" }),
            ApiError::RevisionConflict { current_revision } => json!({
                "error": self.to_string(),
                "current_revision": current_revision,
            }),
            _ => json!({ "error": self.to_string() }),
        };

        (status, Json(body)).into_response()
    }
}

impl From<DbError> for ApiError {
    fn from(err: DbError) -> Self {
        match err {
            DbError::LabelTaken => ApiError::Conflict("key label already in use".to_owned()),
            DbError::RevisionConflict { current_revision } => {
                ApiError::RevisionConflict { current_revision }
            }
            // Storage corruption and I/O failures are logged with detail and
            // answered with a generic 500.
            DbError::CorruptRole(label) => {
                tracing::error!(label = %label, "api_keys row has a role outside the CHECK constraint");
                ApiError::Internal("database error".to_owned())
            }
            DbError::Config(detail) => {
                tracing::error!(detail = %detail, "invalid storage configuration");
                ApiError::Internal("database error".to_owned())
            }
            DbError::Crypto(err) => {
                tracing::error!(error = %err, "body crypto error");
                ApiError::Internal("database error".to_owned())
            }
            DbError::Internal(detail) => {
                tracing::error!(detail = %detail, "storage layer internal error");
                ApiError::Internal("database error".to_owned())
            }
            DbError::Sqlite(err) => {
                tracing::error!(error = %err, "sqlite error");
                ApiError::Internal("database error".to_owned())
            }
            DbError::Io(err) => {
                tracing::error!(error = %err, "io error");
                ApiError::Internal("database error".to_owned())
            }
        }
    }
}
