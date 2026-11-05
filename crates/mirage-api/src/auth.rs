//! Bearer-token authentication middleware.
//!
//! When `MIRAGE_API_TOKEN` is set, every `/api/v1/*` request except
//! `/api/v1/health` must carry a matching `Authorization: Bearer <token>`.
//! When it is unset the server trusts localhost and performs no check.

use axum::extract::{Request, State};
use axum::http::header;
use axum::middleware::Next;
use axum::response::Response;

use crate::error::token_unauthorized;
use crate::AppState;

/// The only path exempt from the token check.
const HEALTH_PATH: &str = "/api/v1/health";

/// Reject requests without a valid token when one is configured.
pub async fn require_token(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if let Some(expected) = state.token() {
        let path = request.uri().path();
        let guarded = path.starts_with("/api/v1/") && path != HEALTH_PATH;

        if guarded && !authorized(request.headers(), expected) {
            return token_unauthorized();
        }
    }

    next.run(request).await
}

/// True when the `Authorization` header carries `Bearer <expected>`.
fn authorized(headers: &axum::http::HeaderMap, expected: &str) -> bool {
    let Some(value) = headers.get(header::AUTHORIZATION) else {
        return false;
    };
    let Ok(value) = value.to_str() else {
        return false;
    };
    let Some((scheme, token)) = value.split_once(' ') else {
        return false;
    };
    scheme.eq_ignore_ascii_case("bearer") && constant_time_eq(token.trim(), expected)
}

/// Constant-time byte comparison so token checks do not leak length/prefix
/// timing. Length is compared first (unavoidable leak, harmless here).
fn constant_time_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers_with(value: &str) -> axum::http::HeaderMap {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(value).expect("valid header"),
        );
        headers
    }

    #[test]
    fn accepts_matching_bearer_any_case_scheme() {
        assert!(authorized(&headers_with("Bearer s3cret"), "s3cret"));
        assert!(authorized(&headers_with("bearer s3cret"), "s3cret"));
        assert!(authorized(&headers_with("BEARER s3cret"), "s3cret"));
    }

    #[test]
    fn rejects_bad_or_missing_tokens() {
        assert!(!authorized(&headers_with("Bearer nope"), "s3cret"));
        assert!(!authorized(&headers_with("s3cret"), "s3cret"));
        assert!(!authorized(&headers_with("Basic s3cret"), "s3cret"));
        assert!(!authorized(&axum::http::HeaderMap::new(), "s3cret"));
    }

    #[test]
    fn constant_time_eq_basics() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
    }
}
