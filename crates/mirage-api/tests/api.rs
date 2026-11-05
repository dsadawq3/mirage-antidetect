//! In-process API tests: drive the router with `tower::ServiceExt::oneshot`
//! against a temp data directory, so no port is bound and no browser is
//! launched. The single exception — `already_running` on launch — needs a real
//! Chrome and is `#[ignore]`d behind `MIRAGE_E2E=1`.

use std::path::Path;

use axum::body::Body;
use axum::http::{header, HeaderValue, Method, Request, StatusCode};
use axum::response::Response;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use mirage_api::{router, AppState, Config};

/// Build an app over `data_dir`; the vault starts locked.
fn app(data_dir: &Path, token: Option<&str>) -> Router {
    let mut config = Config::for_data_dir(data_dir);
    config.token = token.map(str::to_string);
    router(AppState::new(config))
}

/// Build an app whose vault is unlocked at construction.
fn unlocked_app(data_dir: &Path, password: &str) -> Router {
    let mut config = Config::for_data_dir(data_dir);
    config.vault_password = Some(password.to_string());
    router(AppState::new(config))
}

async fn send(app: &Router, request: Request<Body>) -> Response {
    app.clone().oneshot(request).await.expect("router call")
}

fn request(method: Method, uri: &str, token: Option<&str>, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let body = match body {
        Some(value) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&value).expect("encode body"))
        }
        None => Body::empty(),
    };
    builder.body(body).expect("build request")
}

/// Read a JSON response body.
async fn json_body(response: Response) -> Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("response is JSON")
}

/// Send a request and return (status, JSON body).
async fn call(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let response = send(app, request(method, uri, token, body)).await;
    let status = response.status();
    (status, json_body(response).await)
}

/// The frozen error code from a native error envelope.
fn code(body: &Value) -> &str {
    body["error"]["code"].as_str().unwrap_or("<missing>")
}

/// Add an `Origin` header to a request.
fn with_origin(mut request: Request<Body>, origin: &str) -> Request<Body> {
    request.headers_mut().insert(
        header::ORIGIN,
        HeaderValue::from_str(origin).expect("valid origin"),
    );
    request
}

#[tokio::test]
async fn health_reports_locked_and_profile_endpoints_are_guarded() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = app(dir.path(), None);

    let (status, body) = call(&app, Method::GET, "/api/v1/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);
    assert_eq!(body["vault"], "locked");
    assert_eq!(body["running"], 0);
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));

    // Every native profile endpoint refuses while locked.
    for uri in ["/api/v1/profiles", "/api/v1/status"] {
        let (status, body) = call(&app, Method::GET, uri, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
        assert_eq!(code(&body), "vault_locked", "{uri}");
    }
}

#[tokio::test]
async fn vault_unlock_accepts_correct_and_rejects_wrong_password() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = app(dir.path(), None);

    let (status, body) = call(
        &app,
        Method::POST,
        "/api/v1/vault/unlock",
        None,
        Some(json!({ "password": "correct horse" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({}));

    // Store a profile so the vault has content to verify against.
    let (status, _) = call(
        &app,
        Method::POST,
        "/api/v1/profiles",
        None,
        Some(json!({ "name": "Shop-01", "os": "windows", "seed": 20261103 })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // Wrong password now fails verification with 401.
    let (status, body) = call(
        &app,
        Method::POST,
        "/api/v1/vault/unlock",
        None,
        Some(json!({ "password": "wrong" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(code(&body), "vault_locked");

    // Still unlocked with the original password.
    let (_, body) = call(&app, Method::GET, "/api/v1/health", None, None).await;
    assert_eq!(body["vault"], "unlocked");
}

#[tokio::test]
async fn profile_crud_roundtrip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = unlocked_app(dir.path(), "pw");

    // Create.
    let (status, created) = call(
        &app,
        Method::POST,
        "/api/v1/profiles",
        None,
        Some(json!({
            "name": "Shop-01",
            "tags": ["work"],
            "seed": 20261103,
            "os": "windows"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = created["id"].as_str().expect("id").to_string();
    assert_eq!(created["name"], "Shop-01");
    assert_eq!(created["tags"], json!(["work"]));

    // List shows it.
    let (status, list) = call(&app, Method::GET, "/api/v1/profiles", None, None).await;
    assert_eq!(status, StatusCode::OK);
    let profiles = list["profiles"].as_array().expect("profiles array");
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0]["id"], id);
    assert_eq!(profiles[0]["running"], false);

    // Get by id.
    let (status, fetched) = call(
        &app,
        Method::GET,
        &format!("/api/v1/profiles/{id}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["name"], "Shop-01");

    // Update name and tags.
    let (status, updated) = call(
        &app,
        Method::PUT,
        &format!("/api/v1/profiles/{id}"),
        None,
        Some(json!({ "name": "Shop-01-renamed", "tags": ["work", "eu"] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["name"], "Shop-01-renamed");
    assert_eq!(updated["tags"], json!(["work", "eu"]));

    // Update overrides that break consistency → 400.
    let (status, body) = call(
        &app,
        Method::PUT,
        &format!("/api/v1/profiles/{id}"),
        None,
        Some(json!({ "overrides": { "platform": "Nope" } })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&body), "invalid_request");

    // Delete → 204, then 404.
    let response = send(
        &app,
        request(
            Method::DELETE,
            &format!("/api/v1/profiles/{id}"),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let (status, body) = call(
        &app,
        Method::GET,
        &format!("/api/v1/profiles/{id}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(code(&body), "not_found");
}

#[tokio::test]
async fn create_rejects_inconsistent_overrides() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = unlocked_app(dir.path(), "pw");

    let (status, body) = call(
        &app,
        Method::POST,
        "/api/v1/profiles",
        None,
        Some(json!({
            "name": "bad",
            "os": "windows",
            "seed": 1,
            "overrides": { "platform": "MacIntel" }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&body), "invalid_request");
}

#[tokio::test]
async fn put_seed_reseeds_fingerprint_deterministically() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = unlocked_app(dir.path(), "pw");

    let (status, created) = call(
        &app,
        Method::POST,
        "/api/v1/profiles",
        None,
        Some(json!({ "name": "Seed-01", "os": "windows", "seed": 111 })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = created["id"].as_str().expect("id").to_string();
    // create records the seed and generates from it.
    assert_eq!(created["seed"], 111);
    let fp_a = created["fingerprint"].clone();

    // PUT a new seed: the fingerprint is rebuilt from scratch and differs.
    let (status, updated) = call(
        &app,
        Method::PUT,
        &format!("/api/v1/profiles/{id}"),
        None,
        Some(json!({ "seed": 222 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["seed"], 222);
    let fp_b = updated["fingerprint"].clone();
    assert_ne!(fp_b, fp_a, "a new seed must change the fingerprint");

    // Repeating the same seed is deterministic: identical fingerprint.
    let (status, again) = call(
        &app,
        Method::PUT,
        &format!("/api/v1/profiles/{id}"),
        None,
        Some(json!({ "seed": 222 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["fingerprint"], fp_b, "same seed -> same fingerprint");

    // It persisted.
    let (_, fetched) = call(
        &app,
        Method::GET,
        &format!("/api/v1/profiles/{id}"),
        None,
        None,
    )
    .await;
    assert_eq!(fetched["seed"], 222);
    assert_eq!(fetched["fingerprint"], fp_b);

    // Seed + overrides: overrides are layered on the rebuilt fingerprint.
    let (status, merged) = call(
        &app,
        Method::PUT,
        &format!("/api/v1/profiles/{id}"),
        None,
        Some(json!({ "seed": 222, "overrides": { "timezone": "Europe/Paris" } })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(merged["fingerprint"]["timezone"], "Europe/Paris");
    assert_eq!(merged["seed"], 222);

    // A seed combined with an inconsistent override is rejected.
    let (status, body) = call(
        &app,
        Method::PUT,
        &format!("/api/v1/profiles/{id}"),
        None,
        Some(json!({ "seed": 333, "overrides": { "platform": "MacIntel" } })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&body), "invalid_request");
}

#[tokio::test]
async fn cors_mirrors_allowlisted_origin_and_withholds_others() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = app(dir.path(), None);

    // No Origin (curl/SDK): no ACAO is needed, but the request works and the
    // method/header lists are still present.
    let response = send(&app, request(Method::GET, "/api/v1/health", None, None)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response
        .headers()
        .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
        .is_none());
    assert_eq!(
        response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_HEADERS)
            .expect("acah"),
        "authorization, content-type"
    );

    // An allowlisted Tauri origin is mirrored verbatim.
    let response = send(
        &app,
        with_origin(
            request(Method::GET, "/api/v1/health", None, None),
            "http://tauri.localhost",
        ),
    )
    .await;
    assert_eq!(
        response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .expect("acao"),
        "http://tauri.localhost"
    );

    // A cross-origin attacker gets no ACAO, so the browser blocks the read even
    // though the localhost request succeeded.
    let response = send(
        &app,
        with_origin(
            request(Method::GET, "/api/v1/health", None, None),
            "http://evil.example",
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response
        .headers()
        .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
        .is_none());
}

#[tokio::test]
async fn cors_preflight_allows_tauri_and_forbids_evil_origin() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = app(dir.path(), None);

    let preflight = |origin: Option<&str>| {
        let mut builder = Request::builder()
            .method(Method::OPTIONS)
            .uri("/api/v1/profiles");
        if let Some(origin) = origin {
            builder = builder.header(header::ORIGIN, origin);
        }
        builder.body(Body::empty()).expect("preflight request")
    };

    // No Origin: 204, no ACAO.
    let response = send(&app, preflight(None)).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(response
        .headers()
        .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
        .is_none());

    // Allowlisted origin: 204 with the origin mirrored and the method list.
    let response = send(&app, preflight(Some("http://tauri.localhost"))).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .expect("acao"),
        "http://tauri.localhost"
    );
    assert!(response
        .headers()
        .get(header::ACCESS_CONTROL_ALLOW_METHODS)
        .expect("acam")
        .to_str()
        .expect("ascii")
        .contains("DELETE"));

    // Non-allowlisted origin: the preflight is refused with no ACAO.
    let response = send(&app, preflight(Some("http://evil.example"))).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(response
        .headers()
        .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
        .is_none());
}

#[tokio::test]
async fn bearer_token_is_enforced_except_health() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = app(dir.path(), Some("secret"));

    // Health is exempt.
    let (status, _) = call(&app, Method::GET, "/api/v1/health", None, None).await;
    assert_eq!(status, StatusCode::OK);

    // Missing token.
    let (status, body) = call(&app, Method::GET, "/api/v1/profiles", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(code(&body), "invalid_request");

    // Wrong token.
    let (status, _) = call(&app, Method::GET, "/api/v1/profiles", Some("nope"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Correct token reaches the vault-locked guard (proving auth passed).
    let (status, body) = call(&app, Method::GET, "/api/v1/profiles", Some("secret"), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(code(&body), "vault_locked");

    // Unlock with the token, then the endpoint works.
    let (status, _) = call(
        &app,
        Method::POST,
        "/api/v1/vault/unlock",
        Some("secret"),
        Some(json!({ "password": "pw" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(&app, Method::GET, "/api/v1/profiles", Some("secret"), None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn stop_of_a_non_running_profile_is_not_running() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = unlocked_app(dir.path(), "pw");

    let (status, body) = call(
        &app,
        Method::POST,
        "/api/v1/profiles/00000000-0000-4000-8000-000000000000/stop",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(code(&body), "not_running");
}

#[tokio::test]
async fn delete_profile_removes_the_user_data_dir() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = unlocked_app(dir.path(), "pw");

    let (_, created) = call(
        &app,
        Method::POST,
        "/api/v1/profiles",
        None,
        Some(json!({ "name": "del", "os": "windows", "seed": 3 })),
    )
    .await;
    let id = created["id"].as_str().expect("id").to_string();

    // Simulate a Chromium user-data-dir with content on disk.
    let profile_dir = dir.path().join("profiles").join(&id);
    std::fs::create_dir_all(&profile_dir).expect("mkdir");
    std::fs::write(profile_dir.join("Cookies"), b"x").expect("write");
    assert!(profile_dir.exists());

    let response = send(
        &app,
        request(
            Method::DELETE,
            &format!("/api/v1/profiles/{id}"),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(
        !profile_dir.exists(),
        "the profile directory must be removed"
    );

    // Deleting again is a 404: the record is gone.
    let (status, body) = call(
        &app,
        Method::DELETE,
        &format!("/api/v1/profiles/{id}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(code(&body), "not_found");
}

#[tokio::test]
async fn cookies_export_requires_a_known_format() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = unlocked_app(dir.path(), "pw");

    let (_, created) = call(
        &app,
        Method::POST,
        "/api/v1/profiles",
        None,
        Some(json!({ "name": "exp", "os": "windows", "seed": 9 })),
    )
    .await;
    let id = created["id"].as_str().expect("id").to_string();

    // Missing required `format` → 400, not a silent netscape default.
    let (status, body) = call(
        &app,
        Method::GET,
        &format!("/api/v1/profiles/{id}/cookies/export"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&body), "invalid_request");

    // Unknown `format` → 400 too.
    let (status, body) = call(
        &app,
        Method::GET,
        &format!("/api/v1/profiles/{id}/cookies/export?format=xml"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&body), "invalid_request");
}

#[tokio::test]
async fn oversized_body_is_reported_as_a_contract_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = unlocked_app(dir.path(), "pw");

    // Exceed the 2 MiB body limit declared in `lib.rs`.
    let huge = "a".repeat(2 * 1024 * 1024 + 64);
    let body = serde_json::to_vec(&json!({ "name": huge })).expect("encode body");
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/profiles")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .expect("request");

    let response = send(&app, request).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body = json_body(response).await;
    assert_eq!(code(&body), "invalid_request");
}

#[tokio::test]
async fn create_rejects_unknown_request_fields() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = unlocked_app(dir.path(), "pw");

    let (status, body) = call(
        &app,
        Method::POST,
        "/api/v1/profiles",
        None,
        Some(json!({ "name": "typo", "tagz": ["a"] })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&body), "invalid_request");
}

#[tokio::test]
async fn adspower_aliases_use_the_envelope() {
    let dir = tempfile::tempdir().expect("tempdir");
    let api = unlocked_app(dir.path(), "pw");

    // Empty listing: code 0.
    let (status, body) = call(
        &api,
        Method::GET,
        "/api/v1/browser/list?page=1&page_size=10",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["code"], 0);
    assert_eq!(body["msg"], "success");
    assert_eq!(body["data"]["total"], 0);
    assert!(body["data"]["list"].as_array().expect("list").is_empty());

    // Unknown user on start: HTTP 200 but code != 0.
    let (status, body) = call(
        &api,
        Method::GET,
        "/api/v1/browser/start?user_id=does-not-exist",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(body["code"], 0);

    // A locked vault is also reported inside the envelope, not as a 4xx.
    let locked = app(dir.path(), None);
    let (status, body) = call(&locked, Method::GET, "/api/v1/browser/list", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(body["code"], 0);
}

#[tokio::test]
async fn proxy_check_reports_failure_inline() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = app(dir.path(), None);

    // Unknown scheme is a 400.
    let (status, body) = call(
        &app,
        Method::POST,
        "/api/v1/proxy/check",
        None,
        Some(json!({ "scheme": "ftp", "host": "127.0.0.1", "port": 1080 })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&body), "invalid_request");

    // A dead local proxy yields 200 with ok:false.
    let (status, body) = call(
        &app,
        Method::POST,
        "/api/v1/proxy/check",
        None,
        Some(json!({ "scheme": "socks5", "host": "127.0.0.1", "port": 1 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], false);
    assert!(body["error"].is_string());
}

#[tokio::test]
async fn cookies_import_rejects_bad_payload_and_missing_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = unlocked_app(dir.path(), "pw");

    let (_, created) = call(
        &app,
        Method::POST,
        "/api/v1/profiles",
        None,
        Some(json!({ "name": "c", "os": "windows", "seed": 7 })),
    )
    .await;
    let id = created["id"].as_str().expect("id").to_string();
    let uri = format!("/api/v1/profiles/{id}/cookies/import");

    // Malformed JSON payload → 400 invalid_request (platform independent).
    let (status, body) = call(
        &app,
        Method::POST,
        &uri,
        None,
        Some(json!({ "format": "json", "data": "not json" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&body), "invalid_request");

    // Unknown format → 400.
    let (status, _) = call(
        &app,
        Method::POST,
        &uri,
        None,
        Some(json!({ "format": "xml", "data": "x" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Valid payload but the profile never launched → 400 code "io".
    let (status, body) = call(
        &app,
        Method::POST,
        &uri,
        None,
        Some(json!({
            "format": "json",
            "data": "[{\"name\":\"a\",\"value\":\"1\",\"domain\":\".x.com\",\"path\":\"/\"}]"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&body), "io");
}

#[tokio::test]
async fn import_endpoint_saves_profiles() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = unlocked_app(dir.path(), "pw");

    let (status, body) = call(
        &app,
        Method::POST,
        "/api/v1/import",
        None,
        Some(json!({
            "source": "adspower",
            "data": "{\"user_id\":\"u1\",\"name\":\"Imported Shop\"}"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["imported"].as_array().expect("imported").len(), 1);
    assert_eq!(body["imported"][0]["name"], "Imported Shop");
    assert!(body["warnings"].is_array());

    // It persisted.
    let (_, list) = call(&app, Method::GET, "/api/v1/profiles", None, None).await;
    assert_eq!(list["profiles"].as_array().expect("profiles").len(), 1);

    // Unknown source → 400.
    let (status, body) = call(
        &app,
        Method::POST,
        "/api/v1/import",
        None,
        Some(json!({ "source": "nope", "data": "{}" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(code(&body), "invalid_request");
}

/// Launching requires a real Chrome. This exercises the `already_running`
/// branch (launch the same profile twice) and is opt-in.
#[tokio::test]
#[ignore = "launches a real Chrome window; set MIRAGE_E2E=1 and run with --ignored"]
async fn launch_twice_is_already_running() {
    if std::env::var("MIRAGE_E2E").ok().as_deref() != Some("1") {
        eprintln!("MIRAGE_E2E != 1: skipping the live launch test");
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let app = unlocked_app(dir.path(), "pw");

    let (_, created) = call(
        &app,
        Method::POST,
        "/api/v1/profiles",
        None,
        Some(json!({ "name": "live", "os": "windows", "seed": 20261103 })),
    )
    .await;
    let id = created["id"].as_str().expect("id").to_string();
    let uri = format!("/api/v1/profiles/{id}/launch");

    let (status, first) = call(&app, Method::POST, &uri, None, None).await;
    assert_eq!(status, StatusCode::OK, "first launch: {first}");

    let (status, body) = call(&app, Method::POST, &uri, None, None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(code(&body), "already_running");

    // Cleanup.
    let _ = call(
        &app,
        Method::POST,
        &format!("/api/v1/profiles/{id}/stop"),
        None,
        None,
    )
    .await;
}
