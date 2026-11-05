//! CORS middleware.
//!
//! The UI runs inside a Tauri webview and calls this server cross-origin, so a
//! response carries `Access-Control-Allow-Origin` only when the request's
//! `Origin` is allowlisted. The allowlist defaults to the Tauri webview origins
//! and is overridable with `MIRAGE_ALLOWED_ORIGINS` (comma-separated).
//!
//! Decision (see [`classify_origin`]):
//! - `Origin` present and allowlisted → mirror it back into `ACAO`;
//! - `Origin` present but not allowlisted → send no `ACAO` (the browser then
//!   blocks the response) and answer a preflight `OPTIONS` with `403`;
//! - no `Origin` (curl, SDK, server-to-server) → no `ACAO` is needed.
//!
//! `Access-Control-Allow-Methods` / `Access-Control-Allow-Headers` are always
//! sent so allowlisted preflights succeed.

use std::collections::HashSet;
use std::sync::OnceLock;

use axum::extract::Request;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Request headers UI/SDK clients may send.
const ALLOW_HEADERS: &str = "authorization, content-type";
/// Methods allowed by the frozen contract.
const ALLOW_METHODS: &str = "GET, POST, PUT, DELETE, OPTIONS";

/// Origins allowed when `MIRAGE_ALLOWED_ORIGINS` is not set: the Tauri webview
/// origins on every platform (`http` on Windows/Android, `https` on macOS/iOS,
/// `tauri://` on Linux).
const DEFAULT_ALLOWED_ORIGINS: &[&str] = &[
    "http://tauri.localhost",
    "https://tauri.localhost",
    "tauri://localhost",
];

/// How a request's `Origin` maps onto an `ACAO` decision.
#[derive(Debug, PartialEq, Eq)]
enum OriginDecision {
    /// No `Origin` header: no `ACAO` is required.
    Absent,
    /// The origin is allowlisted: mirror exactly this value into `ACAO`.
    Allowed(HeaderValue),
    /// The origin is present but not allowlisted: send no `ACAO`.
    Rejected,
}

/// Apply CORS to every response, short-circuiting preflights.
pub async fn cors_middleware(request: Request, next: Next) -> Response {
    let decision = classify_origin(request.headers().get(header::ORIGIN), allowed_origins());

    if request.method() == Method::OPTIONS {
        // Reply to the preflight without ever routing it: OPTIONS is not a
        // handler method anywhere in the API. A non-allowlisted origin is
        // refused outright, without an `ACAO`, so the browser never sends the
        // real request.
        let status = match decision {
            OriginDecision::Rejected => StatusCode::FORBIDDEN,
            OriginDecision::Absent | OriginDecision::Allowed(_) => StatusCode::NO_CONTENT,
        };
        let mut response = status.into_response();
        apply_cors_headers(response.headers_mut(), &decision);
        return response;
    }

    let mut response = next.run(request).await;
    apply_cors_headers(response.headers_mut(), &decision);
    response
}

/// Decide the `ACAO` outcome for a request `Origin` against the allowlist.
fn classify_origin(origin: Option<&HeaderValue>, allowed: &HashSet<String>) -> OriginDecision {
    match origin {
        None => OriginDecision::Absent,
        // A non-UTF-8 origin can never be an allowlisted match: reject it.
        Some(value) => match value.to_str() {
            Ok(text) if allowed.contains(text) => OriginDecision::Allowed(value.clone()),
            _ => OriginDecision::Rejected,
        },
    }
}

/// Set the CORS headers on a response. `ACAO` is written only for an
/// allowlisted origin; the method/header lists are always written.
fn apply_cors_headers(headers: &mut HeaderMap, decision: &OriginDecision) {
    if let OriginDecision::Allowed(origin) = decision {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin.clone());
    }
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static(ALLOW_HEADERS),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static(ALLOW_METHODS),
    );
}

/// The process allowlist, resolved once from `MIRAGE_ALLOWED_ORIGINS`
/// (comma-separated). An unset or empty variable selects the Tauri defaults.
fn allowed_origins() -> &'static HashSet<String> {
    static ORIGINS: OnceLock<HashSet<String>> = OnceLock::new();
    ORIGINS.get_or_init(|| {
        std::env::var("MIRAGE_ALLOWED_ORIGINS")
            .ok()
            .map(|value| parse_origins(&value))
            .filter(|origins| !origins.is_empty())
            .unwrap_or_else(|| {
                DEFAULT_ALLOWED_ORIGINS
                    .iter()
                    .map(|origin| (*origin).to_string())
                    .collect()
            })
    })
}

/// Split a comma-separated origin list, trimming blanks.
fn parse_origins(value: &str) -> HashSet<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|origin| !origin.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origins(values: &[&str]) -> HashSet<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn parse_origins_trims_and_drops_blanks() {
        let parsed = parse_origins(" http://a , https://b ,, tauri://c ");
        assert_eq!(parsed, origins(&["http://a", "https://b", "tauri://c"]));
        assert!(parse_origins("   ").is_empty());
        assert!(parse_origins("").is_empty());
    }

    #[test]
    fn classify_origin_allowlists_rejects_and_passes_through_absent() {
        let allowed = origins(&["http://tauri.localhost", "https://tauri.localhost"]);

        assert_eq!(classify_origin(None, &allowed), OriginDecision::Absent);

        let good = HeaderValue::from_static("http://tauri.localhost");
        assert_eq!(
            classify_origin(Some(&good), &allowed),
            OriginDecision::Allowed(good.clone())
        );

        let evil = HeaderValue::from_static("http://evil.example");
        assert_eq!(
            classify_origin(Some(&evil), &allowed),
            OriginDecision::Rejected
        );
    }

    #[test]
    fn apply_cors_headers_sets_acao_only_when_allowed() {
        let mut rejected = HeaderMap::new();
        apply_cors_headers(&mut rejected, &OriginDecision::Rejected);
        assert!(!rejected.contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));
        assert!(rejected.contains_key(header::ACCESS_CONTROL_ALLOW_HEADERS));
        assert!(rejected.contains_key(header::ACCESS_CONTROL_ALLOW_METHODS));

        let mut empty = HeaderMap::new();
        apply_cors_headers(&mut empty, &OriginDecision::Absent);
        assert!(!empty.contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));

        let origin = HeaderValue::from_static("https://tauri.localhost");
        let mut allowed = HeaderMap::new();
        apply_cors_headers(&mut allowed, &OriginDecision::Allowed(origin.clone()));
        assert_eq!(
            allowed.get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&origin)
        );
    }

    #[test]
    fn default_allowlist_is_the_tauri_origins() {
        let defaults = origins(DEFAULT_ALLOWED_ORIGINS);
        assert!(defaults.contains("http://tauri.localhost"));
        assert!(defaults.contains("https://tauri.localhost"));
        assert!(defaults.contains("tauri://localhost"));
        assert!(!defaults.contains("*"));
    }
}
