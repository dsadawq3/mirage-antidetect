//! AdsPower-compatible aliases.
//!
//! AdsPower scripts expect an envelope that is always HTTP `200` with
//! `{ "code": <0|non-0>, "msg": ..., "data": ... }` (code `0` = success). Every
//! failure below is therefore reported *inside* the envelope, never as a 4xx —
//! the single exception is a bad API token, which the auth middleware rejects
//! with a 401 before a handler runs.

use std::collections::HashSet;

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use mirage_launcher::AlreadyRunning;

use crate::models::{now_rfc3339, warnings_to_json};
use crate::state::{vault_error, AppState};

/// Generic error code used for every non-auth failure.
const CODE_ERROR: i64 = 1;

/// Build the AdsPower alias router (state not yet attached).
pub fn adspower_router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/browser/start", get(browser_start))
        .route("/api/v1/browser/stop", get(browser_stop))
        .route("/api/v1/browser/list", get(browser_list))
        .route("/api/v1/user/create", post(user_create))
        .route("/api/v1/user/update", post(user_update))
        .route("/api/v1/user/delete", post(user_delete))
}

/// `{ "code": 0, "msg": "success", "data": ... }`.
fn success(data: Value) -> Json<Value> {
    Json(json!({ "code": 0, "msg": "success", "data": data }))
}

/// `{ "code": <non-0>, "msg": ..., "data": null }`, still HTTP 200.
fn failure(code: i64, message: impl Into<String>) -> Json<Value> {
    Json(json!({ "code": code, "msg": message.into(), "data": Value::Null }))
}

/// `?user_id=`.
#[derive(Debug, Deserialize)]
struct UserQuery {
    user_id: Option<String>,
}

/// `?page=&page_size=`.
#[derive(Debug, Deserialize)]
struct ListQuery {
    page: Option<u32>,
    page_size: Option<u32>,
}

/// `GET /api/v1/browser/start?user_id=`.
async fn browser_start(
    State(state): State<AppState>,
    Query(query): Query<UserQuery>,
) -> Json<Value> {
    let Some(user_id) = non_empty(query.user_id) else {
        return failure(CODE_ERROR, "user_id is required");
    };
    let vault = match state.require_vault() {
        Ok(vault) => vault,
        Err(error) => return failure(CODE_ERROR, error.to_string()),
    };
    let profile = match vault.load(&user_id) {
        Ok(profile) => profile,
        Err(error) => return failure(CODE_ERROR, vault_error(error).to_string()),
    };

    let profile_dir = state.profile_dir(&user_id);
    match state.session().launch_profile(profile, profile_dir).await {
        Ok(handle) => success(json!({
            "ws": { "puppeteer": handle.ws_url },
            "debug_port": handle.debug_port,
            "target_id": handle.target_id,
        })),
        Err(error) => {
            if error.downcast_ref::<AlreadyRunning>().is_some() {
                failure(CODE_ERROR, format!("profile {user_id} is already running"))
            } else {
                failure(CODE_ERROR, format!("launch failed: {error}"))
            }
        }
    }
}

/// `GET /api/v1/browser/stop?user_id=`.
async fn browser_stop(
    State(state): State<AppState>,
    Query(query): Query<UserQuery>,
) -> Json<Value> {
    let Some(user_id) = non_empty(query.user_id) else {
        return failure(CODE_ERROR, "user_id is required");
    };
    if let Err(error) = state.require_vault() {
        return failure(CODE_ERROR, error.to_string());
    }

    let running = state.session().running().await;
    if !running.iter().any(|session| session.profile_id == user_id) {
        return failure(CODE_ERROR, format!("profile {user_id} is not running"));
    }
    match state.session().stop(&user_id).await {
        Ok(()) => success(json!({})),
        Err(error) => failure(CODE_ERROR, format!("stop failed: {error}")),
    }
}

/// `GET /api/v1/browser/list?page=&page_size=`.
async fn browser_list(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Json<Value> {
    let vault = match state.require_vault() {
        Ok(vault) => vault,
        Err(error) => return failure(CODE_ERROR, error.to_string()),
    };
    let ids = match vault.list() {
        Ok(ids) => ids,
        Err(error) => return failure(CODE_ERROR, vault_error(error).to_string()),
    };
    let running: HashSet<String> = state
        .session()
        .running()
        .await
        .into_iter()
        .map(|session| session.profile_id)
        .collect();

    let mut items = Vec::new();
    for id in &ids {
        let Ok(profile) = vault.load(id) else {
            // A profile that cannot be decrypted is skipped rather than
            // failing the whole listing.
            continue;
        };
        let status = if running.contains(&profile.id) {
            "Active"
        } else {
            ""
        };
        items.push(json!({
            "user_id": profile.id,
            "name": profile.name,
            "group_id": "0",
            "status": status,
        }));
    }

    let page = query.page.unwrap_or(1).max(1);
    let page_size = query.page_size.unwrap_or(50).clamp(1, 1000);
    let total = items.len();
    let start = ((page - 1) as usize * page_size as usize).min(total);
    let end = (start + page_size as usize).min(total);
    let slice: Vec<Value> = items[start..end].to_vec();

    success(json!({
        "list": slice,
        "page": page,
        "page_size": page_size,
        "total": total,
    }))
}

/// `POST /api/v1/user/create` — AdsPower profile body → a saved Mirage profile.
async fn user_create(State(state): State<AppState>, body: Bytes) -> Json<Value> {
    let vault = match state.require_vault() {
        Ok(vault) => vault,
        Err(error) => return failure(CODE_ERROR, error.to_string()),
    };
    let Ok(value) = serde_json::from_slice::<Value>(&body) else {
        return failure(CODE_ERROR, "invalid JSON body");
    };
    let Ok(serialized) = serde_json::to_string(&value) else {
        return failure(CODE_ERROR, "could not re-serialize the request body");
    };

    let result = match mirage_import::import_ads_power(&serialized) {
        Ok(result) => result,
        Err(error) => return failure(CODE_ERROR, error.to_string()),
    };
    if let Err(error) = vault.save(&result.profile) {
        return failure(CODE_ERROR, vault_error(error).to_string());
    }

    success(json!({
        "id": result.profile.id,
        "warnings": warnings_to_json(&result.warnings),
    }))
}

/// `POST /api/v1/user/update` — AdsPower body + `user_id`.
async fn user_update(
    State(state): State<AppState>,
    Query(query): Query<UserQuery>,
    body: Bytes,
) -> Json<Value> {
    let vault = match state.require_vault() {
        Ok(vault) => vault,
        Err(error) => return failure(CODE_ERROR, error.to_string()),
    };
    // An empty body is allowed; the id may come from the query string alone.
    let value: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);

    let user_id = non_empty(query.user_id).or_else(|| {
        value
            .get("user_id")
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    let Some(user_id) = user_id.filter(|id| !id.trim().is_empty()) else {
        return failure(CODE_ERROR, "user_id is required");
    };

    let mut profile = match vault.load(&user_id) {
        Ok(profile) => profile,
        Err(error) => return failure(CODE_ERROR, vault_error(error).to_string()),
    };
    if let Some(name) = value.get("name").and_then(Value::as_str) {
        if !name.trim().is_empty() {
            profile.name = name.to_string();
        }
    }
    if let Some(remark) = value.get("remark").and_then(Value::as_str) {
        profile.notes = remark.to_string();
    }
    profile.updated_at = now_rfc3339();

    if let Err(error) = vault.save(&profile) {
        return failure(CODE_ERROR, vault_error(error).to_string());
    }
    success(json!({}))
}

/// `POST /api/v1/user/delete` — `user_id` or `{ "user_ids": [] }`.
async fn user_delete(
    State(state): State<AppState>,
    Query(query): Query<UserQuery>,
    body: Bytes,
) -> Json<Value> {
    let vault = match state.require_vault() {
        Ok(vault) => vault,
        Err(error) => return failure(CODE_ERROR, error.to_string()),
    };

    let mut ids: Vec<String> = Vec::new();
    if let Some(id) = non_empty(query.user_id) {
        ids.push(id);
    }
    if ids.is_empty() {
        if let Ok(value) = serde_json::from_slice::<Value>(&body) {
            if let Some(array) = value.get("user_ids").and_then(Value::as_array) {
                for item in array {
                    if let Some(id) = item.as_str() {
                        ids.push(id.to_string());
                    }
                }
            }
            if ids.is_empty() {
                if let Some(id) = value.get("user_id").and_then(Value::as_str) {
                    ids.push(id.to_string());
                }
            }
        }
    }

    if ids.is_empty() {
        return failure(CODE_ERROR, "user_id or user_ids is required");
    }

    for id in ids {
        if let Err(error) = vault.delete(&id) {
            return failure(CODE_ERROR, vault_error(error).to_string());
        }
    }
    success(json!({}))
}

/// `Some(trimmed)` for a present, non-blank string.
fn non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_shapes_match_contract() {
        let ok = success(json!({ "a": 1 }));
        assert_eq!(ok.0["code"], 0);
        assert_eq!(ok.0["msg"], "success");
        assert_eq!(ok.0["data"]["a"], 1);

        let err = failure(CODE_ERROR, "boom");
        assert_eq!(err.0["code"], 1);
        assert_eq!(err.0["msg"], "boom");
        assert!(err.0["data"].is_null());
    }

    #[test]
    fn non_empty_trims_and_filters() {
        assert_eq!(non_empty(Some("  x ".to_string())).as_deref(), Some("x"));
        assert_eq!(non_empty(Some("   ".to_string())), None);
        assert_eq!(non_empty(None), None);
    }
}
