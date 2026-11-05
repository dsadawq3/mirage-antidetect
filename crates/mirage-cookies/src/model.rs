//! The portable cookie model shared by every format in this crate.

use serde::{Deserialize, Serialize};

/// A single HTTP cookie, independent of any storage format.
///
/// This is the JSON shape used by [`crate::import_json`] / [`crate::export_json`]
/// (serde `camelCase`). `domain` is stored exactly as a Chromium `host_key`:
/// domain cookies carry a leading dot (`.example.com`), host-only cookies do
/// not (`example.com`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cookie {
    /// Cookie name.
    #[serde(default)]
    pub name: String,

    /// Cookie value (plaintext).
    #[serde(default)]
    pub value: String,

    /// Cookie domain / Chromium `host_key`.
    #[serde(default)]
    pub domain: String,

    /// Path scope.
    #[serde(default)]
    pub path: String,

    /// `Secure` attribute.
    #[serde(default)]
    pub secure: bool,

    /// `HttpOnly` attribute.
    #[serde(default)]
    pub http_only: bool,

    /// Expiry as a Unix timestamp in seconds, or `None` for a session cookie.
    #[serde(default)]
    pub expiry: Option<i64>,
}
