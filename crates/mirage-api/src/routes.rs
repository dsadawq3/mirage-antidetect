//! Native `/api/v1/*` routing and handlers (see `docs/API.md`).

use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use axum::body::Bytes;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::SecondsFormat;
use serde_json::{json, Value};

use mirage_cookies::CookieError;
use mirage_core::{generate, validate, Fingerprint, GenerateError, Profile};
use mirage_import::{ImportError, ImportResult};
use mirage_launcher::AlreadyRunning;
use mirage_proxy::check::check_exit_ip;
use mirage_proxy::{ProxyConfig as ProxyCfg, ProxyScheme};

use crate::error::ApiError;
use crate::models::{
    apply_overrides, now_rfc3339, warnings_to_json, CookiesExportQuery, CookiesExportResponse,
    CookiesImportRequest, CookiesImportResponse, CreateProfileRequest, ImportRequest,
    ImportResponse, ProfileSummary, ProxyCheckRequest, UnlockRequest, UpdateProfileRequest,
};
use crate::state::{vault_error, AppState, UnlockError};

/// Budget for a proxy exit-IP check.
const PROXY_CHECK_TIMEOUT: Duration = Duration::from_secs(15);

/// Build the native router (state not yet attached).
pub fn native_router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/vault/unlock", post(vault_unlock))
        .route("/api/v1/profiles", get(list_profiles).post(create_profile))
        .route(
            "/api/v1/profiles/{id}",
            get(get_profile).put(update_profile).delete(delete_profile),
        )
        .route("/api/v1/profiles/{id}/launch", post(launch_profile))
        .route("/api/v1/profiles/{id}/stop", post(stop_profile))
        .route("/api/v1/status", get(status))
        .route("/api/v1/proxy/check", post(proxy_check))
        .route("/api/v1/profiles/{id}/cookies/import", post(cookies_import))
        .route("/api/v1/profiles/{id}/cookies/export", get(cookies_export))
        .route("/api/v1/import", post(import_profiles))
}

/// Parse a JSON request body, turning any failure into a `400`.
fn parse_body<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<T, ApiError> {
    serde_json::from_slice(body)
        .map_err(|error| ApiError::InvalidRequest(format!("invalid JSON body: {error}")))
}

/// `generate` can only fail if the embedded fp-db is broken: an internal bug.
fn generate_error(error: GenerateError) -> ApiError {
    ApiError::Internal(format!("fingerprint generation failed: {error}"))
}

/// Reject a fingerprint that violates the CONTRACT consistency rules.
fn ensure_consistent(fingerprint: &Fingerprint) -> Result<(), ApiError> {
    let warnings = validate(fingerprint);
    if warnings.is_empty() {
        return Ok(());
    }
    let detail = warnings
        .iter()
        .map(|warning| format!("{warning:?}"))
        .collect::<Vec<_>>()
        .join("; ");
    Err(ApiError::InvalidRequest(format!(
        "fingerprint is inconsistent: {detail}"
    )))
}

/// `GET /api/v1/health` — unauthenticated liveness + vault/session snapshot.
async fn health(State(state): State<AppState>) -> Json<Value> {
    let running = state.session().running().await.len();
    Json(json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "vault": if state.is_unlocked() { "unlocked" } else { "locked" },
        "running": running,
    }))
}

/// `POST /api/v1/vault/unlock`.
async fn vault_unlock(State(state): State<AppState>, body: Bytes) -> Result<Json<Value>, ApiError> {
    let request: UnlockRequest = parse_body(&body)?;
    match state.unlock(&request.password) {
        Ok(()) => Ok(Json(json!({}))),
        Err(UnlockError::WrongPassword) => Err(ApiError::UnlockDenied),
        Err(UnlockError::Io(detail)) => Err(ApiError::Internal(detail)),
    }
}

/// `GET /api/v1/profiles`.
async fn list_profiles(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let vault = state.require_vault()?;
    let running: std::collections::HashSet<String> = state
        .session()
        .running()
        .await
        .into_iter()
        .map(|session| session.profile_id)
        .collect();

    let mut profiles = Vec::new();
    for id in vault.list().map_err(vault_error)? {
        let profile = vault.load(&id).map_err(vault_error)?;
        profiles.push(ProfileSummary::of(&profile, running.contains(&profile.id)));
    }
    Ok(Json(json!({ "profiles": profiles })))
}

/// `POST /api/v1/profiles`.
async fn create_profile(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<(StatusCode, Json<Profile>), ApiError> {
    let vault = state.require_vault()?;
    let request: CreateProfileRequest = parse_body(&body)?;

    let name = request.name.trim();
    if name.is_empty() {
        return Err(ApiError::InvalidRequest(
            "name must not be empty".to_string(),
        ));
    }

    let seed = request.seed.unwrap_or_else(rand::random::<u64>);
    let mut fingerprint = generate(seed, request.os).map_err(generate_error)?;
    if let Some(overrides) = &request.overrides {
        fingerprint = apply_overrides(&fingerprint, overrides)?;
    }
    ensure_consistent(&fingerprint)?;

    let mut profile = Profile::new(name.to_string(), fingerprint, request.proxy).with_seed(seed);
    profile.tags = request.tags;
    vault.save(&profile).map_err(vault_error)?;

    Ok((StatusCode::CREATED, Json(profile)))
}

/// `GET /api/v1/profiles/:id`.
async fn get_profile(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<Profile>, ApiError> {
    let vault = state.require_vault()?;
    let profile = vault.load(&id).map_err(vault_error)?;
    Ok(Json(profile))
}

/// `PUT /api/v1/profiles/:id`.
async fn update_profile(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    body: Bytes,
) -> Result<Json<Profile>, ApiError> {
    let vault = state.require_vault()?;
    let request: UpdateProfileRequest = parse_body(&body)?;
    let mut profile = vault.load(&id).map_err(vault_error)?;

    if let Some(name) = request.name {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(ApiError::InvalidRequest(
                "name must not be empty".to_string(),
            ));
        }
        profile.name = name;
    }
    if let Some(tags) = request.tags {
        profile.tags = tags;
    }
    // `Some(None)` clears the proxy, `Some(Some(cfg))` replaces it.
    if let Some(proxy) = request.proxy {
        profile.proxy = proxy;
    }

    // A new seed rebuilds the fingerprint from scratch (deterministic), then
    // any overrides are layered on top; both paths revalidate before saving.
    let mut fingerprint = profile.fingerprint.clone();
    let mut fingerprint_changed = false;
    if let Some(seed) = request.seed {
        fingerprint = generate(seed, Some(profile.fingerprint.os)).map_err(generate_error)?;
        profile.seed = seed;
        fingerprint_changed = true;
    }
    if let Some(overrides) = &request.overrides {
        fingerprint = apply_overrides(&fingerprint, overrides)?;
        fingerprint_changed = true;
    }
    if fingerprint_changed {
        ensure_consistent(&fingerprint)?;
        profile.fingerprint = fingerprint;
    }

    profile.updated_at = now_rfc3339();
    vault.save(&profile).map_err(vault_error)?;
    Ok(Json(profile))
}

/// `DELETE /api/v1/profiles/:id`.
async fn delete_profile(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<StatusCode, ApiError> {
    let vault = state.require_vault()?;

    // Stop a running session first so Chromium releases the profile directory
    // before it is removed. `stop` is idempotent: stopping an idle profile is a
    // no-op success.
    state
        .session()
        .stop(&id)
        .await
        .map_err(|error| ApiError::Internal(format!("stop failed: {error}")))?;

    // Removing the profile record is the authoritative step; an unknown id is a
    // 404 and nothing on disk is touched.
    vault.delete(&id).map_err(vault_error)?;

    // Best effort: drop the Chromium user-data-dir. A leftover directory is
    // harmless, so a removal failure (including "does not exist") is ignored.
    let profile_dir = state.profile_dir(&id);
    if profile_dir.exists() {
        let _ = std::fs::remove_dir_all(&profile_dir);
    }

    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/profiles/:id/launch`.
async fn launch_profile(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<Value>, ApiError> {
    let vault = state.require_vault()?;
    let profile = vault.load(&id).map_err(vault_error)?;
    let profile_dir = state.profile_dir(&id);

    match state
        .session()
        .launch_profile(profile, profile_dir.clone())
        .await
    {
        Ok(handle) => Ok(Json(json!({
            "target_id": handle.target_id,
            "ws_url": handle.ws_url,
            "debug_port": handle.debug_port,
            "profile_dir": profile_dir.to_string_lossy(),
        }))),
        Err(error) => {
            if error.downcast_ref::<AlreadyRunning>().is_some() {
                Err(ApiError::AlreadyRunning(format!(
                    "profile `{id}` is already running"
                )))
            } else {
                Err(ApiError::Internal(format!("launch failed: {error}")))
            }
        }
    }
}

/// `POST /api/v1/profiles/:id/stop`.
async fn stop_profile(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<Value>, ApiError> {
    state.require_vault()?;

    let running = state.session().running().await;
    if !running.iter().any(|session| session.profile_id == id) {
        return Err(ApiError::NotRunning(format!(
            "profile `{id}` is not running"
        )));
    }

    state
        .session()
        .stop(&id)
        .await
        .map_err(|error| ApiError::Internal(format!("stop failed: {error}")))?;
    Ok(Json(json!({})))
}

/// `GET /api/v1/status`.
async fn status(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    state.require_vault()?;
    let running: Vec<Value> = state
        .session()
        .running()
        .await
        .into_iter()
        .map(|session| {
            json!({
                "profile_id": session.profile_id,
                "target_id": session.target_id,
                "debug_port": session.debug_port,
                "started_at": system_time_to_rfc3339(session.started_at),
            })
        })
        .collect();
    Ok(Json(json!({ "running": running })))
}

/// `POST /api/v1/proxy/check` — always `200`; success is the `ok` flag.
async fn proxy_check(body: Bytes) -> Result<Json<Value>, ApiError> {
    let request: ProxyCheckRequest = parse_body(&body)?;
    let scheme = match request.scheme.trim().to_ascii_lowercase().as_str() {
        "http" => ProxyScheme::Http,
        "socks5" => ProxyScheme::Socks5,
        other => {
            return Err(ApiError::InvalidRequest(format!(
                "unsupported proxy scheme `{other}` (use http or socks5)"
            )))
        }
    };
    if request.host.trim().is_empty() {
        return Err(ApiError::InvalidRequest(
            "proxy host must not be empty".to_string(),
        ));
    }

    let config = ProxyCfg {
        scheme,
        host: request.host,
        port: request.port,
        username: request.username,
        password: request.password,
    };

    let started = Instant::now();
    match check_exit_ip(&config, PROXY_CHECK_TIMEOUT).await {
        Ok(ip) => Ok(Json(json!({
            "ok": true,
            "ip": ip.to_string(),
            "latency_ms": started.elapsed().as_millis() as u64,
        }))),
        Err(error) => Ok(Json(json!({ "ok": false, "error": error.to_string() }))),
    }
}

/// `POST /api/v1/profiles/:id/cookies/import`.
async fn cookies_import(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    body: Bytes,
) -> Result<Json<CookiesImportResponse>, ApiError> {
    let vault = state.require_vault()?;
    // Ensure the profile exists before touching its directory.
    vault.load(&id).map_err(vault_error)?;

    let request: CookiesImportRequest = parse_body(&body)?;
    let format = normalize_cookie_format(&request.format)?;

    // Parse first (platform independent) so malformed data is a clean 400 even
    // where the store cannot be written.
    let total = count_cookies(&format, &request.data)?;
    let profile_dir = state.profile_dir(&id);
    let imported = import_cookies(&format, &profile_dir, &request.data)?;
    let skipped = total.saturating_sub(imported);

    Ok(Json(CookiesImportResponse { imported, skipped }))
}

/// `GET /api/v1/profiles/:id/cookies/export`.
async fn cookies_export(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<CookiesExportQuery>,
) -> Result<Json<CookiesExportResponse>, ApiError> {
    let vault = state.require_vault()?;
    vault.load(&id).map_err(vault_error)?;

    // `format` is a required query parameter in the contract; a missing value
    // is a client error, not a silent default to netscape.
    let format = match query.format.as_deref() {
        Some(format) => normalize_cookie_format(format)?,
        None => {
            return Err(ApiError::InvalidRequest(
                "missing required query parameter `format` (use netscape or json)".to_string(),
            ))
        }
    };
    let profile_dir = state.profile_dir(&id);
    let data = export_cookies(&format, &profile_dir)?;
    Ok(Json(CookiesExportResponse { format, data }))
}

/// `POST /api/v1/import`.
async fn import_profiles(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<ImportResponse>, ApiError> {
    let vault = state.require_vault()?;
    let request: ImportRequest = parse_body(&body)?;

    let results = run_import(&request.source, &request.data)?;

    // Safety net (defence in depth): the authoritative consistency guarantee
    // lives in `mirage-import`, but never persist a profile the contract
    // validator rejects. Validate every result first so a single bad profile
    // never leaves a partial save behind.
    for result in &results {
        ensure_consistent(&result.profile.fingerprint)?;
    }

    let mut imported = Vec::with_capacity(results.len());
    let mut warnings = Vec::new();
    for result in results {
        vault.save(&result.profile).map_err(vault_error)?;
        warnings.extend(warnings_to_json(&result.warnings));
        imported.push(result.profile);
    }

    Ok(Json(ImportResponse { imported, warnings }))
}

/// Run the requested importer, mapping its errors onto the native envelope.
fn run_import(source: &str, data: &str) -> Result<Vec<ImportResult>, ApiError> {
    match source.trim().to_ascii_lowercase().as_str() {
        "adspower" => mirage_import::import_ads_power_all(data).map_err(import_error),
        "dolphin" => mirage_import::import_dolphin_all(data).map_err(import_error),
        "gologin" => mirage_import::import_gologin_all(data).map_err(import_error),
        other => Err(ApiError::InvalidRequest(format!(
            "unknown import source `{other}` (use adspower, dolphin or gologin)"
        ))),
    }
}

/// Import failures are client problems except for a broken fp-db.
fn import_error(error: ImportError) -> ApiError {
    match error {
        ImportError::Generate(inner) => {
            ApiError::Internal(format!("fingerprint generation failed: {inner}"))
        }
        other => ApiError::InvalidRequest(other.to_string()),
    }
}

/// Canonicalize a cookie `format` value, rejecting anything unknown.
fn normalize_cookie_format(format: &str) -> Result<String, ApiError> {
    match format.trim().to_ascii_lowercase().as_str() {
        "netscape" => Ok("netscape".to_string()),
        "json" => Ok("json".to_string()),
        other => Err(ApiError::InvalidRequest(format!(
            "unknown cookie format `{other}` (use netscape or json)"
        ))),
    }
}

/// Parse cookie data to count what was offered (platform independent).
fn count_cookies(format: &str, data: &str) -> Result<usize, ApiError> {
    let parsed = match format {
        "netscape" => mirage_cookies::parse_netscape(data),
        "json" => mirage_cookies::parse_json(data),
        // `format` is already normalized; unreachable in practice.
        _ => {
            return Err(ApiError::InvalidRequest(format!(
                "unknown cookie format `{format}`"
            )))
        }
    };
    parsed.map(|cookies| cookies.len()).map_err(cookie_error)
}

/// Map a cookie-parse or store failure onto the native envelope.
fn cookie_error(error: CookieError) -> ApiError {
    match error {
        CookieError::NoProfileDb(_) | CookieError::NoProfileKey(_) => ApiError::Io(
            "profile has no cookie database yet; launch the profile once so Chromium creates it"
                .to_string(),
        ),
        CookieError::DbLocked(detail) => ApiError::Io(format!(
            "cookie database is locked (close the running browser first): {detail}"
        )),
        CookieError::Parse { line, reason } => {
            ApiError::InvalidRequest(format!("cookies.txt parse error on line {line}: {reason}"))
        }
        CookieError::Json(error) => ApiError::InvalidRequest(format!("cookie JSON error: {error}")),
        CookieError::UnsupportedEncryption(detail) => ApiError::Io(detail),
        other => ApiError::Internal(format!("cookie store error: {other}")),
    }
}

/// Write cookies into the profile store (Windows only; DPAPI-protected key).
#[cfg(windows)]
fn import_cookies(format: &str, profile_dir: &Path, data: &str) -> Result<usize, ApiError> {
    let result = match format {
        "netscape" => mirage_cookies::import_netscape(profile_dir, data),
        "json" => mirage_cookies::import_json(profile_dir, data),
        other => {
            return Err(ApiError::InvalidRequest(format!(
                "unknown cookie format `{other}`"
            )))
        }
    };
    result.map_err(cookie_error)
}

#[cfg(not(windows))]
fn import_cookies(_format: &str, _profile_dir: &Path, _data: &str) -> Result<usize, ApiError> {
    Err(ApiError::Io(
        "cookie import requires Windows (the profile cookie store is DPAPI-protected)".to_string(),
    ))
}

/// Read cookies out of the profile store (Windows only).
#[cfg(windows)]
fn export_cookies(format: &str, profile_dir: &Path) -> Result<String, ApiError> {
    let result = match format {
        "netscape" => mirage_cookies::export_netscape(profile_dir),
        "json" => mirage_cookies::export_json(profile_dir),
        other => {
            return Err(ApiError::InvalidRequest(format!(
                "unknown cookie format `{other}`"
            )))
        }
    };
    result.map_err(cookie_error)
}

#[cfg(not(windows))]
fn export_cookies(format: &str, _profile_dir: &Path) -> Result<String, ApiError> {
    match format {
        "netscape" | "json" => Err(ApiError::Io(
            "cookie export requires Windows (the profile cookie store is DPAPI-protected)"
                .to_string(),
        )),
        other => Err(ApiError::InvalidRequest(format!(
            "unknown cookie format `{other}`"
        ))),
    }
}

/// Convert a `SystemTime` to RFC 3339 UTC seconds.
fn system_time_to_rfc3339(time: SystemTime) -> String {
    let datetime: chrono::DateTime<chrono::Utc> = time.into();
    datetime.to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_cookie_format_accepts_known_and_rejects_unknown() {
        assert_eq!(normalize_cookie_format("Netscape").expect("ok"), "netscape");
        assert_eq!(normalize_cookie_format(" json ").expect("ok"), "json");
        assert!(normalize_cookie_format("xml").is_err());
    }

    /// The shared guard used by create/update *and* import: a generated
    /// fingerprint passes, a tampered one is a `400 invalid_request`.
    #[test]
    fn ensure_consistent_accepts_generated_and_rejects_tampered() {
        let mut fingerprint = generate(20261103, Some(mirage_core::Os::Windows)).expect("generate");
        assert!(ensure_consistent(&fingerprint).is_ok());

        fingerprint.platform = "MacIntel".to_string();
        let error = ensure_consistent(&fingerprint).expect_err("must be inconsistent");
        assert_eq!(error.code(), "invalid_request");
    }

    #[test]
    fn count_cookies_validates_payload() {
        assert_eq!(
            count_cookies(
                "json",
                r#"[{"name":"a","value":"1","domain":".x.com","path":"/"}]"#
            )
            .expect("json parses"),
            1
        );
        // A bad JSON body is a 400.
        let err = count_cookies("json", "not json").expect_err("must fail");
        assert_eq!(err.code(), "invalid_request");
    }
}
