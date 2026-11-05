//! Request/response DTOs and the small helpers that keep handler bodies lean:
//! time formatting, partial-fingerprint merging and warning rendering.

use chrono::{SecondsFormat, Utc};
use mirage_core::{Fingerprint, Os, Profile, ProxyConfig, ProxyScheme};
use mirage_import::ImportWarning;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::ApiError;

/// Current UTC time, RFC 3339 with a trailing `Z` (CONTRACT date format).
pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// `POST /vault/unlock` body.
#[derive(Debug, Deserialize)]
pub struct UnlockRequest {
    pub password: String,
}

/// `POST /profiles` body.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateProfileRequest {
    pub name: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Generation seed. Absent → a random seed is drawn.
    pub seed: Option<u64>,
    /// Target OS for generation. Absent → derived from the seed.
    pub os: Option<Os>,
    pub proxy: Option<ProxyConfig>,
    /// Partial [`Fingerprint`] merged over the generated baseline.
    pub overrides: Option<Value>,
}

/// `PUT /profiles/:id` body. Every field is optional; absent fields are kept.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateProfileRequest {
    pub name: Option<String>,
    pub tags: Option<Vec<String>>,
    /// `Some(None)` clears the proxy, `Some(Some(cfg))` replaces it, `None`
    /// leaves it untouched.
    #[serde(default, deserialize_with = "double_option")]
    pub proxy: Option<Option<ProxyConfig>>,
    /// When present, the fingerprint is rebuilt from `generate(seed, os)` and
    /// any `overrides` are layered on top; the profile seed is updated.
    pub seed: Option<u64>,
    /// Partial [`Fingerprint`] merged over the current one.
    pub overrides: Option<Value>,
}

/// Distinguishes a missing field (`None`) from an explicit JSON `null`
/// (`Some(None)`) for `Option<Option<T>>`.
fn double_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// Proxy identity exposed in a [`ProfileSummary`] (no credentials).
#[derive(Debug, Serialize)]
pub struct ProxySummary {
    pub scheme: ProxyScheme,
    pub host: String,
    pub port: u16,
}

/// `GET /profiles` list item.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSummary {
    pub id: String,
    pub name: String,
    pub tags: Vec<String>,
    pub running: bool,
    pub proxy: Option<ProxySummary>,
    pub created_at: String,
    pub updated_at: String,
}

impl ProfileSummary {
    /// Build a summary from a profile plus its running flag.
    pub fn of(profile: &Profile, running: bool) -> Self {
        ProfileSummary {
            id: profile.id.clone(),
            name: profile.name.clone(),
            tags: profile.tags.clone(),
            running,
            proxy: profile.proxy.as_ref().map(|proxy| ProxySummary {
                scheme: proxy.scheme,
                host: proxy.host.clone(),
                port: proxy.port,
            }),
            created_at: profile.created_at.clone(),
            updated_at: profile.updated_at.clone(),
        }
    }
}

/// `POST /profiles/:id/cookies/import` body.
#[derive(Debug, Deserialize)]
pub struct CookiesImportRequest {
    pub format: String,
    pub data: String,
}

/// `GET /profiles/:id/cookies/export` query.
#[derive(Debug, Deserialize)]
pub struct CookiesExportQuery {
    pub format: Option<String>,
}

/// Cookie import result.
#[derive(Debug, Serialize)]
pub struct CookiesImportResponse {
    pub imported: usize,
    pub skipped: usize,
}

/// Cookie export result.
#[derive(Debug, Serialize)]
pub struct CookiesExportResponse {
    pub format: String,
    pub data: String,
}

/// `POST /import` body.
#[derive(Debug, Deserialize)]
pub struct ImportRequest {
    pub source: String,
    pub data: String,
}

/// `POST /import` response.
#[derive(Debug, Serialize)]
pub struct ImportResponse {
    pub imported: Vec<Profile>,
    pub warnings: Vec<Value>,
}

/// `POST /proxy/check` body.
#[derive(Debug, Deserialize)]
pub struct ProxyCheckRequest {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

/// Deep-merge `patch` into `base`. JSON objects merge recursively; every other
/// value (arrays included) is replaced wholesale, so an override of
/// `languages`, `fonts` or `brands` replaces the generated array.
pub fn deep_merge(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(base_map), Value::Object(patch_map)) => {
            for (key, value) in patch_map {
                deep_merge(base_map.entry(key.clone()).or_insert(Value::Null), value);
            }
        }
        (base, patch) => *base = patch.clone(),
    }
}

/// Merge a partial-fingerprint override over a generated fingerprint and
/// deserialize the result. A structurally invalid merge is a `400`.
pub fn apply_overrides(
    fingerprint: &Fingerprint,
    overrides: &Value,
) -> Result<Fingerprint, ApiError> {
    let mut base = serde_json::to_value(fingerprint)
        .map_err(|error| ApiError::Internal(format!("serialize fingerprint: {error}")))?;
    deep_merge(&mut base, overrides);
    serde_json::from_value(base).map_err(|error| {
        ApiError::InvalidRequest(format!("overrides produce an invalid fingerprint: {error}"))
    })
}

/// Render import warnings as `[{ "field": ..., "kind": "mapped"|... }]`.
pub fn warnings_to_json(warnings: &[ImportWarning]) -> Vec<Value> {
    warnings
        .iter()
        .map(|warning| {
            let kind = if warning.is_mapped() {
                "mapped"
            } else if warning.is_defaulted() {
                "defaulted"
            } else {
                "ignored"
            };
            serde_json::json!({ "field": warning.field(), "kind": kind })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_merge_replaces_arrays_and_merges_objects() {
        let mut base = serde_json::json!({
            "os": "windows",
            "languages": ["en-US", "en"],
            "screen": { "width": 1920, "height": 1080 }
        });
        let patch = serde_json::json!({
            "languages": ["fr-FR", "fr"],
            "screen": { "height": 1440 }
        });
        deep_merge(&mut base, &patch);
        assert_eq!(base["languages"], serde_json::json!(["fr-FR", "fr"]));
        assert_eq!(base["screen"]["width"], 1920);
        assert_eq!(base["screen"]["height"], 1440);
        assert_eq!(base["os"], "windows");
    }

    #[test]
    fn warnings_render_with_kind() {
        let warnings = vec![
            ImportWarning::Mapped {
                field: "name".into(),
            },
            ImportWarning::Ignored { field: "os".into() },
        ];
        let json = warnings_to_json(&warnings);
        assert_eq!(json[0]["kind"], "mapped");
        assert_eq!(json[0]["field"], "name");
        assert_eq!(json[1]["kind"], "ignored");
    }

    /// A typo in a request body must be a hard error, never silently ignored.
    #[test]
    fn request_structs_reject_unknown_fields() {
        assert!(serde_json::from_str::<CreateProfileRequest>(r#"{"name":"a"}"#).is_ok());
        assert!(serde_json::from_str::<CreateProfileRequest>(r#"{"name":"a","tagz":[]}"#).is_err());

        assert!(serde_json::from_str::<UpdateProfileRequest>(r#"{"name":"a"}"#).is_ok());
        assert!(
            serde_json::from_str::<UpdateProfileRequest>(r#"{"name":"a","unknown":1}"#).is_err()
        );
    }
}
