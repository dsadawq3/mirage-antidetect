//! Storage layer: a single SQLite file guarded by a mutex.
//!
//! The whole server is one process with one open connection, so a `Mutex` is
//! enough — there is no writer pool to coordinate. Every statement is bound
//! through `params!`, never built by string concatenation, so a profile id,
//! key label or key id can never change the shape of a query.
//!
//! Profile bodies are opaque JSON: `mirage-sync` deliberately does not depend
//! on `mirage-core`, and profile validation happens on the client. Bodies are
//! stored verbatim in plaintext mode and as an AES-256-GCM envelope when a key
//! is configured (see [`crate::crypto`]).
//!
//! Every mutating operation takes the write and its audit line in one
//! transaction, so a failed write never leaves an audit trail claiming it
//! happened and a successful one is always recorded.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rand::RngCore;
use rusqlite::{params, Connection, OptionalExtension};

use crate::auth::{KeyInfo, Role, KEY_ID_LEN};
use crate::crypto::{self, BodyCipher, CryptoError, KeyConfig};

/// Default number of audit rows to keep. Older rows are pruned as new ones are
/// appended so the table cannot grow without bound.
pub const AUDIT_RETENTION_DEFAULT: i64 = 10_000;

/// `meta` key holding the hex-encoded Argon2id salt for passphrase keys.
const META_KDF_SALT: &str = "kdf_salt";

/// Schema created on startup. `IF NOT EXISTS` makes the migration idempotent;
/// there is nothing to version yet, so a plain batch is enough.
///
/// `api_keys.label` is unique only among *active* keys (partial index on
/// `revoked_at IS NULL`): revoking a key frees its label for reuse while the
/// historical row survives as an audit trail. Because a label is reusable it
/// is *not* a stable subject: audit rows and `profiles.updated_by` record the
/// immutable `key_id` (a prefix of `sha256(key)`) instead.
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS api_keys (
    key_hash   TEXT PRIMARY KEY,          -- sha256(key) as lowercase hex, never the raw key
    label      TEXT NOT NULL,
    role       TEXT NOT NULL CHECK (role IN ('admin', 'member')),
    created_at TEXT NOT NULL,             -- RFC 3339 UTC
    revoked_at TEXT NULL                  -- NULL while the key is usable
);
CREATE UNIQUE INDEX IF NOT EXISTS api_keys_label_active
    ON api_keys(label) WHERE revoked_at IS NULL;

CREATE TABLE IF NOT EXISTS profiles (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    tags_json  TEXT NOT NULL,
    body_json  TEXT NOT NULL,             -- opaque JSON, encrypted envelope when a key is set
    revision   INTEGER NOT NULL,
    updated_at TEXT NOT NULL,             -- RFC 3339 UTC
    updated_by TEXT NOT NULL              -- immutable key id that wrote the row
);

CREATE TABLE IF NOT EXISTS audit (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    at         TEXT NOT NULL,             -- RFC 3339 UTC
    key_id     TEXT NOT NULL,             -- immutable prefix of sha256(key), never the reusable label
    action     TEXT NOT NULL,
    profile_id TEXT NULL,
    detail     TEXT NULL
);

CREATE TABLE IF NOT EXISTS meta (
    k TEXT PRIMARY KEY,
    v TEXT NOT NULL
);
"#;

/// Everything the storage layer can fail with. `error.rs` maps each variant
/// onto an HTTP status and a JSON body.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("key label is already used by an active key")]
    LabelTaken,

    #[error("profile revision conflict")]
    RevisionConflict { current_revision: i64 },

    #[error("api key {0:?} carries a role outside the CHECK constraint")]
    CorruptRole(String),

    #[error("invalid configuration: {0}")]
    Config(String),

    #[error("crypto error: {0}")]
    Crypto(#[from] CryptoError),

    #[error("storage error: {0}")]
    Internal(String),

    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// How to open a database: which key to encrypt bodies with and how many audit
/// rows to retain.
#[derive(Debug, Clone)]
pub struct DbConfig {
    pub key: KeyConfig,
    pub audit_retention: i64,
}

impl DbConfig {
    /// Read the key configuration from the environment.
    pub fn from_env() -> Result<Self, DbError> {
        Ok(DbConfig {
            key: KeyConfig::from_env().map_err(DbError::from)?,
            audit_retention: AUDIT_RETENTION_DEFAULT,
        })
    }

    /// Plaintext storage, default retention — used by tests and callers that
    /// build the config explicitly.
    pub fn plaintext() -> Self {
        DbConfig {
            key: KeyConfig::Plaintext,
            audit_retention: AUDIT_RETENTION_DEFAULT,
        }
    }
}

/// A row of `api_keys` as returned to admins. The hash never leaves the db.
#[derive(Debug, Clone)]
pub struct KeyRecord {
    pub label: String,
    pub role: Role,
    pub created_at: String,
    pub revoked_at: Option<String>,
}

/// Row shape for `GET /v1/profiles` — no body, just enough to list and sync.
#[derive(Debug, Clone)]
pub struct ProfileSummary {
    pub id: String,
    pub name: String,
    pub tags: Vec<String>,
    pub revision: i64,
    pub updated_at: String,
}

/// Full profile row, body included.
#[derive(Debug, Clone)]
pub struct ProfileRecord {
    pub id: String,
    pub name: String,
    pub tags: Vec<String>,
    pub body: serde_json::Value,
    pub revision: i64,
    pub updated_at: String,
    pub updated_by: String,
}

/// Input of [`Db::put_profile_audited`].
///
/// `name` and `tags` are optional so a partial update can change only the body
/// (or only the name) without clobbering the columns it did not send: `None`
/// leaves the existing value untouched on update and uses the empty default on
/// insert.
pub struct ProfileWrite<'a> {
    pub id: &'a str,
    pub name: Option<&'a str>,
    pub tags: Option<&'a [String]>,
    pub body: &'a serde_json::Value,
    pub base_revision: i64,
    pub updated_by: &'a str,
    pub now: &'a str,
}

/// One audit entry.
#[derive(Debug, Clone)]
pub struct AuditRecord {
    pub id: i64,
    pub at: String,
    pub key_id: String,
    pub action: String,
    pub profile_id: Option<String>,
    pub detail: Option<String>,
}

/// Raw `profiles` row before `tags_json`/`body_json` are decoded.
struct RawProfile {
    id: String,
    name: String,
    tags_json: String,
    body_json: String,
    revision: i64,
    updated_at: String,
    updated_by: String,
}

impl RawProfile {
    fn into_record(self, cipher: &BodyCipher) -> Result<ProfileRecord, DbError> {
        let tags = decode_tags(&self.tags_json)?;
        let plaintext = cipher.decrypt(&self.body_json)?;
        let body: serde_json::Value = serde_json::from_str(&plaintext)
            .map_err(|e| DbError::Internal(format!("stored body is not valid JSON: {e}")))?;
        Ok(ProfileRecord {
            id: self.id,
            name: self.name,
            tags,
            body,
            revision: self.revision,
            updated_at: self.updated_at,
            updated_by: self.updated_by,
        })
    }
}

/// Decode a `tags_json` column into a tag vector.
fn decode_tags(tags_json: &str) -> Result<Vec<String>, DbError> {
    serde_json::from_str::<Vec<String>>(tags_json)
        .map_err(|e| DbError::Internal(format!("stored tags_json is not a string array: {e}")))
}

/// Current wall clock as RFC 3339 UTC with a trailing `Z` and second precision.
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// The database handle.
pub struct Db {
    conn: Mutex<Connection>,
    cipher: BodyCipher,
    audit_retention: i64,
}

impl Db {
    /// Open (creating if missing) the SQLite file using environment config.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DbError> {
        Self::open_with_config(path, DbConfig::from_env()?)
    }

    /// Open (creating if missing) the SQLite file with an explicit config.
    ///
    /// Runs migrations, resolves the body cipher (creating the Argon2id salt
    /// on first use for a passphrase key) and builds the handle.
    pub fn open_with_config(path: impl AsRef<Path>, config: DbConfig) -> Result<Self, DbError> {
        let conn = Connection::open(path)?;
        // WAL keeps readers from blocking the single writer; the busy timeout
        // covers the rare case of two connections to the same file.
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;\
             PRAGMA busy_timeout = 5000;\
             PRAGMA foreign_keys = ON;",
        )?;
        conn.execute_batch(SCHEMA)?;
        migrate_audit_subject(&conn)?;

        let cipher = resolve_cipher(&config.key, &conn)?;

        Ok(Db {
            conn: Mutex::new(conn),
            cipher,
            audit_retention: config.audit_retention,
        })
    }

    /// Whether profile bodies are encrypted at rest.
    pub fn is_encrypted(&self) -> bool {
        self.cipher.is_encrypted()
    }

    /// Acquire the connection guard.
    ///
    /// A poisoned mutex means a handler panicked while holding the lock; we
    /// recover the guard rather than aborting the process, and let SQLite
    /// report any half-finished transaction on the next statement.
    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Bootstrap the first admin key if the key table is empty.
    ///
    /// When `provided` is `Some`, that operator-supplied key is installed and
    /// never returned (so the caller cannot log it); malformed short values are
    /// rejected. When `provided` is `None`, a fresh key is generated and
    /// returned so the caller can show it exactly once. Returns `None` when the
    /// key table was already non-empty.
    pub fn ensure_bootstrap(&self, provided: Option<&str>) -> Result<Option<String>, DbError> {
        if let Some(key) = provided {
            if key.trim().len() < 16 {
                return Err(DbError::Config(
                    "MIRAGE_SYNC_BOOTSTRAP_KEY must be at least 16 characters".to_owned(),
                ));
            }
        }

        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let existing: i64 = tx.query_row("SELECT COUNT(*) FROM api_keys", [], |r| r.get(0))?;
        if existing > 0 {
            // Dropping the transaction rolls back; nothing was written.
            return Ok(None);
        }

        let (key, generated) = match provided {
            Some(key) => (key.to_owned(), false),
            None => (crate::secrets::generate_key(), true),
        };
        let hash = crate::secrets::hash_key(&key);
        let now = now_rfc3339();
        tx.execute(
            "INSERT INTO api_keys (key_hash, label, role, created_at, revoked_at) \
             VALUES (?1, 'bootstrap', 'admin', ?2, NULL)",
            params![hash, now],
        )?;
        audit_insert(
            &tx,
            &key_id_from_hash(&hash),
            "key.bootstrap",
            None,
            None,
            &now,
            self.audit_retention,
        )?;
        tx.commit()?;
        Ok(if generated { Some(key) } else { None })
    }

    /// Resolve a presented key hash to its subject. Revoked keys are invisible
    /// here, which is what makes them fail authentication.
    pub fn lookup_key(&self, key_hash: &str) -> Result<Option<KeyInfo>, DbError> {
        let conn = self.lock();
        let row = conn
            .query_row(
                "SELECT label, role FROM api_keys WHERE key_hash = ?1 AND revoked_at IS NULL",
                params![key_hash],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?;
        match row {
            None => Ok(None),
            Some((label, role)) => {
                let role = Role::parse(&role).ok_or_else(|| DbError::CorruptRole(label.clone()))?;
                Ok(Some(KeyInfo {
                    label,
                    role,
                    key_id: key_id_from_hash(key_hash),
                }))
            }
        }
    }

    /// Insert a new key and its audit line in one transaction. A label already
    /// held by an *active* key is rejected via the partial unique index and
    /// surfaced as `LabelTaken` (HTTP 409).
    pub fn create_key_audited(
        &self,
        label: &str,
        role: Role,
        key_hash: &str,
        created_at: &str,
        subject: &str,
    ) -> Result<(), DbError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let result = tx.execute(
            "INSERT INTO api_keys (key_hash, label, role, created_at, revoked_at) \
             VALUES (?1, ?2, ?3, ?4, NULL)",
            params![key_hash, label, role.as_str(), created_at],
        );
        match result {
            Ok(_) => {}
            Err(rusqlite::Error::SqliteFailure(err, _))
                if err.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                return Err(DbError::LabelTaken);
            }
            Err(err) => return Err(DbError::Sqlite(err)),
        }
        audit_insert(
            &tx,
            subject,
            "key.create",
            None,
            Some(&format!("label={label} role={}", role.as_str())),
            created_at,
            self.audit_retention,
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Soft-delete the active key with this label and record the audit line in
    /// one transaction. `false` when nothing matched (no audit line is written).
    pub fn revoke_key_audited(
        &self,
        label: &str,
        at: &str,
        subject: &str,
    ) -> Result<bool, DbError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let changed = tx.execute(
            "UPDATE api_keys SET revoked_at = ?2 WHERE label = ?1 AND revoked_at IS NULL",
            params![label, at],
        )?;
        if changed == 0 {
            return Ok(false);
        }
        audit_insert(
            &tx,
            subject,
            "key.revoke",
            None,
            Some(&format!("label={label}")),
            at,
            self.audit_retention,
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Keys, newest first, revoked ones included, paginated.
    pub fn list_keys(&self, limit: i64, offset: i64) -> Result<Vec<KeyRecord>, DbError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT label, role, created_at, revoked_at FROM api_keys \
             ORDER BY created_at DESC, label ASC LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt.query_map(params![limit, offset], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?;
        let mut keys = Vec::new();
        for row in rows {
            let (label, role, created_at, revoked_at) = row?;
            let role = Role::parse(&role).ok_or_else(|| DbError::CorruptRole(label.clone()))?;
            keys.push(KeyRecord {
                label,
                role,
                created_at,
                revoked_at,
            });
        }
        Ok(keys)
    }

    /// Profiles, optionally restricted to those carrying `tag`, paginated.
    pub fn list_profiles(
        &self,
        tag: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<ProfileSummary>, DbError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, name, tags_json, revision, updated_at FROM profiles \
             WHERE (?1 IS NULL OR EXISTS ( \
                 SELECT 1 FROM json_each(profiles.tags_json) WHERE json_each.value = ?1 \
             )) \
             ORDER BY updated_at DESC, id ASC LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt.query_map(params![tag, limit, offset], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, name, tags_json, revision, updated_at) = row?;
            out.push(ProfileSummary {
                id,
                name,
                tags: decode_tags(&tags_json)?,
                revision,
                updated_at,
            });
        }
        Ok(out)
    }

    /// Fetch one full profile.
    pub fn get_profile(&self, id: &str) -> Result<Option<ProfileRecord>, DbError> {
        let conn = self.lock();
        let raw = conn
            .query_row(
                "SELECT id, name, tags_json, body_json, revision, updated_at, updated_by \
                 FROM profiles WHERE id = ?1",
                params![id],
                |r| {
                    Ok(RawProfile {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        tags_json: r.get(2)?,
                        body_json: r.get(3)?,
                        revision: r.get(4)?,
                        updated_at: r.get(5)?,
                        updated_by: r.get(6)?,
                    })
                },
            )
            .optional()?;
        match raw {
            None => Ok(None),
            Some(raw) => Ok(Some(raw.into_record(&self.cipher)?)),
        }
    }

    /// Optimistic-concurrency write and its audit line in one transaction.
    ///
    /// `base_revision` must match the stored revision (`0` when the profile is
    /// new) or the write conflicts and returns the true revision so the client
    /// can rebase. On success the revision becomes `current + 1` and both the
    /// write and the audit line commit together: a conflict or storage error
    /// leaves no trace in `audit`.
    pub fn put_profile_audited(
        &self,
        write: ProfileWrite<'_>,
        subject: &str,
    ) -> Result<i64, DbError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;

        let current: i64 = tx
            .query_row(
                "SELECT revision FROM profiles WHERE id = ?1",
                params![write.id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);

        if write.base_revision != current {
            return Err(DbError::RevisionConflict {
                current_revision: current,
            });
        }

        let new_revision = current + 1;
        let tags_json = match write.tags {
            Some(tags) => Some(
                serde_json::to_string(tags)
                    .map_err(|e| DbError::Internal(format!("failed to encode tags: {e}")))?,
            ),
            None => None,
        };
        let body_json = self.cipher.encrypt(
            &serde_json::to_string(write.body)
                .map_err(|e| DbError::Internal(format!("failed to encode body: {e}")))?,
        )?;

        // COALESCE keeps the existing name/tags when the parameter is NULL
        // (field absent), while the insert path supplies the empty defaults.
        tx.execute(
            "INSERT INTO profiles (id, name, tags_json, body_json, revision, updated_at, updated_by) \
             VALUES (?1, COALESCE(?2, ''), COALESCE(?3, '[]'), ?4, ?5, ?6, ?7) \
             ON CONFLICT(id) DO UPDATE SET \
                 name = COALESCE(?2, profiles.name), \
                 tags_json = COALESCE(?3, profiles.tags_json), \
                 body_json = excluded.body_json, \
                 revision = excluded.revision, \
                 updated_at = excluded.updated_at, \
                 updated_by = excluded.updated_by",
            params![
                write.id,
                write.name,
                tags_json,
                body_json,
                new_revision,
                write.now,
                write.updated_by
            ],
        )?;

        audit_insert(
            &tx,
            subject,
            "profile.put",
            Some(write.id),
            Some(&format!("revision={new_revision}")),
            write.now,
            self.audit_retention,
        )?;
        tx.commit()?;
        Ok(new_revision)
    }

    /// Hard-delete a profile and its audit line in one transaction. `false`
    /// when the id was not present (no audit line is written).
    pub fn delete_profile_audited(
        &self,
        id: &str,
        at: &str,
        subject: &str,
    ) -> Result<bool, DbError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let changed = tx.execute("DELETE FROM profiles WHERE id = ?1", params![id])?;
        if changed == 0 {
            return Ok(false);
        }
        audit_insert(
            &tx,
            subject,
            "profile.delete",
            Some(id),
            None,
            at,
            self.audit_retention,
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Newest audit entries first, paginated.
    pub fn list_audit(&self, limit: i64, offset: i64) -> Result<Vec<AuditRecord>, DbError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, at, key_id, action, profile_id, detail FROM audit \
             ORDER BY id DESC LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt.query_map(params![limit, offset], |r| {
            Ok(AuditRecord {
                id: r.get(0)?,
                at: r.get(1)?,
                key_id: r.get(2)?,
                action: r.get(3)?,
                profile_id: r.get(4)?,
                detail: r.get(5)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}

/// Append an audit line and prune everything older than the retention window,
/// so the table cannot grow without bound. Runs inside the caller's transaction.
fn audit_insert(
    conn: &Connection,
    subject: &str,
    action: &str,
    profile_id: Option<&str>,
    detail: Option<&str>,
    at: &str,
    retention: i64,
) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO audit (at, key_id, action, profile_id, detail) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![at, subject, action, profile_id, detail],
    )?;

    if retention > 0 {
        // Keep the newest `retention` rows: delete anything older than the
        // `retention`-th newest. With fewer rows the subquery is NULL and
        // nothing is deleted.
        conn.execute(
            "DELETE FROM audit WHERE id < ( \
                 SELECT id FROM audit ORDER BY id DESC LIMIT 1 OFFSET ?1 \
             )",
            params![retention - 1],
        )?;
    }
    Ok(())
}

/// Resolve the configured key into a cipher, creating the per-database
/// Argon2id salt on first use when the value is a passphrase.
fn resolve_cipher(key: &KeyConfig, conn: &Connection) -> Result<BodyCipher, DbError> {
    match key {
        KeyConfig::Plaintext => Ok(BodyCipher::Plaintext),
        KeyConfig::Hex(raw) => Ok(BodyCipher::Encrypted { key: *raw }),
        KeyConfig::Passphrase(passphrase) => {
            let salt = read_or_create_salt(conn)?;
            Ok(BodyCipher::Encrypted {
                key: crypto::derive_key(passphrase, &salt),
            })
        }
    }
}

/// Read the hex-encoded Argon2id salt from `meta`, generating a random 16-byte
/// value on first use.
fn read_or_create_salt(conn: &Connection) -> Result<[u8; 16], DbError> {
    let existing: Option<String> = conn
        .query_row(
            "SELECT v FROM meta WHERE k = ?1",
            params![META_KDF_SALT],
            |r| r.get(0),
        )
        .optional()?;

    match existing {
        Some(encoded) => {
            let bytes = hex::decode(&encoded)
                .map_err(|e| DbError::Config(format!("stored KDF salt is not hex: {e}")))?;
            if bytes.len() != 16 {
                return Err(DbError::Config(format!(
                    "stored KDF salt must be 16 bytes, found {}",
                    bytes.len()
                )));
            }
            let mut salt = [0u8; 16];
            salt.copy_from_slice(&bytes);
            Ok(salt)
        }
        None => {
            let mut salt = [0u8; 16];
            rand::rngs::OsRng.fill_bytes(&mut salt);
            conn.execute(
                "INSERT INTO meta (k, v) VALUES (?1, ?2)",
                params![META_KDF_SALT, hex::encode(salt)],
            )?;
            Ok(salt)
        }
    }
}

/// Rename the legacy `audit.key_label` column to `key_id` on databases created
/// before the subject changed from a reusable label to an immutable id.
fn migrate_audit_subject(conn: &Connection) -> Result<(), DbError> {
    if table_has_column(conn, "audit", "key_label")? && !table_has_column(conn, "audit", "key_id")?
    {
        conn.execute_batch("ALTER TABLE audit RENAME COLUMN key_label TO key_id;")?;
    }
    Ok(())
}

/// Whether `table` has a column named `column` (`table` is a trusted literal).
fn table_has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, DbError> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Derive the immutable subject id from a key hash: its leading hex characters.
fn key_id_from_hash(key_hash: &str) -> String {
    key_hash.chars().take(KEY_ID_LEN).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A db backed by a scratch dir that lives as long as the tempdir binding.
    fn temp_db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db =
            Db::open_with_config(dir.path().join("t.db"), DbConfig::plaintext()).expect("open db");
        (dir, db)
    }

    fn write<'a>(
        id: &'a str,
        body: &'a serde_json::Value,
        base: i64,
        now: &'a str,
    ) -> ProfileWrite<'a> {
        ProfileWrite {
            id,
            name: Some("n"),
            tags: Some(&[]),
            body,
            base_revision: base,
            updated_by: "keystub",
            now,
        }
    }

    #[test]
    fn bootstrap_only_creates_once() {
        let (_dir, db) = temp_db();
        let first = db.ensure_bootstrap(None).expect("bootstrap");
        assert!(first.is_some());
        assert!(db.ensure_bootstrap(None).expect("second").is_none());
    }

    #[test]
    fn provided_bootstrap_key_is_installed_and_not_returned() {
        let (_dir, db) = temp_db();
        let provided = "operator-supplied-admin-key-0123456789";
        assert!(db
            .ensure_bootstrap(Some(provided))
            .expect("bootstrap")
            .is_none());
        let info = db
            .lookup_key(&crate::secrets::hash_key(provided))
            .expect("lookup")
            .expect("provided key must be active");
        assert_eq!(info.role, Role::Admin);
    }

    #[test]
    fn short_bootstrap_key_is_rejected() {
        let (_dir, db) = temp_db();
        let err = db.ensure_bootstrap(Some("short")).expect_err("reject");
        assert!(matches!(err, DbError::Config(_)));
    }

    #[test]
    fn revision_conflict_reports_current() {
        let (_dir, db) = temp_db();
        let body = serde_json::json!({ "k": 1 });
        let now = now_rfc3339();
        let rev = db
            .put_profile_audited(write("p", &body, 0, &now), "subject")
            .expect("create");
        assert_eq!(rev, 1);
        let err = db
            .put_profile_audited(write("p", &body, 99, &now), "subject")
            .expect_err("stale base revision must conflict");
        match err {
            DbError::RevisionConflict { current_revision } => assert_eq!(current_revision, 1),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn conflict_leaves_no_audit_line() {
        let (_dir, db) = temp_db();
        let body = serde_json::json!({ "k": 1 });
        let now = now_rfc3339();
        db.put_profile_audited(write("p", &body, 0, &now), "subject")
            .expect("create");
        let _ = db.put_profile_audited(write("p", &body, 7, &now), "subject");
        let audit = db.list_audit(100, 0).expect("audit");
        // One bootstrap is not recorded here (no bootstrap called), so exactly
        // the single successful put must be present.
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].action, "profile.put");
    }

    #[test]
    fn partial_update_preserves_absent_fields() {
        let (_dir, db) = temp_db();
        let now = now_rfc3339();
        let body1 = serde_json::json!({ "v": 1 });
        db.put_profile_audited(
            ProfileWrite {
                id: "p",
                name: Some("shop"),
                tags: Some(&["work".to_owned()]),
                body: &body1,
                base_revision: 0,
                updated_by: "subject",
                now: &now,
            },
            "subject",
        )
        .expect("create");

        let body2 = serde_json::json!({ "v": 2 });
        db.put_profile_audited(
            ProfileWrite {
                id: "p",
                name: None,
                tags: None,
                body: &body2,
                base_revision: 1,
                updated_by: "subject",
                now: &now,
            },
            "subject",
        )
        .expect("update body only");

        let rec = db.get_profile("p").expect("get").expect("exists");
        assert_eq!(rec.name, "shop");
        assert_eq!(rec.tags, vec!["work".to_owned()]);
        assert_eq!(rec.body, body2);
    }

    #[test]
    fn audit_retention_prunes_old_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Db::open_with_config(
            dir.path().join("t.db"),
            DbConfig {
                key: KeyConfig::Plaintext,
                audit_retention: 3,
            },
        )
        .expect("open");
        let now = now_rfc3339();
        for i in 0..6 {
            let body = serde_json::json!({ "i": i });
            db.put_profile_audited(write("p", &body, i, &now), "subject")
                .expect("put");
        }
        assert_eq!(db.list_audit(100, 0).expect("audit").len(), 3);
    }

    #[test]
    fn label_is_only_unique_while_active() {
        let (_dir, db) = temp_db();
        let now = now_rfc3339();
        db.create_key_audited("ops", Role::Member, "hash-1", &now, "s")
            .expect("first");
        let dup = db.create_key_audited("ops", Role::Member, "hash-2", &now, "s");
        assert!(matches!(dup, Err(DbError::LabelTaken)));
        assert!(db.revoke_key_audited("ops", &now, "s").expect("revoke"));
        // Reusing a freed label is allowed.
        db.create_key_audited("ops", Role::Admin, "hash-3", &now, "s")
            .expect("reuse after revoke");
    }
}
