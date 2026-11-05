//! Live end-to-end roundtrip against a **real** Chrome profile.
//!
//! This test proves the whole path on a store Chromium actually created under
//! `--user-data-dir=X`: cookies resolve at `X/Default/Network/Cookies`, values
//! encrypted by this crate survive a byte-for-byte roundtrip, and an
//! all-app-bound (`v20`) store is reported as an error rather than an empty
//! export.
//!
//! It spawns a real, GUI Chrome, so it is `#[ignore]`d and additionally gated
//! on `MIRAGE_E2E=1`. Run it explicitly:
//!
//! ```text
//! MIRAGE_E2E=1 cargo test -p mirage-cookies --test live_roundtrip -- --ignored --nocapture
//! ```
#![cfg(windows)]

use std::path::Path;

use mirage_cookies::{export_json, export_netscape, import_netscape, Cookie, CookieError};
use mirage_launcher::{find_chrome, launch, LaunchConfig};

/// Live tests only run when `MIRAGE_E2E=1` is set (they open a Chrome window).
fn e2e_enabled() -> bool {
    std::env::var("MIRAGE_E2E")
        .map(|value| value == "1")
        .unwrap_or(false)
}

/// Launch a profile once on `user_data_dir` and stop it, so Chromium creates
/// the real profile structure. Returns `false` when Chrome is not installed.
async fn provision_with_chrome(user_data_dir: &Path) -> bool {
    let Some(chrome_path) = find_chrome() else {
        eprintln!("skipping live E2E: Chrome/Chromium not found");
        return false;
    };

    let fingerprint = mirage_core::generate(0xC0_0C_1E, Some(mirage_core::Os::Windows))
        .expect("generate fingerprint");
    let profile = mirage_core::Profile::new("cookies-e2e", fingerprint, None);
    let cfg = LaunchConfig {
        profile,
        chrome_path: Some(chrome_path),
        profile_dir: user_data_dir.to_path_buf(),
        debug_port: 0,
    };

    let mut browser = launch(&cfg).await.expect("launch Chrome");
    browser.stop().await.expect("stop Chrome");
    true
}

/// The database Chromium creates for a default `--user-data-dir` launch.
fn chrome_created_db(user_data_dir: &Path) -> std::path::PathBuf {
    user_data_dir
        .join("Default")
        .join("Network")
        .join("Cookies")
}

/// Empty the cookies table so the roundtrip assertions are exact.
fn clear_cookies(db_path: &Path) {
    let conn = rusqlite::Connection::open(db_path).expect("open cookie db");
    conn.execute("DELETE FROM cookies", [])
        .expect("clear cookies");
}

#[tokio::test]
#[ignore = "spawns a real Chrome window; run with --ignored and MIRAGE_E2E=1"]
async fn live_roundtrip_through_a_real_chrome_profile() {
    if !e2e_enabled() {
        eprintln!("skipping live E2E: set MIRAGE_E2E=1 to run");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let user_data_dir = tmp.path().join("profile");

    if !provision_with_chrome(&user_data_dir).await {
        return;
    }

    // 1. Chromium must have produced the real layout: root Local State, cookie
    //    database under Default/Network.
    assert!(
        user_data_dir.join("Local State").is_file(),
        "Local State must exist at the user-data-dir root"
    );
    let db_path = chrome_created_db(&user_data_dir);
    assert!(
        db_path.is_file(),
        "Chrome must create {db_path:?} (Default/Network/Cookies)"
    );
    assert!(
        !user_data_dir.join("Network").exists(),
        "Chrome must not create a Network directory at the root"
    );

    clear_cookies(&db_path);

    // Seed one row exactly as Chromium writes it, for a cookie we are about to
    // import: a correct upsert must replace it, not add a second row beside it.
    insert_chrome_style_row(&db_path, &user_data_dir);

    // 2. Import 2-3 cookies, including a secure/HttpOnly one and a session one.
    let future = mirage_cookies::chrome_db::now_unix_seconds() + 3_600;
    let input = format!(
        "# Netscape HTTP Cookie File\n\
         #HttpOnly_.example.com\tTRUE\t/\tTRUE\t{future}\tsid\tsession-value-äöü\n\
         .example.com\tTRUE\t/\tFALSE\t{future}\tpref\tvalue with spaces\n\
         shop.example.com\tFALSE\t/cart\tTRUE\t0\tcart\tpersistent-cart\n"
    );
    let written = import_netscape(&user_data_dir, &input).expect("import");
    assert_eq!(written, 3, "three distinct cookies must be written");

    // The DB is still the file Chromium created, now holding exactly our rows.
    assert!(db_path.is_file());
    let expected = vec![
        Cookie {
            name: "sid".to_owned(),
            value: "session-value-äöü".to_owned(),
            domain: ".example.com".to_owned(),
            path: "/".to_owned(),
            secure: true,
            http_only: true,
            expiry: Some(future),
        },
        Cookie {
            name: "pref".to_owned(),
            value: "value with spaces".to_owned(),
            domain: ".example.com".to_owned(),
            path: "/".to_owned(),
            secure: false,
            http_only: false,
            expiry: Some(future),
        },
        Cookie {
            name: "cart".to_owned(),
            value: "persistent-cart".to_owned(),
            domain: "shop.example.com".to_owned(),
            path: "/cart".to_owned(),
            secure: true,
            http_only: false,
            expiry: None,
        },
    ];

    // 3.+4. Export back and compare byte-for-byte.
    let netscape = export_netscape(&user_data_dir).expect("export netscape");
    let parsed = mirage_cookies::parse_netscape(&netscape).expect("parse export");
    assert_eq!(sorted(parsed), sorted(expected.clone()));

    let json = export_json(&user_data_dir).expect("export json");
    let parsed_json = mirage_cookies::parse_json(&json).expect("parse json export");
    assert_eq!(sorted(parsed_json), sorted(expected));

    // 5. Stats: all three exported, none skipped.
    let (_data, stats) =
        mirage_cookies::export_netscape_with_stats(&user_data_dir).expect("export with stats");
    assert_eq!(stats.exported, 3);
    assert_eq!(stats.skipped_unsupported, 0);

    // And the rows really live in the file Chrome created: the seeded
    // Chromium-style row was replaced (three rows, not four).
    let conn = rusqlite::Connection::open(&db_path).expect("open db");
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM cookies", [], |row| row.get(0))
        .expect("count");
    assert_eq!(
        count, 3,
        "the seeded Chromium row must be replaced, not shadowed"
    );
    let (pref_port, pref_hac): (i64, i64) = conn
        .query_row(
            "SELECT source_port, has_cross_site_ancestor FROM cookies WHERE name = 'pref'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("pref row");
    assert_eq!(pref_port, 443, "Chromium's source_port was reused");
    assert_eq!(pref_hac, 1);

    eprintln!("live_roundtrip: OK (db={})", db_path.display());
}

#[tokio::test]
#[ignore = "spawns a real Chrome window; run with --ignored and MIRAGE_E2E=1"]
async fn live_all_v20_export_is_an_error() {
    if !e2e_enabled() {
        eprintln!("skipping live E2E: set MIRAGE_E2E=1 to run");
        return;
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let user_data_dir = tmp.path().join("profile");

    if !provision_with_chrome(&user_data_dir).await {
        return;
    }

    let db_path = chrome_created_db(&user_data_dir);
    assert!(db_path.is_file(), "Chrome must create the cookie database");

    // A store whose only cookie is app-bound (v20): the format this crate
    // cannot decrypt.
    clear_cookies(&db_path);
    insert_v20_row(&db_path);

    let error = export_netscape(&user_data_dir).expect_err("all-v20 export must fail");
    match error {
        CookieError::UnsupportedEncryption(detail) => assert!(
            detail.contains("1 cookies use app-bound encryption (v20)"),
            "unexpected detail: {detail}"
        ),
        other => panic!("expected UnsupportedEncryption, got {other:?}"),
    }

    // The reader agrees: no readable cookie, one skipped.
    let (cookies, stats) =
        mirage_cookies::chrome_db::read_cookies_with_stats(&user_data_dir).expect("read stats");
    assert!(cookies.is_empty());
    assert_eq!(stats.skipped_unsupported, 1);

    eprintln!("live_all_v20: OK (UnsupportedEncryption surfaced)");
}

/// Seed a row exactly as Chromium writes an unpartitioned cookie: empty
/// `top_frame_site_key`, `has_cross_site_ancestor` 1, a concrete `source_port`
/// (443) and https `source_scheme` (2). The value is a valid `v10` blob so a
/// failed replacement surfaces as a duplicate row, not a decrypt error.
fn insert_chrome_style_row(db_path: &Path, user_data_dir: &Path) {
    let key = mirage_cookies::oscrypt::load_key(user_data_dir).expect("profile key");
    let blob = mirage_cookies::oscrypt::encrypt_value(&key, ".example.com", "chrome-seed")
        .expect("encrypt seed");
    let conn = rusqlite::Connection::open(db_path).expect("open cookie db");
    let now = mirage_cookies::chrome_db::now_windows_micros();
    conn.execute(
        "INSERT INTO cookies (creation_utc, host_key, top_frame_site_key, name, value, \
         encrypted_value, path, expires_utc, is_secure, is_httponly, last_access_utc, has_expires, \
         is_persistent, priority, samesite, source_scheme, source_port, last_update_utc, \
         source_type, has_cross_site_ancestor) \
         VALUES (?1, '.example.com', '', 'pref', '', ?2, '/', 0, 0, 0, ?1, 0, 0, 1, -1, 2, 443, ?1, 1, 1)",
        rusqlite::params![now, blob],
    )
    .expect("insert chrome-style row");
}

/// Insert a single `v20`-encrypted cookie row into a Chromium store.
fn insert_v20_row(db_path: &Path) {
    let conn = rusqlite::Connection::open(db_path).expect("open cookie db");
    let now = mirage_cookies::chrome_db::now_windows_micros();
    let expires = mirage_cookies::chrome_db::unix_to_windows_micros(
        mirage_cookies::chrome_db::now_unix_seconds() + 3_600,
    );
    let v20_blob: Vec<u8> = {
        let mut blob = b"v20".to_vec();
        blob.extend_from_slice(&[0xAA; 40]);
        blob
    };
    conn.execute(
        "INSERT INTO cookies (creation_utc, host_key, top_frame_site_key, name, value, \
         encrypted_value, path, expires_utc, is_secure, is_httponly, last_access_utc, has_expires, \
         is_persistent, priority, samesite, source_scheme, source_port, last_update_utc, \
         source_type, has_cross_site_ancestor) \
         VALUES (?1, ?2, '', ?3, '', ?4, '/', ?5, 1, 0, ?1, 1, 1, 1, -1, 2, -1, ?1, 1, 0)",
        rusqlite::params![now, ".example.com", "appbound", v20_blob, expires],
    )
    .expect("insert v20 row");
}

/// Sort by `(domain, name)` so comparisons ignore row order.
fn sorted(mut cookies: Vec<Cookie>) -> Vec<Cookie> {
    cookies.sort_by(|a, b| (&a.domain, &a.name).cmp(&(&b.domain, &b.name)));
    cookies
}
