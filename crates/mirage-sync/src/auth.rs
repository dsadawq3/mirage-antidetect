//! Bearer API-key authentication.
//!
//! The `Authorization: Bearer <key>` header is the whole subject model: keys
//! carry a role (`admin` or `member`), there are no users or passwords. The
//! presented key is hashed and looked up in `api_keys`; a miss or a revoked
//! key is indistinguishable from the outside (401 either way).

use axum::http::{header::AUTHORIZATION, HeaderMap};

use crate::db::Db;
use crate::error::ApiError;
use crate::secrets::hash_key;

/// Role attached to an API key. Wire and storage format is lowercase
/// (`admin` / `member`), matching the DB CHECK constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Member,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Member => "member",
        }
    }

    /// Parse the storage format. Returns `None` for anything the CHECK
    /// constraint should have prevented (hand-edited databases).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "admin" => Some(Role::Admin),
            "member" => Some(Role::Member),
            _ => None,
        }
    }
}

/// Length of the immutable subject id: a prefix of `sha256(key)`.
///
/// 64 bits of the hash is plenty to distinguish team keys (collisions would
/// need ~2^32 keys) while keeping audit rows readable.
pub const KEY_ID_LEN: usize = 16;

/// The authenticated caller of a request.
#[derive(Debug, Clone)]
pub struct KeyInfo {
    /// Human-readable label. Reusable after revocation, so never use it as a
    /// stable subject — use `key_id`.
    pub label: String,
    pub role: Role,
    /// Immutable subject id (leading hex of the key hash); the value recorded
    /// in audit rows and `profiles.updated_by`.
    pub key_id: String,
}

impl KeyInfo {
    /// `Err(403)` unless the caller is an admin.
    pub fn require_admin(&self) -> Result<(), ApiError> {
        match self.role {
            Role::Admin => Ok(()),
            Role::Member => Err(ApiError::Forbidden),
        }
    }
}

/// Verify the Bearer token against the key table.
///
/// A missing header, a malformed header, an unknown key and a revoked key all
/// fail with 401 — the caller learns nothing beyond "not authenticated".
pub fn authenticate(headers: &HeaderMap, db: &Db) -> Result<KeyInfo, ApiError> {
    let header = headers
        .get(AUTHORIZATION)
        .ok_or(ApiError::Unauthorized("missing Authorization header"))?;
    let header = header
        .to_str()
        .map_err(|_| ApiError::Unauthorized("Authorization header is not valid UTF-8"))?;
    let (scheme, key) = header
        .split_once(' ')
        .ok_or(ApiError::Unauthorized("expected 'Bearer <key>'"))?;
    // Auth schemes are case-insensitive per RFC 7235; everyone sends "Bearer".
    if !scheme.eq_ignore_ascii_case("bearer") {
        return Err(ApiError::Unauthorized("expected 'Bearer <key>'"));
    }
    if key.is_empty() {
        return Err(ApiError::Unauthorized("empty bearer token"));
    }

    match db.lookup_key(&hash_key(key)) {
        Ok(Some(info)) => Ok(info),
        Ok(None) => Err(ApiError::Unauthorized("unknown or revoked API key")),
        Err(err) => Err(ApiError::from(err)),
    }
}
