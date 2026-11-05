//! HTTP routing and handlers.
//!
//! Authentication is performed by calling [`authenticate`] at the top of each
//! handler rather than through a custom extractor: the `Authorization` header
//! is a `FromRequestParts` extractor and the body is read as raw `Bytes`, which
//! lets a malformed body map to `400` instead of axum's default `422`.
//!
//! Every extractor used here is *infallible* for client input (`RawQuery`,
//! `HeaderMap`, `Path<String>`, `Bytes`). Query strings are parsed by hand
//! after `authenticate`, so a malformed `?limit=` is a clean `401`/`400`
//! envelope instead of an extractor rejection that would bypass auth. A
//! response layer normalizes any remaining non-JSON error into the single
//! `{"error": ...}` envelope.

use axum::body::{Body, Bytes};
use axum::extract::{Path, RawQuery, State};
use axum::http::{header, HeaderMap, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::auth::authenticate;
use crate::db::{now_rfc3339, ProfileWrite};
use crate::error::ApiError;
use crate::models::{
    AuditListQuery, AuditView, CreateKeyRequest, CreateKeyResponse, KeyListQuery, KeyView,
    ProfileListQuery, ProfileSummaryView, ProfileView, PutProfileRequest, PutProfileResponse,
};
use crate::secrets::{generate_key, hash_key};
use crate::AppState;

/// Default and maximum page size for list endpoints.
const DEFAULT_LIMIT: i64 = 100;
const MAX_LIMIT: i64 = 1000;

/// Longest accepted key label. Labels appear in URL paths, so they are also
/// restricted to a conservative ASCII subset.
const LABEL_MAX_LEN: usize = 128;

/// Build the application router with all routes and shared state attached.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/keys", post(create_key).get(list_keys))
        .route("/v1/keys/{label}", delete(revoke_key))
        .route("/v1/profiles", get(list_profiles))
        .route(
            "/v1/profiles/{id}",
            get(get_profile).put(put_profile).delete(delete_profile),
        )
        .route("/v1/audit", get(list_audit))
        .fallback(fallback_not_found)
        .layer(middleware::from_fn(envelope_errors))
        .with_state(state)
}

/// Parse a JSON request body, turning every failure into a `400`.
fn parse_body<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<T, ApiError> {
    serde_json::from_slice(body)
        .map_err(|e| ApiError::BadRequest(format!("invalid JSON body: {e}")))
}

/// Parse a query string, turning every failure into a `400`.
///
/// Runs *after* authentication so an anonymous caller cannot reach the query
/// parser. A missing query string is treated as empty (all defaults).
fn parse_query<T: serde::de::DeserializeOwned>(raw: Option<&str>) -> Result<T, ApiError> {
    serde_urlencoded::from_str(raw.unwrap_or(""))
        .map_err(|e| ApiError::BadRequest(format!("invalid query string: {e}")))
}

/// Clamp a requested page size into `1..=MAX_LIMIT`.
fn clamp_limit(limit: Option<i64>) -> i64 {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

/// Clamp a requested offset to be non-negative.
fn clamp_offset(offset: Option<i64>) -> i64 {
    offset.unwrap_or(0).max(0)
}

/// A label must be non-empty, bounded and URL-safe.
fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= LABEL_MAX_LEN
        && label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Liveness probe; intentionally unauthenticated.
async fn healthz() -> Json<Value> {
    Json(json!({ "ok": true }))
}

/// `POST /v1/keys` (admin): mint a key and return it once.
async fn create_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<CreateKeyResponse>), ApiError> {
    let caller = authenticate(&headers, &state.db)?;
    caller.require_admin()?;

    let req: CreateKeyRequest = parse_body(&body)?;
    if !valid_label(&req.label) {
        return Err(ApiError::BadRequest(
            "label must be 1-128 chars of [A-Za-z0-9._-]".to_owned(),
        ));
    }

    let key = generate_key();
    let now = now_rfc3339();
    state
        .db
        .create_key_audited(&req.label, req.role, &hash_key(&key), &now, &caller.key_id)?;

    Ok((StatusCode::CREATED, Json(CreateKeyResponse { key })))
}

/// `GET /v1/keys?limit=&offset=` (admin): keys, revoked ones included.
async fn list_keys(
    State(state): State<AppState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Result<Json<Vec<KeyView>>, ApiError> {
    let caller = authenticate(&headers, &state.db)?;
    caller.require_admin()?;

    let query: KeyListQuery = parse_query(raw.as_deref())?;
    let keys = state
        .db
        .list_keys(clamp_limit(query.limit), clamp_offset(query.offset))?;
    Ok(Json(keys.into_iter().map(KeyView::from).collect()))
}

/// `DELETE /v1/keys/{label}` (admin): revoke the active key with this label.
async fn revoke_key(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(label): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let caller = authenticate(&headers, &state.db)?;
    caller.require_admin()?;

    let now = now_rfc3339();
    if !state.db.revoke_key_audited(&label, &now, &caller.key_id)? {
        return Err(ApiError::NotFound(format!(
            "no active key with label {label:?}"
        )));
    }

    Ok(Json(json!({ "revoked": label })))
}

/// `GET /v1/profiles?tag=&limit=&offset=` (any role): listing without bodies.
async fn list_profiles(
    State(state): State<AppState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Result<Json<Vec<ProfileSummaryView>>, ApiError> {
    authenticate(&headers, &state.db)?;

    let query: ProfileListQuery = parse_query(raw.as_deref())?;
    let tag = query.tag.as_deref().filter(|t| !t.is_empty());
    let profiles =
        state
            .db
            .list_profiles(tag, clamp_limit(query.limit), clamp_offset(query.offset))?;
    Ok(Json(
        profiles.into_iter().map(ProfileSummaryView::from).collect(),
    ))
}

/// `GET /v1/profiles/{id}` (any role): one full profile.
async fn get_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<ProfileView>, ApiError> {
    authenticate(&headers, &state.db)?;

    match state.db.get_profile(&id)? {
        Some(record) => Ok(Json(ProfileView::from(record))),
        None => Err(ApiError::NotFound(format!("profile {id:?}"))),
    }
}

/// `PUT /v1/profiles/{id}` (any role): create or update under optimistic
/// concurrency. The write and its audit line commit together.
async fn put_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<PutProfileResponse>, ApiError> {
    let caller = authenticate(&headers, &state.db)?;

    if id.is_empty() {
        return Err(ApiError::BadRequest(
            "profile id must not be empty".to_owned(),
        ));
    }
    let req: PutProfileRequest = parse_body(&body)?;

    let now = now_rfc3339();
    let revision = state.db.put_profile_audited(
        ProfileWrite {
            id: &id,
            name: req.name.as_deref(),
            tags: req.tags.as_deref(),
            body: &req.body,
            base_revision: req.base_revision,
            updated_by: &caller.key_id,
            now: &now,
        },
        &caller.key_id,
    )?;

    Ok(Json(PutProfileResponse { id, revision }))
}

/// `DELETE /v1/profiles/{id}` (admin).
async fn delete_profile(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let caller = authenticate(&headers, &state.db)?;
    caller.require_admin()?;

    if !state
        .db
        .delete_profile_audited(&id, &now_rfc3339(), &caller.key_id)?
    {
        return Err(ApiError::NotFound(format!("profile {id:?}")));
    }

    Ok(Json(json!({ "deleted": id })))
}

/// `GET /v1/audit?limit=&offset=` (admin): newest audit entries first.
async fn list_audit(
    State(state): State<AppState>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
) -> Result<Json<Vec<AuditView>>, ApiError> {
    let caller = authenticate(&headers, &state.db)?;
    caller.require_admin()?;

    let query: AuditListQuery = parse_query(raw.as_deref())?;
    let entries = state
        .db
        .list_audit(clamp_limit(query.limit), clamp_offset(query.offset))?;
    Ok(Json(entries.into_iter().map(AuditView::from).collect()))
}

/// Fallback for unmatched routes: same envelope as every other error.
async fn fallback_not_found() -> ApiError {
    ApiError::NotFound("no such route".to_owned())
}

/// Normalize any error response that is not already the JSON envelope.
///
/// Our own [`ApiError`] responses are already JSON and pass through untouched.
/// This catches framework-produced errors (extractor rejections, `405`, `404`)
/// so a client never sees an HTML or text error body.
async fn envelope_errors(request: Request<Body>, next: Next) -> Response {
    let response = next.run(request).await;
    let status = response.status();
    if !(status.is_client_error() || status.is_server_error()) {
        return response;
    }

    let is_json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("application/json")
        })
        .unwrap_or(false);
    if is_json {
        return response;
    }

    let message = status.canonical_reason().unwrap_or("request failed");
    (status, Json(json!({ "error": message }))).into_response()
}
