//! Read/write access to a Chromium profile cookie store.
//!
//! A Mirage profile is a Chromium `user-data-dir` (the `--user-data-dir=X`
//! value). Chromium does **not** put the cookie database at the root of `X`:
//! `Local State` lives at `X/Local State`, but cookies live under a profile
//! subdirectory, `X/<profile>/Network/Cookies`, where `<profile>` is `Default`
//! unless `--profile-directory` says otherwise. [`cookies_db_path`] resolves
//! that subdirectory (see [`resolve_cookies_db`]).
//!
//! The database is a SQLite file whose `cookies` table schema is fixed by
//! Chromium (schema version 24, the current one). Cookie values are encrypted
//! with the profile key from `Local State` (see [`crate::oscrypt`]) and stored
//! in `encrypted_value` with an empty `value` column.
//!
//! Timestamps are microseconds since 1601-01-01 UTC (the Windows FILETIME
//! epoch); [`unix_to_windows_micros`] and [`windows_micros_to_unix`] convert
//! to and from Unix seconds.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Transaction};

use crate::error::CookieError;
use crate::model::Cookie;
use crate::oscrypt;

/// Subdirectory holding the network databases.
const NETWORK_DIR: &str = "Network";
/// Cookie database file name.
const COOKIES_FILE: &str = "Cookies";
/// Profile subdirectory Chromium uses when no `--profile-directory` is given.
///
/// Mirage never passes `--profile-directory`, so a Mirage-launched profile
/// always creates `X/Default/Network/Cookies`.
const DEFAULT_PROFILE_SUBDIR: &str = "Default";
/// `Local State` file name, at the **root** of the user-data-dir.
const LOCAL_STATE_FILE: &str = "Local State";
/// `Local State` key naming the most recently used profile subdirectory.
const LAST_USED_KEY: &str = "last_used";
/// `Local State` key whose object keys are the known profile subdirectories.
const INFO_CACHE_KEY: &str = "info_cache";

/// Chromium schema version this crate reads and writes.
const REQUIRED_SCHEMA_VERSION: i64 = 24;

/// Seconds between the Windows FILETIME epoch (1601-01-01) and the Unix
/// epoch (1970-01-01).
const WINDOWS_EPOCH_OFFSET_SECS: i64 = 11_644_473_600;
/// Microseconds per second.
const MICROS_PER_SEC: i64 = 1_000_000;

/// Busy timeout for every SQLite connection. Best effort: it turns a brief
/// overlap with the browser into a short wait, and a persistent hold into a
/// prompt [`CookieError::DbLocked`] rather than an indefinite hang.
const BUSY_TIMEOUT: Duration = Duration::from_millis(500);

/// `has_cross_site_ancestor` Chromium stores for an **unpartitioned** cookie
/// (empty `top_frame_site_key`).
///
/// Chromium serializes an absent partition key as the pair
/// `(kEmptyCookiePartitionKey, true)`, so an empty `top_frame_site_key` goes
/// with `has_cross_site_ancestor = 1`. Verified against a live Chrome 156
/// store (3185 of 3185 unpartitioned rows are `1`). The earlier constant `0`
/// was wrong and made every row this crate wrote unmatchable by both its own
/// upsert and Chromium's deletes.
const HAS_CROSS_SITE_ANCESTOR_UNPARTITIONED: i64 = 1;
/// Top-frame site key of an unpartitioned cookie (the empty string).
const TOP_FRAME_SITE_KEY_UNPARTITIONED: &str = "";
/// `source_port` when unknown (`-1`).
const SOURCE_PORT_UNSPECIFIED: i64 = -1;
/// `source_scheme` for a plain-HTTP origin.
const SOURCE_SCHEME_HTTP: i64 = 1;
/// `source_scheme` for an HTTPS origin.
const SOURCE_SCHEME_HTTPS: i64 = 2;
/// `priority` = medium.
const PRIORITY_MEDIUM: i64 = 1;
/// `samesite` = unspecified.
const SAMESITE_UNSPECIFIED: i64 = -1;
/// `source_type` = http.
const SOURCE_TYPE_HTTP: i64 = 1;

/// Chromium's `cookies` table. Columns and order match `CreateV24Schema`
/// exactly; no column is invented.
const CREATE_COOKIES_TABLE: &str = "CREATE TABLE IF NOT EXISTS cookies(\
creation_utc INTEGER NOT NULL,\
host_key TEXT NOT NULL,\
top_frame_site_key TEXT NOT NULL,\
name TEXT NOT NULL,\
value TEXT NOT NULL,\
encrypted_value BLOB NOT NULL,\
path TEXT NOT NULL,\
expires_utc INTEGER NOT NULL,\
is_secure INTEGER NOT NULL,\
is_httponly INTEGER NOT NULL,\
last_access_utc INTEGER NOT NULL,\
has_expires INTEGER NOT NULL,\
is_persistent INTEGER NOT NULL,\
priority INTEGER NOT NULL,\
samesite INTEGER NOT NULL,\
source_scheme INTEGER NOT NULL,\
source_port INTEGER NOT NULL,\
last_update_utc INTEGER NOT NULL,\
source_type INTEGER NOT NULL,\
has_cross_site_ancestor INTEGER NOT NULL)";

/// Chromium's unique index, used by its upsert/delete by partition key.
const CREATE_COOKIES_INDEX: &str = "CREATE UNIQUE INDEX IF NOT EXISTS cookies_unique_index \
ON cookies(host_key, top_frame_site_key, has_cross_site_ancestor, name, path, source_scheme, source_port)";

/// Chromium's `meta` table (`sql::MetaTable`).
const CREATE_META_TABLE: &str = "CREATE TABLE IF NOT EXISTS meta(key LONGVARCHAR NOT NULL UNIQUE PRIMARY KEY, value LONGVARCHAR)";

/// Initialize the schema-version rows in `meta` (schema v24). Applied only
/// when `meta.version` is absent, so an existing older database keeps its own
/// version and [`require_schema_version`] can refuse to write into it.
const INIT_META_ROWS: &str = "INSERT OR IGNORE INTO meta(key, value) VALUES ('version', '24');\
INSERT OR IGNORE INTO meta(key, value) VALUES ('last_compatible_version', '24');";

/// The `INSERT` used for every cookie, in Chromium's exact column order.
const INSERT_COOKIE: &str =
    "INSERT INTO cookies (creation_utc, host_key, top_frame_site_key, name, \
value, encrypted_value, path, expires_utc, is_secure, is_httponly, last_access_utc, has_expires, \
is_persistent, priority, samesite, source_scheme, source_port, last_update_utc, source_type, \
has_cross_site_ancestor) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, \
?15, ?16, ?17, ?18, ?19, ?20)";

/// The seven columns of Chromium's `cookies_unique_index`, as this crate
/// writes them. Used both by the upsert `DELETE` and to count distinct rows.
type PartitionKey = (String, String, i64, String, String, i64, i64);

/// The resolved location of the cookie database inside a `user-data-dir`.
///
/// Returns [`CookieError::NoProfileDb`] when no profile subdirectory holds a
/// cookie database (the profile never launched, or it is not a Chromium
/// user-data-dir).
pub fn cookies_db_path(user_data_dir: &Path) -> Result<PathBuf, CookieError> {
    resolve_cookies_db(user_data_dir)
}

/// Path of a cookie database inside a named profile subdirectory of `X`.
fn cookies_db_in_profile(user_data_dir: &Path, profile: &str) -> PathBuf {
    user_data_dir
        .join(profile)
        .join(NETWORK_DIR)
        .join(COOKIES_FILE)
}

/// Resolve the cookie database for `user_data_dir`.
///
/// Search order:
/// 1. `X/Default/Network/Cookies` — the profile Chromium creates for a
///    `--user-data-dir=X` launch with no `--profile-directory` (Mirage never
///    passes one, so this is the profile Mirage profiles use). It is
///    deliberately preferred over `profile.last_used`, which on a shared
///    user-data-dir can name a different, user-selected profile;
/// 2. the profile named by `Local State`'s `profile.last_used`;
/// 3. any profile in `Local State`'s `profile.info_cache`.
fn resolve_cookies_db(user_data_dir: &Path) -> Result<PathBuf, CookieError> {
    let default_db = cookies_db_in_profile(user_data_dir, DEFAULT_PROFILE_SUBDIR);
    if default_db.is_file() {
        return Ok(default_db);
    }

    if let Some(meta) = read_profile_meta(user_data_dir) {
        if let Some(profile) = meta.last_used {
            let candidate = cookies_db_in_profile(user_data_dir, &profile);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
        for profile in meta.info_cache {
            let candidate = cookies_db_in_profile(user_data_dir, &profile);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    // Nothing matched: report the default location as the expected one.
    Err(CookieError::NoProfileDb(default_db))
}

/// Profile-selection fields read from `Local State`.
struct ProfileMeta {
    /// `profile.last_used`, the most recently used profile subdirectory.
    last_used: Option<String>,
    /// Keys of `profile.info_cache`, the known profile subdirectories.
    info_cache: Vec<String>,
}

/// Read `Local State` and extract the profile-selection fields.
///
/// Returns `None` when the file is missing or not valid JSON — callers treat
/// that as "no profile metadata", never as a hard error, because the default
/// profile is checked first anyway.
fn read_profile_meta(user_data_dir: &Path) -> Option<ProfileMeta> {
    let text = fs::read_to_string(user_data_dir.join(LOCAL_STATE_FILE)).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let profile = value.get("profile")?;

    let last_used = profile
        .get(LAST_USED_KEY)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let info_cache = profile
        .get(INFO_CACHE_KEY)
        .and_then(serde_json::Value::as_object)
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();

    Some(ProfileMeta {
        last_used,
        info_cache,
    })
}

/// Create `X/Default/Network/Cookies` with an empty Chromium schema.
///
/// Used to provision a profile that never ran, and by tests as a fixture. The
/// layout matches what a real `--user-data-dir` launch creates (`Local State`
/// stays at the root, the database under the `Default` profile). Idempotent:
/// an existing database is left untouched.
pub fn create_profile_database(user_data_dir: &Path) -> Result<PathBuf, CookieError> {
    let db_path = cookies_db_in_profile(user_data_dir, DEFAULT_PROFILE_SUBDIR);
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let conn = Connection::open(&db_path)?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    ensure_schema(&conn)?;
    Ok(db_path)
}

/// Create the `cookies`, `cookies_unique_index` and `meta` objects if they
/// are missing. Safe to run against a database Chromium already created.
///
/// The schema-version rows are stamped only when `meta.version` is absent, so
/// an existing older database keeps its own version and is not silently
/// relabelled as v24.
pub fn ensure_schema(conn: &Connection) -> Result<(), CookieError> {
    conn.execute_batch(CREATE_COOKIES_TABLE)?;
    conn.execute_batch(CREATE_COOKIES_INDEX)?;
    conn.execute_batch(CREATE_META_TABLE)?;
    if read_meta_version(conn)?.is_none() {
        conn.execute_batch(INIT_META_ROWS)?;
    }
    Ok(())
}

/// Read `meta.version`, or `None` when the row is absent or not an integer.
fn read_meta_version(conn: &Connection) -> Result<Option<i64>, CookieError> {
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'version'",
            rusqlite::params![],
            |row| row.get(0),
        )
        .optional()?;
    Ok(value.and_then(|value| value.trim().parse::<i64>().ok()))
}

/// Refuse to write unless the database is schema v24 or newer.
///
/// The v24 value format prepends `SHA-256(domain)` to the plaintext; writing
/// that into an older database would be corrupted further when Chromium runs
/// its own V23→V24 migration (adding a second prefix).
fn require_schema_version(conn: &Connection) -> Result<(), CookieError> {
    match read_meta_version(conn)? {
        Some(version) if version >= REQUIRED_SCHEMA_VERSION => Ok(()),
        // `ensure_schema` stamps v24 on a versionless database, so a `None`
        // here would be unexpected; refuse rather than guess.
        None => Err(CookieError::SchemaTooOld { found: 0 }),
        Some(found) => Err(CookieError::SchemaTooOld { found }),
    }
}

/// Encrypt and upsert `cookies` into the profile's cookie database.
///
/// Every cookie is keyed by Chromium's full `cookies_unique_index` tuple
/// `(host_key, top_frame_site_key, has_cross_site_ancestor, name, path,
/// source_scheme, source_port)`. A row already present for the same logical
/// cookie (same host, partition, name and path) is replaced: its own partition
/// attributes are reused so the delete hits Chromium's row instead of keying a
/// second one beside it (see [`resolve_partition`]).
///
/// Returns the number of **distinct** rows written (duplicate keys in
/// `cookies` collapse to one row). Fails with [`CookieError::NoProfileKey`]
/// when the key is missing, [`CookieError::DbLocked`] when the browser still
/// holds the file, and [`CookieError::SchemaTooOld`] when the database
/// predates schema v24.
pub fn write_cookies(user_data_dir: &Path, cookies: &[Cookie]) -> Result<usize, CookieError> {
    let key = oscrypt::load_key(user_data_dir)?;
    let db_path = cookies_db_path(user_data_dir)?;
    let mut conn = open_existing(&db_path)?;
    ensure_schema(&conn)?;
    require_schema_version(&conn)?;

    let now = now_windows_micros();
    let tx = conn.transaction()?;
    let mut distinct: HashSet<PartitionKey> = HashSet::new();
    for cookie in cookies {
        let written = upsert_cookie(&tx, &key, cookie, now)?;
        distinct.insert(written);
    }
    tx.commit()?;

    Ok(distinct.len())
}

/// Read and decrypt every cookie from the profile's cookie database.
///
/// Cookies stored with an encryption scheme this crate does not implement
/// (`v20` app-bound encryption) are skipped; genuine failures on supported
/// values are reported. Use [`read_cookies_with_stats`] to learn how many were
/// skipped.
pub fn read_cookies(user_data_dir: &Path) -> Result<Vec<Cookie>, CookieError> {
    Ok(read_cookies_with_stats(user_data_dir)?.0)
}

/// Counters gathered while reading a cookie store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReadStats {
    /// Rows skipped because their value uses unsupported encryption (`v20`).
    pub skipped_unsupported: usize,
}

/// [`read_cookies`] plus the number of skipped (unsupported-encryption) rows.
pub fn read_cookies_with_stats(
    user_data_dir: &Path,
) -> Result<(Vec<Cookie>, ReadStats), CookieError> {
    let key = oscrypt::load_key(user_data_dir)?;
    let db_path = cookies_db_path(user_data_dir)?;
    let conn = open_existing(&db_path)?;

    let mut stmt = conn.prepare(
        "SELECT host_key, name, value, encrypted_value, path, expires_utc, is_secure, is_httponly, has_expires \
         FROM cookies",
    )?;

    let rows = stmt.query_map(rusqlite::params![], |row| {
        Ok(RawRow {
            host_key: row.get(0)?,
            name: row.get(1)?,
            value: row.get(2)?,
            encrypted_value: row.get(3)?,
            path: row.get(4)?,
            expires_utc: row.get(5)?,
            is_secure: row.get(6)?,
            is_httponly: row.get(7)?,
            has_expires: row.get(8)?,
        })
    })?;

    let mut raw_rows = Vec::new();
    for row in rows {
        raw_rows.push(row?);
    }

    let mut cookies = Vec::with_capacity(raw_rows.len());
    let mut skipped_unsupported = 0usize;
    for row in raw_rows {
        match row.into_cookie(&key) {
            Ok(Some(cookie)) => cookies.push(cookie),
            // v20 / unknown scheme: not our format, count and skip.
            Ok(None) => skipped_unsupported += 1,
            Err(error) => return Err(error),
        }
    }

    Ok((
        cookies,
        ReadStats {
            skipped_unsupported,
        },
    ))
}

/// Current time as Windows FILETIME microseconds.
pub fn now_windows_micros() -> i64 {
    unix_to_windows_micros(now_unix_seconds())
}

/// Current time as Unix seconds (0 if the clock is before 1970).
pub fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

/// Convert Unix seconds to Windows FILETIME microseconds.
pub fn unix_to_windows_micros(unix_seconds: i64) -> i64 {
    (unix_seconds + WINDOWS_EPOCH_OFFSET_SECS) * MICROS_PER_SEC
}

/// Convert Windows FILETIME microseconds to Unix seconds.
pub fn windows_micros_to_unix(windows_micros: i64) -> i64 {
    windows_micros / MICROS_PER_SEC - WINDOWS_EPOCH_OFFSET_SECS
}

/// Open an existing cookie database, mapping lock/open failures clearly.
fn open_existing(db_path: &Path) -> Result<Connection, CookieError> {
    if !db_path.is_file() {
        return Err(CookieError::NoProfileDb(db_path.to_path_buf()));
    }

    let conn = Connection::open(db_path)?;
    // A short busy timeout turns "browser is holding the file" into a
    // prompt, descriptive error instead of an indefinite wait.
    conn.busy_timeout(BUSY_TIMEOUT)?;
    Ok(conn)
}

/// Default `source_scheme` for a cookie with no existing row, inferred from
/// `Secure`.
///
/// **Approximation:** Chromium's `source_scheme` is the scheme of the origin
/// that set the cookie (`0` unset, `1` non-secure, `2` secure), which no
/// portable format carries. The only signal available is the `Secure`
/// attribute, so this maps `Secure` → 2 and everything else → 1. It is used
/// only when no row exists yet; [`resolve_partition`] reuses an existing row's
/// own value so a real Chromium row is still replaced.
fn source_scheme(cookie: &Cookie) -> i64 {
    if cookie.secure {
        SOURCE_SCHEME_HTTPS
    } else {
        SOURCE_SCHEME_HTTP
    }
}

/// Resolve the partition attributes to write for `cookie`.
///
/// Returns `(has_cross_site_ancestor, source_scheme, source_port)`. When a row
/// already exists for the same logical cookie (host, partition, name, path),
/// that row's own three values are reused, so the delete below matches it
/// instead of keying a second row beside it. Otherwise the defaults are used:
/// the unpartitioned `has_cross_site_ancestor`, the `Secure`-derived
/// `source_scheme` heuristic and the unspecified `source_port`.
///
/// These three columns are part of Chromium's unique index and Chromium stores
/// concrete, originated values (`source_port` 443, `source_scheme` from the
/// setting origin), which is exactly why a fixed literal cannot replace a
/// Chromium-authored row.
fn resolve_partition(
    tx: &Transaction<'_>,
    cookie: &Cookie,
) -> Result<(i64, i64, i64), CookieError> {
    let existing = tx
        .query_row(
            "SELECT has_cross_site_ancestor, source_scheme, source_port FROM cookies \
             WHERE host_key = ?1 AND top_frame_site_key = ?2 AND name = ?3 AND path = ?4 LIMIT 1",
            rusqlite::params![
                cookie.domain,
                TOP_FRAME_SITE_KEY_UNPARTITIONED,
                cookie.name,
                cookie.path,
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;

    Ok(existing.unwrap_or((
        HAS_CROSS_SITE_ANCESTOR_UNPARTITIONED,
        source_scheme(cookie),
        SOURCE_PORT_UNSPECIFIED,
    )))
}

/// Delete the full partition-key row, then insert the encrypted cookie.
///
/// Returns the seven-column partition key actually written, so the caller can
/// count distinct rows.
fn upsert_cookie(
    tx: &Transaction<'_>,
    key: &[u8; oscrypt::KEY_LEN],
    cookie: &Cookie,
    now: i64,
) -> Result<PartitionKey, CookieError> {
    let encrypted_value = oscrypt::encrypt_value(key, &cookie.domain, &cookie.value)?;
    let expires_utc = cookie.expiry.map_or(0, unix_to_windows_micros);
    let has_expires = i64::from(cookie.expiry.is_some());
    let is_secure = i64::from(cookie.secure);
    let is_httponly = i64::from(cookie.http_only);
    let (has_cross_site_ancestor, source_scheme, source_port) = resolve_partition(tx, cookie)?;

    // Delete by the *full* unique-index tuple, using the resolved partition
    // attributes so an existing Chromium row for the same logical cookie is
    // removed rather than shadowed by a second row.
    tx.execute(
        "DELETE FROM cookies WHERE host_key = ?1 AND top_frame_site_key = ?2 \
         AND has_cross_site_ancestor = ?3 AND name = ?4 AND path = ?5 \
         AND source_scheme = ?6 AND source_port = ?7",
        rusqlite::params![
            cookie.domain,
            TOP_FRAME_SITE_KEY_UNPARTITIONED,
            has_cross_site_ancestor,
            cookie.name,
            cookie.path,
            source_scheme,
            source_port,
        ],
    )?;

    tx.execute(
        INSERT_COOKIE,
        rusqlite::params![
            now,                              // creation_utc
            cookie.domain,                    // host_key
            TOP_FRAME_SITE_KEY_UNPARTITIONED, // top_frame_site_key
            cookie.name,                      // name
            "",                               // value (ciphertext lives in encrypted_value)
            encrypted_value,                  // encrypted_value
            cookie.path,                      // path
            expires_utc,                      // expires_utc
            is_secure,                        // is_secure
            is_httponly,                      // is_httponly
            now,                              // last_access_utc
            has_expires,                      // has_expires
            has_expires,                      // is_persistent
            PRIORITY_MEDIUM,                  // priority
            SAMESITE_UNSPECIFIED,             // samesite
            source_scheme,                    // source_scheme
            source_port,                      // source_port
            now,                              // last_update_utc
            SOURCE_TYPE_HTTP,                 // source_type
            has_cross_site_ancestor,          // has_cross_site_ancestor
        ],
    )?;

    Ok((
        cookie.domain.clone(),
        TOP_FRAME_SITE_KEY_UNPARTITIONED.to_owned(),
        has_cross_site_ancestor,
        cookie.name.clone(),
        cookie.path.clone(),
        source_scheme,
        source_port,
    ))
}

/// One `cookies` row, as loaded from SQLite before decryption.
struct RawRow {
    host_key: String,
    name: String,
    value: String,
    encrypted_value: Vec<u8>,
    path: String,
    expires_utc: i64,
    is_secure: i64,
    is_httponly: i64,
    has_expires: i64,
}

impl RawRow {
    /// Decrypt this row into a [`Cookie`].
    ///
    /// Returns `Ok(None)` when the value uses an unsupported scheme so the
    /// caller can count and skip it, and `Err` for real decryption failures.
    fn into_cookie(self, key: &[u8; oscrypt::KEY_LEN]) -> Result<Option<Cookie>, CookieError> {
        let value = if !self.value.is_empty() {
            // Older/plaintext rows keep the value in the text column.
            self.value
        } else {
            match oscrypt::decrypt_value(key, &self.host_key, &self.encrypted_value) {
                Ok(value) => value,
                Err(CookieError::UnsupportedEncryption(_)) => return Ok(None),
                Err(error) => return Err(error),
            }
        };

        let expiry = if self.has_expires != 0 && self.expires_utc != 0 {
            Some(windows_micros_to_unix(self.expires_utc))
        } else {
            None
        };

        Ok(Some(Cookie {
            name: self.name,
            value,
            domain: self.host_key,
            path: self.path,
            secure: self.is_secure != 0,
            http_only: self.is_httponly != 0,
            expiry,
        }))
    }
}
