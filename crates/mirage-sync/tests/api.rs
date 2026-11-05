//! In-process API tests.
//!
//! The router is driven directly through `tower::ServiceExt::oneshot` against a
//! real SQLite file in a temporary directory — no sockets, no mocks. Storage
//! tests open a second raw connection to inspect columns at rest.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use mirage_sync::db::ProfileWrite;
use mirage_sync::{router, AppState, Db, DbConfig, KeyConfig};

/// Build an app on a fresh, bootstrap-admin'd database. Plaintext bodies so the
/// test is independent of `MIRAGE_SYNC_KEY` in the environment.
fn app() -> (tempfile::TempDir, Router, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Db::open_with_config(dir.path().join("mirage-sync.db"), DbConfig::plaintext())
        .expect("open db");
    let admin = db
        .ensure_bootstrap(None)
        .expect("bootstrap")
        .expect("fresh db yields a bootstrap admin key");
    (dir, router(AppState::new(db)), admin)
}

/// Build a request with an optional bearer token and JSON body.
fn req(method: &str, uri: &str, key: Option<&str>, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(key) = key {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {key}"));
    }
    let body = match body {
        Some(value) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(value.to_string())
        }
        None => Body::empty(),
    };
    builder.body(body).expect("request builds")
}

/// Build a request with a raw (possibly malformed) string body.
fn req_raw(method: &str, uri: &str, key: &str, raw: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {key}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(raw.to_owned()))
        .expect("request builds")
}

/// Run a request through the router and decode the JSON response.
async fn send(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.expect("router responds");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body collects")
        .to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

/// Create a key as the admin and return the raw key string.
async fn make_key(app: &Router, admin: &str, label: &str, role: &str) -> String {
    let (status, body) = send(
        app,
        req(
            "POST",
            "/v1/keys",
            Some(admin),
            Some(json!({ "label": label, "role": role })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "key creation: {body}");
    body["key"].as_str().expect("key in response").to_owned()
}

/// PUT a profile and return the status + body.
async fn put_profile(
    app: &Router,
    key: &str,
    id: &str,
    base_revision: i64,
    name: &str,
    tags: Value,
    body: Value,
) -> (StatusCode, Value) {
    send(
        app,
        req(
            "PUT",
            &format!("/v1/profiles/{id}"),
            Some(key),
            Some(json!({
                "name": name,
                "tags": tags,
                "body": body,
                "base_revision": base_revision,
            })),
        ),
    )
    .await
}

/// Read the raw `body_json` column with a second connection, bypassing the API.
fn raw_body_json(path: &std::path::Path, id: &str) -> String {
    let conn = rusqlite::Connection::open(path).expect("open raw connection");
    conn.query_row(
        "SELECT body_json FROM profiles WHERE id = ?1",
        rusqlite::params![id],
        |r| r.get(0),
    )
    .expect("body row")
}

const ID_A: &str = "6f9619ff-8b86-d011-b42d-00cf4fc964ff";
const ID_B: &str = "7a1c2d3e-4f5a-6b7c-8d9e-0f1a2b3c4d5e";

#[tokio::test]
async fn bootstrap_admin_key_authenticates_and_is_created_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db =
        Db::open_with_config(dir.path().join("db.sqlite"), DbConfig::plaintext()).expect("open db");

    let first = db.ensure_bootstrap(None).expect("bootstrap");
    assert!(first.is_some(), "fresh database must bootstrap a key");
    assert!(
        db.ensure_bootstrap(None)
            .expect("bootstrap again")
            .is_none(),
        "bootstrap must not run twice"
    );

    let admin = first.unwrap();
    let app = router(AppState::new(db));
    let (status, body) = send(&app, req("GET", "/v1/keys", Some(&admin), None)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "bootstrap key must be a valid admin: {body}"
    );
}

#[tokio::test]
async fn provided_bootstrap_key_is_installed_and_not_returned() {
    // M5: an operator-supplied key is used verbatim and never echoed back.
    let dir = tempfile::tempdir().expect("tempdir");
    let db =
        Db::open_with_config(dir.path().join("db.sqlite"), DbConfig::plaintext()).expect("open db");
    let provided = "operator-supplied-bootstrap-key-0123456789";
    assert!(
        db.ensure_bootstrap(Some(provided))
            .expect("bootstrap")
            .is_none(),
        "a provided key must not be returned for logging"
    );

    let app = router(AppState::new(db));
    let (status, _) = send(&app, req("GET", "/v1/keys", Some(provided), None)).await;
    assert_eq!(status, StatusCode::OK, "provided key must be a valid admin");
}

#[tokio::test]
async fn healthz_is_unauthenticated() {
    let (_dir, app, _admin) = app();
    let (status, body) = send(&app, req("GET", "/healthz", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "ok": true }));
}

#[tokio::test]
async fn missing_or_unknown_key_is_unauthorized() {
    let (_dir, app, _admin) = app();

    let (status, _) = send(&app, req("GET", "/v1/profiles", None, None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let bogus = "0".repeat(64);
    let (status, _) = send(&app, req("GET", "/v1/profiles", Some(&bogus), None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn revoked_key_is_unauthorized() {
    let (_dir, app, admin) = app();
    let member = make_key(&app, &admin, "member-1", "member").await;

    // Usable before revocation.
    let (status, _) = send(&app, req("GET", "/v1/profiles", Some(&member), None)).await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = send(&app, req("DELETE", "/v1/keys/member-1", Some(&admin), None)).await;
    assert_eq!(status, StatusCode::OK);

    // Dead after revocation.
    let (status, _) = send(&app, req("GET", "/v1/profiles", Some(&member), None)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn member_is_forbidden_on_admin_key_routes() {
    let (_dir, app, admin) = app();
    let member = make_key(&app, &admin, "member-1", "member").await;

    let (status, _) = send(
        &app,
        req(
            "POST",
            "/v1/keys",
            Some(&member),
            Some(json!({ "label": "sneaky", "role": "admin" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = send(&app, req("GET", "/v1/keys", Some(&member), None)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The admin can do both.
    let (status, _) = send(&app, req("GET", "/v1/keys", Some(&admin), None)).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn member_cannot_delete_profile_but_admin_can() {
    let (_dir, app, admin) = app();
    let member = make_key(&app, &admin, "member-1", "member").await;

    let (status, _) = put_profile(
        &app,
        &member,
        ID_A,
        0,
        "shop",
        json!(["work"]),
        json!({"fp": 1}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = send(
        &app,
        req(
            "DELETE",
            &format!("/v1/profiles/{ID_A}"),
            Some(&member),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = send(
        &app,
        req(
            "DELETE",
            &format!("/v1/profiles/{ID_A}"),
            Some(&admin),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = send(
        &app,
        req("GET", &format!("/v1/profiles/{ID_A}"), Some(&admin), None),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn put_enforces_base_revision() {
    let (_dir, app, admin) = app();

    let (status, body) = put_profile(
        &app,
        &admin,
        ID_A,
        0,
        "shop",
        json!(["work"]),
        json!({"v": 1}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["revision"], json!(1));

    let (status, body) = put_profile(
        &app,
        &admin,
        ID_A,
        0,
        "shop",
        json!(["work"]),
        json!({"v": 2}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["current_revision"], json!(1));

    let (status, body) = put_profile(
        &app,
        &admin,
        ID_A,
        1,
        "shop",
        json!(["work"]),
        json!({"v": 2}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["revision"], json!(2));

    // Stale write is rejected and reports the true revision.
    let (status, body) = put_profile(
        &app,
        &admin,
        ID_A,
        1,
        "shop",
        json!(["work"]),
        json!({"v": 3}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["current_revision"], json!(2));
}

#[tokio::test]
async fn malformed_json_body_is_bad_request() {
    let (_dir, app, admin) = app();
    let (status, _) = send(
        &app,
        req_raw("PUT", &format!("/v1/profiles/{ID_A}"), &admin, "{ not json"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn profile_roundtrip_preserves_body() {
    let (_dir, app, admin) = app();

    let body = json!({ "fingerprint": { "os": "windows", "canvasSeed": 42 }, "proxy": null });
    let (status, _) = put_profile(
        &app,
        &admin,
        ID_A,
        0,
        "shop-01",
        json!(["work", "eu"]),
        body.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, fetched) = send(
        &app,
        req("GET", &format!("/v1/profiles/{ID_A}"), Some(&admin), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["id"], json!(ID_A));
    assert_eq!(fetched["name"], json!("shop-01"));
    assert_eq!(fetched["tags"], json!(["work", "eu"]));
    assert_eq!(
        fetched["body"], body,
        "opaque body must survive a round trip"
    );
    assert_eq!(fetched["revision"], json!(1));
}

#[tokio::test]
async fn tag_filter_selects_matching_profiles() {
    let (_dir, app, admin) = app();

    put_profile(&app, &admin, ID_A, 0, "a", json!(["work"]), json!({})).await;
    put_profile(&app, &admin, ID_B, 0, "b", json!(["home"]), json!({})).await;

    let (status, all) = send(&app, req("GET", "/v1/profiles", Some(&admin), None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(all.as_array().map(Vec::len), Some(2));

    let (status, filtered) = send(
        &app,
        req("GET", "/v1/profiles?tag=work", Some(&admin), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<&str> = filtered
        .as_array()
        .expect("array")
        .iter()
        .map(|p| p["id"].as_str().expect("id"))
        .collect();
    assert_eq!(ids, vec![ID_A]);
}

#[tokio::test]
async fn duplicate_active_label_conflicts() {
    let (_dir, app, admin) = app();
    make_key(&app, &admin, "ops", "member").await;

    let (status, _) = send(
        &app,
        req(
            "POST",
            "/v1/keys",
            Some(&admin),
            Some(json!({ "label": "ops", "role": "admin" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

// --- MINOR: partial update -------------------------------------------------

#[tokio::test]
async fn put_without_name_or_tags_preserves_them() {
    let (_dir, app, admin) = app();
    put_profile(
        &app,
        &admin,
        ID_A,
        0,
        "shop",
        json!(["work"]),
        json!({"v": 1}),
    )
    .await;

    // Only `body` and `base_revision`: name/tags keys are absent entirely.
    let (status, body) = send(
        &app,
        req(
            "PUT",
            &format!("/v1/profiles/{ID_A}"),
            Some(&admin),
            Some(json!({ "body": { "v": 2 }, "base_revision": 1 })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "partial update: {body}");
    assert_eq!(body["revision"], json!(2));

    let (_status, fetched) = send(
        &app,
        req("GET", &format!("/v1/profiles/{ID_A}"), Some(&admin), None),
    )
    .await;
    assert_eq!(
        fetched["name"],
        json!("shop"),
        "absent name must be preserved"
    );
    assert_eq!(
        fetched["tags"],
        json!(["work"]),
        "absent tags must be preserved"
    );
    assert_eq!(fetched["body"], json!({"v": 2}));
}

#[tokio::test]
async fn explicit_empty_name_and_tags_clear_them() {
    let (_dir, app, admin) = app();
    put_profile(
        &app,
        &admin,
        ID_A,
        0,
        "shop",
        json!(["work"]),
        json!({"v": 1}),
    )
    .await;

    let (status, _) = send(
        &app,
        req(
            "PUT",
            &format!("/v1/profiles/{ID_A}"),
            Some(&admin),
            Some(json!({ "name": "", "tags": [], "body": { "v": 2 }, "base_revision": 1 })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_status, fetched) = send(
        &app,
        req("GET", &format!("/v1/profiles/{ID_A}"), Some(&admin), None),
    )
    .await;
    assert_eq!(fetched["name"], json!(""), "explicit empty clears the name");
    assert_eq!(fetched["tags"], json!([]), "explicit empty clears the tags");
}

// --- MINOR: atomic write + audit ------------------------------------------

#[tokio::test]
async fn failed_write_is_not_audited_and_success_is() {
    let (_dir, app, admin) = app();

    // Conflicting write: no audit line must be written.
    let (status, _) = put_profile(&app, &admin, ID_A, 0, "a", json!([]), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = put_profile(&app, &admin, ID_A, 0, "a", json!([]), json!({})).await;
    assert_eq!(status, StatusCode::CONFLICT);

    let (_status, entries) = send(&app, req("GET", "/v1/audit", Some(&admin), None)).await;
    let puts = entries
        .as_array()
        .expect("audit array")
        .iter()
        .filter(|e| e["action"] == json!("profile.put"))
        .count();
    assert_eq!(puts, 1, "only the successful put may be audited");
}

// --- MINOR: pagination -----------------------------------------------------

#[tokio::test]
async fn profiles_and_keys_support_pagination() {
    let (_dir, app, admin) = app();
    put_profile(&app, &admin, ID_A, 0, "a", json!([]), json!({})).await;
    put_profile(&app, &admin, ID_B, 0, "b", json!([]), json!({})).await;

    let (status, page0) = send(
        &app,
        req("GET", "/v1/profiles?limit=1&offset=0", Some(&admin), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, page1) = send(
        &app,
        req("GET", "/v1/profiles?limit=1&offset=1", Some(&admin), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let page0 = page0.as_array().expect("array");
    let page1 = page1.as_array().expect("array");
    assert_eq!(page0.len(), 1);
    assert_eq!(page1.len(), 1);
    assert_ne!(page0[0]["id"], page1[0]["id"], "pages must not overlap");

    make_key(&app, &admin, "k1", "member").await;
    make_key(&app, &admin, "k2", "member").await;
    let (status, keys) = send(&app, req("GET", "/v1/keys?limit=1", Some(&admin), None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(keys.as_array().expect("array").len(), 1);
}

// --- MINOR: auth-first + single error envelope -----------------------------

#[tokio::test]
async fn malformed_query_authenticates_first_and_uses_the_envelope() {
    let (_dir, app, admin) = app();

    // No credentials + malformed query -> 401 (auth runs before query parsing).
    let (status, body) = send(
        &app,
        req("GET", "/v1/profiles?limit=notanumber", None, None),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "auth must precede query parsing"
    );
    assert!(
        body["error"].is_string(),
        "error body must be the envelope: {body}"
    );

    // Authenticated + malformed query -> 400 envelope.
    let (status, body) = send(
        &app,
        req("GET", "/v1/profiles?limit=notanumber", Some(&admin), None),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"]
        .as_str()
        .expect("error string")
        .contains("query"));
}

#[tokio::test]
async fn framework_errors_are_normalized_to_the_envelope() {
    let (_dir, app, admin) = app();

    // Unknown route -> JSON envelope, not an empty text body.
    let (status, body) = send(&app, req("GET", "/nope", None, None)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["error"].is_string(), "404 body: {body}");

    // Wrong method on an existing route -> 405 normalized to the envelope.
    let (status, body) = send(&app, req("POST", "/healthz", Some(&admin), None)).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert!(body["error"].is_string(), "405 body: {body}");
}

// --- MINOR: immutable audit subject ---------------------------------------

#[tokio::test]
async fn audit_subject_is_the_immutable_key_id() {
    let (_dir, app, admin) = app();
    put_profile(&app, &admin, ID_A, 0, "a", json!([]), json!({})).await;

    let (_status, entries) = send(&app, req("GET", "/v1/audit", Some(&admin), None)).await;
    let entries = entries.as_array().expect("audit array");

    // Key minting is audited too and carries an immutable id.
    make_key(&app, &admin, "member-2", "member").await;
    let (_status, entries_after) = send(&app, req("GET", "/v1/audit", Some(&admin), None)).await;
    assert!(entries_after
        .as_array()
        .expect("audit array")
        .iter()
        .any(|e| e["action"] == json!("key.create")));

    let put = entries
        .iter()
        .find(|e| e["action"] == json!("profile.put"))
        .expect("a profile.put entry");
    assert_eq!(put["profile_id"], json!(ID_A));

    // The subject is the 16-hex prefix of sha256(key), not the reusable label.
    let key_id = put["key_id"].as_str().expect("key_id string");
    assert_eq!(key_id.len(), 16);
    assert!(key_id.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(key_id, &mirage_sync::secrets::hash_key(&admin)[..16]);
    assert_ne!(
        key_id, "bootstrap",
        "the reusable label must not be the subject"
    );
}

// --- MAJOR M6: encryption at rest -----------------------------------------

fn key_config(key: [u8; 32]) -> DbConfig {
    DbConfig {
        key: KeyConfig::Hex(key),
        audit_retention: 100,
    }
}

#[tokio::test]
async fn bodies_are_encrypted_at_rest_when_a_key_is_set() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("enc.db");
    let key = [0x5a; 32];

    let db = Db::open_with_config(&path, key_config(key)).expect("open");
    assert!(db.is_encrypted());
    let admin = db.ensure_bootstrap(None).expect("bootstrap").expect("key");
    let app = router(AppState::new(db));

    let secret = json!({ "proxy": { "username": "u", "password": "hunter2" } });
    let (status, _) = put_profile(
        &app,
        &admin,
        ID_A,
        0,
        "shop",
        json!(["work"]),
        secret.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // The raw column must be an envelope that does not leak the secret.
    let raw = raw_body_json(&path, ID_A);
    assert!(
        raw.starts_with("enc:v1:"),
        "stored body must be encrypted: {raw}"
    );
    assert!(
        !raw.contains("hunter2"),
        "plaintext secret must not survive at rest"
    );

    // Reading through the API still returns the original body.
    let (status, fetched) = send(
        &app,
        req("GET", &format!("/v1/profiles/{ID_A}"), Some(&admin), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fetched["body"], secret);
}

#[tokio::test]
async fn encrypted_bodies_survive_reopen_and_reject_a_wrong_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("enc.db");
    let key = [0x11; 32];
    let now = mirage_sync::now_rfc3339();
    let body = json!({ "proxy": { "password": "hunter2" }, "n": 1 });

    {
        let db = Db::open_with_config(&path, key_config(key)).expect("open");
        db.ensure_bootstrap(None).expect("bootstrap");
        db.put_profile_audited(
            ProfileWrite {
                id: ID_A,
                name: Some("shop"),
                tags: Some(&["work".to_owned()]),
                body: &body,
                base_revision: 0,
                updated_by: "subject",
                now: &now,
            },
            "subject",
        )
        .expect("put");
    }

    // Reopen with the same key: body is readable.
    let db = Db::open_with_config(&path, key_config(key)).expect("reopen");
    let rec = db.get_profile(ID_A).expect("get").expect("present");
    assert_eq!(rec.body, body);

    // Reopen with the wrong key: the row fails closed.
    let mut other = key;
    other[0] ^= 0xff;
    let db_wrong = Db::open_with_config(&path, key_config(other)).expect("reopen wrong");
    assert!(
        db_wrong.get_profile(ID_A).is_err(),
        "wrong key must fail closed"
    );
}

#[tokio::test]
async fn passphrase_key_derives_a_stable_key_from_the_persisted_salt() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("pass.db");
    let config = DbConfig {
        key: KeyConfig::Passphrase("correct horse battery staple".to_owned()),
        audit_retention: 100,
    };
    let now = mirage_sync::now_rfc3339();
    let body = json!({ "k": "v" });

    {
        let db = Db::open_with_config(&path, config.clone()).expect("open");
        db.ensure_bootstrap(None).expect("bootstrap");
        db.put_profile_audited(
            ProfileWrite {
                id: ID_A,
                name: None,
                tags: None,
                body: &body,
                base_revision: 0,
                updated_by: "subject",
                now: &now,
            },
            "subject",
        )
        .expect("put");
    }

    // Same passphrase, new process: the persisted salt must reproduce the key.
    let db = Db::open_with_config(&path, config).expect("reopen");
    let rec = db.get_profile(ID_A).expect("get").expect("present");
    assert_eq!(rec.body, body);
}

#[tokio::test]
async fn plaintext_database_rejects_an_encrypted_row_without_a_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("flip.db");
    let now = mirage_sync::now_rfc3339();

    // Write one encrypted row.
    {
        let db = Db::open_with_config(&path, key_config([0x33; 32])).expect("open");
        db.ensure_bootstrap(None).expect("bootstrap");
        db.put_profile_audited(
            ProfileWrite {
                id: ID_A,
                name: None,
                tags: None,
                body: &json!({ "x": 1 }),
                base_revision: 0,
                updated_by: "subject",
                now: &now,
            },
            "subject",
        )
        .expect("put");
    }

    // Reopen with no key at all: an encrypted row must not silently decode.
    let db = Db::open_with_config(&path, DbConfig::plaintext()).expect("reopen");
    assert!(!db.is_encrypted());
    assert!(db.get_profile(ID_A).is_err(), "encrypted row needs the key");
}

// --- MINOR: bind policy ----------------------------------------------------

#[test]
fn bind_policy_allows_loopback_and_requires_opt_in_for_remote() {
    use std::net::SocketAddr;

    let v4: SocketAddr = "127.0.0.1:8787".parse().expect("addr");
    let v6: SocketAddr = "[::1]:8787".parse().expect("addr");
    let lan: SocketAddr = "0.0.0.0:8787".parse().expect("addr");

    assert!(mirage_sync::ensure_bind_allowed(v4, false).is_ok());
    assert!(mirage_sync::ensure_bind_allowed(v6, false).is_ok());
    assert!(mirage_sync::ensure_bind_allowed(lan, false).is_err());
    assert!(mirage_sync::ensure_bind_allowed(lan, true).is_ok());
}
