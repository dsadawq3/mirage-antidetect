//! Profile-level tests: encrypt into and decrypt out of a Chromium-shaped
//! cookie database. Windows only (DPAPI-backed Local State key).
//!
//! The fixtures build the **real** layout a `--user-data-dir=X` launch
//! produces: `Local State` at the root of `X`, the cookie database under the
//! `Default` profile (`X/Default/Network/Cookies`). Building the old
//! sibling layout would only encode the path bug this crate fixed.
#![cfg(windows)]

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use mirage_cookies::{
    chrome_db, export_netscape, import_netscape, oscrypt, parse_netscape, Cookie, CookieError,
};

/// Deterministic 32-byte AES key for the fixtures.
const TEST_KEY: [u8; 32] = [0x42; 32];

/// Write a fake `Local State` holding `TEST_KEY`, DPAPI-protected, at the
/// **root** of the user-data-dir (where Chromium keeps it).
fn write_fake_local_state(dir: &Path) -> anyhow::Result<()> {
    let encoded = oscrypt::local_state_encrypted_key(&TEST_KEY)?;
    let json = format!(r#"{{"os_crypt":{{"encrypted_key":"{encoded}"}}}}"#);
    std::fs::write(dir.join("Local State"), json)?;
    Ok(())
}

/// Provision a profile that ran once, in Chromium's real layout: the root
/// `Local State` plus `<dir>/Default/Network/Cookies`.
fn provision(dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    write_fake_local_state(dir)?;
    chrome_db::create_profile_database(dir)?;
    Ok(())
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after 1970")
        .as_secs() as i64
}

/// Sort by `(domain, name)` so comparisons ignore row order.
fn sorted(mut cookies: Vec<Cookie>) -> Vec<Cookie> {
    cookies.sort_by(|a, b| (&a.domain, &a.name).cmp(&(&b.domain, &b.name)));
    cookies
}

#[test]
fn profile_layout_matches_chrome() -> anyhow::Result<()> {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path();
    provision(dir)?;

    assert!(
        dir.join("Local State").is_file(),
        "Local State lives at the root"
    );
    assert!(
        dir.join("Default")
            .join("Network")
            .join("Cookies")
            .is_file(),
        "cookies live under the Default profile"
    );
    assert!(
        !dir.join("Network").exists(),
        "Chromium never creates a Network directory at the user-data-dir root"
    );

    // The public resolver returns exactly the database Chromium created.
    assert_eq!(
        chrome_db::cookies_db_path(dir)?,
        dir.join("Default").join("Network").join("Cookies")
    );

    Ok(())
}

#[test]
fn write_then_read_matches() -> anyhow::Result<()> {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path();
    provision(dir)?;

    let now = now_unix();
    let cookies = vec![
        Cookie {
            name: "sid".to_owned(),
            value: "session-abc".to_owned(),
            domain: ".example.com".to_owned(),
            path: "/".to_owned(),
            secure: true,
            http_only: true,
            expiry: None,
        },
        Cookie {
            name: "token".to_owned(),
            value: "persist-xyz".to_owned(),
            domain: "example.com".to_owned(),
            path: "/app".to_owned(),
            secure: false,
            http_only: false,
            expiry: Some(now + 3_600),
        },
    ];

    let written = chrome_db::write_cookies(dir, &cookies)?;
    assert_eq!(written, 2);

    let read = chrome_db::read_cookies(dir)?;
    assert_eq!(sorted(read), sorted(cookies));

    Ok(())
}

#[test]
fn netscape_import_and_export_roundtrip() -> anyhow::Result<()> {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path();
    provision(dir)?;

    let now = now_unix();
    let cookies = vec![
        Cookie {
            name: "a".to_owned(),
            value: "1".to_owned(),
            domain: ".example.com".to_owned(),
            path: "/".to_owned(),
            secure: true,
            http_only: true,
            expiry: Some(now + 7_200),
        },
        Cookie {
            name: "b".to_owned(),
            value: "value with spaces".to_owned(),
            domain: "shop.example.com".to_owned(),
            path: "/cart".to_owned(),
            secure: false,
            http_only: false,
            expiry: None,
        },
    ];

    let text = mirage_cookies::serialize_netscape(&cookies);
    let written = import_netscape(dir, &text)?;
    assert_eq!(written, 2);

    let exported = export_netscape(dir)?;
    let parsed = parse_netscape(&exported)?;
    assert_eq!(sorted(parsed), sorted(cookies));

    Ok(())
}

#[test]
fn expired_cookies_are_dropped_on_export() -> anyhow::Result<()> {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path();
    provision(dir)?;

    let now = now_unix();
    let cookies = vec![
        Cookie {
            name: "old".to_owned(),
            value: "gone".to_owned(),
            domain: ".example.com".to_owned(),
            path: "/".to_owned(),
            secure: false,
            http_only: false,
            expiry: Some(now - 60),
        },
        Cookie {
            name: "fresh".to_owned(),
            value: "kept".to_owned(),
            domain: ".example.com".to_owned(),
            path: "/".to_owned(),
            secure: false,
            http_only: false,
            expiry: Some(now + 3_600),
        },
        Cookie {
            name: "session".to_owned(),
            value: "kept-too".to_owned(),
            domain: ".example.com".to_owned(),
            path: "/".to_owned(),
            secure: false,
            http_only: false,
            expiry: None,
        },
    ];

    chrome_db::write_cookies(dir, &cookies)?;

    let exported = export_netscape(dir)?;
    let parsed = parse_netscape(&exported)?;

    assert_eq!(parsed.len(), 2);
    assert!(parsed.iter().all(|cookie| cookie.name != "old"));

    Ok(())
}

#[test]
fn reimport_updates_in_place() -> anyhow::Result<()> {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path();
    provision(dir)?;

    let base = Cookie {
        name: "token".to_owned(),
        value: "v1".to_owned(),
        domain: ".example.com".to_owned(),
        path: "/".to_owned(),
        secure: true,
        http_only: false,
        expiry: None,
    };

    chrome_db::write_cookies(dir, std::slice::from_ref(&base))?;

    let updated = Cookie {
        value: "v2".to_owned(),
        ..base
    };
    chrome_db::write_cookies(dir, std::slice::from_ref(&updated))?;

    let read = chrome_db::read_cookies(dir)?;
    assert_eq!(read.len(), 1, "upsert must not duplicate the row");
    assert_eq!(read[0].value, "v2");

    Ok(())
}

#[test]
fn duplicate_keys_count_once() -> anyhow::Result<()> {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path();
    provision(dir)?;

    let cookie = Cookie {
        name: "token".to_owned(),
        value: "first".to_owned(),
        domain: ".example.com".to_owned(),
        path: "/".to_owned(),
        secure: true,
        http_only: false,
        expiry: None,
    };
    // Same partition key, different value: one row survives.
    let duplicate = Cookie {
        value: "second".to_owned(),
        ..cookie.clone()
    };

    let written = chrome_db::write_cookies(dir, &[cookie, duplicate])?;
    assert_eq!(written, 1, "duplicate keys must count as one written row");
    assert_eq!(chrome_db::read_cookies(dir)?.len(), 1);

    Ok(())
}

#[test]
fn writing_replaces_a_chrome_authored_row() -> anyhow::Result<()> {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path();
    provision(dir)?;

    // A row exactly as Chromium writes it: unpartitioned, has_cross_site_ancestor
    // 1, a concrete source_port (443) and an https source_scheme.
    let db_path = chrome_db::cookies_db_path(dir)?;
    let chrome_blob = oscrypt::encrypt_value(&TEST_KEY, ".example.com", "chrome-value")?;
    let now = chrome_db::now_windows_micros();
    let conn = rusqlite::Connection::open(&db_path)?;
    conn.execute(
        "INSERT INTO cookies (creation_utc, host_key, top_frame_site_key, name, value, \
         encrypted_value, path, expires_utc, is_secure, is_httponly, last_access_utc, has_expires, \
         is_persistent, priority, samesite, source_scheme, source_port, last_update_utc, \
         source_type, has_cross_site_ancestor) \
         VALUES (?1, '.example.com', '', 'token', '', ?2, '/', 0, 0, 1, ?1, 0, 0, 1, -1, 2, 443, ?1, 1, 1)",
        rusqlite::params![now, chrome_blob],
    )?;
    drop(conn);

    // Importing the same logical cookie (insecure, so the crate's heuristic
    // alone would pick scheme 1 / port -1 and miss Chromium's row).
    let cookie = Cookie {
        name: "token".to_owned(),
        value: "mirage-value".to_owned(),
        domain: ".example.com".to_owned(),
        path: "/".to_owned(),
        secure: false,
        http_only: true,
        expiry: None,
    };
    let written = chrome_db::write_cookies(dir, std::slice::from_ref(&cookie))?;
    assert_eq!(written, 1);

    // Exactly one row remains, with Chromium's partition attributes reused so
    // Chromium's own lookups keep finding it, and the value replaced.
    let conn = rusqlite::Connection::open(&db_path)?;
    let (count, scheme, port, hac): (i64, i64, i64, i64) = conn.query_row(
        "SELECT COUNT(*), source_scheme, source_port, has_cross_site_ancestor FROM cookies",
        rusqlite::params![],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    assert_eq!(count, 1, "the Chromium row must be replaced, not shadowed");
    assert_eq!(scheme, 2, "existing source_scheme is reused");
    assert_eq!(port, 443, "existing source_port is reused");
    assert_eq!(hac, 1);
    drop(conn);

    assert_eq!(chrome_db::read_cookies(dir)?, vec![cookie]);

    Ok(())
}

#[test]
fn missing_key_is_reported() -> anyhow::Result<()> {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path();
    std::fs::create_dir_all(dir)?;

    let error = chrome_db::write_cookies(dir, &[]).expect_err("no key");
    assert!(matches!(error, CookieError::NoProfileKey(_)));

    Ok(())
}

#[test]
fn missing_database_is_reported() -> anyhow::Result<()> {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path();
    std::fs::create_dir_all(dir)?;
    write_fake_local_state(dir)?;

    let error = chrome_db::read_cookies(dir).expect_err("no db");
    assert!(matches!(error, CookieError::NoProfileDb(_)));

    Ok(())
}

#[test]
fn resolves_cookie_db_from_local_state_last_used() -> anyhow::Result<()> {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path();
    std::fs::create_dir_all(dir)?;

    // No Default profile: only "Profile 4" holds a database, and Local State
    // points at it via profile.last_used.
    let encoded = oscrypt::local_state_encrypted_key(&TEST_KEY)?;
    let json = format!(
        r#"{{"os_crypt":{{"encrypted_key":"{encoded}"}},"profile":{{"last_used":"Profile 4","info_cache":{{"Profile 4":{{}}}}}}}}"#
    );
    std::fs::write(dir.join("Local State"), json)?;

    let db_dir = dir.join("Profile 4").join("Network");
    std::fs::create_dir_all(&db_dir)?;
    let conn = rusqlite::Connection::open(db_dir.join("Cookies"))?;
    chrome_db::ensure_schema(&conn)?;
    drop(conn);

    let cookie = Cookie {
        name: "sid".to_owned(),
        value: "from-profile-4".to_owned(),
        domain: ".example.com".to_owned(),
        path: "/".to_owned(),
        secure: true,
        http_only: true,
        expiry: None,
    };
    chrome_db::write_cookies(dir, std::slice::from_ref(&cookie))?;

    let read = chrome_db::read_cookies(dir)?;
    assert_eq!(read, vec![cookie]);
    assert_eq!(
        chrome_db::cookies_db_path(dir)?,
        db_dir.join("Cookies"),
        "resolver must pick the profile named by last_used"
    );

    Ok(())
}

#[test]
fn schema_older_than_v24_is_refused() -> anyhow::Result<()> {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path();
    std::fs::create_dir_all(dir)?;
    write_fake_local_state(dir)?;
    let db_path = chrome_db::create_profile_database(dir)?;

    // Pretend Chromium created this store with schema v23.
    let conn = rusqlite::Connection::open(&db_path)?;
    conn.execute("UPDATE meta SET value = '23' WHERE key = 'version'", [])?;
    drop(conn);

    let cookie = Cookie {
        name: "sid".to_owned(),
        value: "v".to_owned(),
        domain: ".example.com".to_owned(),
        path: "/".to_owned(),
        secure: true,
        http_only: false,
        expiry: None,
    };
    let error = chrome_db::write_cookies(dir, std::slice::from_ref(&cookie))
        .expect_err("writing into a v23 store must fail");
    assert!(
        matches!(error, CookieError::SchemaTooOld { found: 23 }),
        "expected SchemaTooOld(23), got {error:?}"
    );

    Ok(())
}

#[test]
fn database_records_use_chromium_schema() -> anyhow::Result<()> {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path();
    provision(dir)?;

    let cookie = Cookie {
        name: "sid".to_owned(),
        value: "value-123".to_owned(),
        domain: ".example.com".to_owned(),
        path: "/".to_owned(),
        secure: true,
        http_only: true,
        expiry: Some(now_unix() + 3_600),
    };
    chrome_db::write_cookies(dir, std::slice::from_ref(&cookie))?;

    let conn = rusqlite::Connection::open(chrome_db::cookies_db_path(dir)?)?;
    let (host_key, value, encrypted, is_secure, is_httponly, has_cross_site_ancestor): (
        String,
        String,
        Vec<u8>,
        i64,
        i64,
        i64,
    ) = conn.query_row(
        "SELECT host_key, value, encrypted_value, is_secure, is_httponly, has_cross_site_ancestor FROM cookies",
        rusqlite::params![],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        },
    )?;

    assert_eq!(host_key, ".example.com");
    assert_eq!(value, "", "plaintext column stays empty when encrypted");
    assert!(encrypted.starts_with(b"v10"), "value uses the v10 scheme");
    assert_eq!(is_secure, 1);
    assert_eq!(is_httponly, 1);
    assert_eq!(
        has_cross_site_ancestor, 1,
        "Chromium stores 1 for an unpartitioned cookie (empty top_frame_site_key)"
    );

    // The stored blob decrypts back to the original value with the profile key.
    let decrypted = oscrypt::decrypt_value(&TEST_KEY, &host_key, &encrypted)?;
    assert_eq!(decrypted, "value-123");

    Ok(())
}
