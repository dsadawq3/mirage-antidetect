//! Wire types for the REST API.
//!
//! Requests deserialize strictly enough to reject malformed input (via the
//! `400`-mapped body parser in `routes.rs`) while staying lenient about
//! optional fields. Responses carry the role as its lowercase storage form.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::auth::Role;
use crate::db::{AuditRecord, KeyRecord, ProfileRecord, ProfileSummary};

/// `POST /v1/keys` body.
#[derive(Debug, Deserialize)]
pub struct CreateKeyRequest {
    pub label: String,
    pub role: Role,
}

/// `POST /v1/keys` response. The raw key is shown here and nowhere else.
#[derive(Debug, Serialize)]
pub struct CreateKeyResponse {
    pub key: String,
}

/// One entry of `GET /v1/keys`.
#[derive(Debug, Serialize)]
pub struct KeyView {
    pub label: String,
    pub role: Role,
    pub created_at: String,
    pub revoked_at: Option<String>,
}

impl From<KeyRecord> for KeyView {
    fn from(record: KeyRecord) -> Self {
        KeyView {
            label: record.label,
            role: record.role,
            created_at: record.created_at,
            revoked_at: record.revoked_at,
        }
    }
}

/// One entry of `GET /v1/profiles`.
#[derive(Debug, Serialize)]
pub struct ProfileSummaryView {
    pub id: String,
    pub name: String,
    pub tags: Vec<String>,
    pub revision: i64,
    pub updated_at: String,
}

impl From<ProfileSummary> for ProfileSummaryView {
    fn from(row: ProfileSummary) -> Self {
        ProfileSummaryView {
            id: row.id,
            name: row.name,
            tags: row.tags,
            revision: row.revision,
            updated_at: row.updated_at,
        }
    }
}

/// `GET /v1/profiles/:id` response — the full profile.
#[derive(Debug, Serialize)]
pub struct ProfileView {
    pub id: String,
    pub name: String,
    pub tags: Vec<String>,
    pub body: Value,
    pub revision: i64,
    pub updated_at: String,
    /// Immutable key id of the last writer (not the reusable label).
    pub updated_by: String,
}

impl From<ProfileRecord> for ProfileView {
    fn from(row: ProfileRecord) -> Self {
        ProfileView {
            id: row.id,
            name: row.name,
            tags: row.tags,
            body: row.body,
            revision: row.revision,
            updated_at: row.updated_at,
            updated_by: row.updated_by,
        }
    }
}

/// `PUT /v1/profiles/:id` body.
///
/// `body` is the opaque profile JSON and is required. `name` and `tags` are
/// optional: an absent field is left unchanged on update (and defaulted to
/// empty on first insert), so a client can bump only the body without losing
/// metadata, while an explicit `""`/`[]` still clears them.
#[derive(Debug, Deserialize)]
pub struct PutProfileRequest {
    pub name: Option<String>,
    pub tags: Option<Vec<String>>,
    pub body: Value,
    pub base_revision: i64,
}

/// `PUT /v1/profiles/:id` response.
#[derive(Debug, Serialize)]
pub struct PutProfileResponse {
    pub id: String,
    pub revision: i64,
}

/// One entry of `GET /v1/audit`.
#[derive(Debug, Serialize)]
pub struct AuditView {
    pub id: i64,
    pub at: String,
    /// Immutable subject id (prefix of the key hash), not the reusable label.
    pub key_id: String,
    pub action: String,
    pub profile_id: Option<String>,
    pub detail: Option<String>,
}

impl From<AuditRecord> for AuditView {
    fn from(row: AuditRecord) -> Self {
        AuditView {
            id: row.id,
            at: row.at,
            key_id: row.key_id,
            action: row.action,
            profile_id: row.profile_id,
            detail: row.detail,
        }
    }
}

/// Query string of `GET /v1/profiles`.
#[derive(Debug, Deserialize)]
pub struct ProfileListQuery {
    pub tag: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// Query string of `GET /v1/keys`.
#[derive(Debug, Deserialize)]
pub struct KeyListQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// Query string of `GET /v1/audit`.
#[derive(Debug, Deserialize)]
pub struct AuditListQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}
