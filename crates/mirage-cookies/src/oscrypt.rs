//! Windows Chromium value encryption: DPAPI-protected key + AES-256-GCM.
//!
//! Chromium on Windows keeps a 256-bit AES key in `<profile>/Local State`
//! under `os_crypt.encrypted_key`: a base64 string whose decoded bytes are
//! `"DPAPI" || CryptProtectData(key)`. Cookie values are encrypted with that
//! key as `"v10" || nonce[12] || AES-256-GCM(plaintext)`, where the GCM tag
//! is appended to the ciphertext and no additional authenticated data is
//! used.
//!
//! Schema version 24 (Chromium since 2024-08) additionally prepends a
//! 32-byte `SHA-256(domain)` to the plaintext before encryption, and verifies
//! it on read. This crate writes that prefix (so a real Chrome 156 profile
//! accepts the cookies) and strips it on read, tolerating values without it.

use std::fs;
use std::path::Path;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};

use crate::error::CookieError;

/// Chromium AES-GCM version prefix for DPAPI-backed keys.
pub const V10_PREFIX: &[u8; 3] = b"v10";
/// Chromium AES-GCM version prefix for app-bound (v20) keys.
const V20_PREFIX: &[u8; 3] = b"v20";
/// AES-GCM nonce length in bytes.
pub const NONCE_LEN: usize = 12;
/// AES-256 key length in bytes.
pub const KEY_LEN: usize = 32;
/// AES-GCM authentication tag length in bytes.
const TAG_LEN: usize = 16;
/// Length of the `SHA-256(domain)` prefix added by schema version 24.
const DOMAIN_HASH_LEN: usize = 32;
/// `Local State` file name inside a user-data-dir.
const LOCAL_STATE_FILE: &str = "Local State";
/// Prefix Chromium stores in front of the DPAPI-protected key.
const DPAPI_PREFIX: &[u8] = b"DPAPI";

/// The tiny slice of `Local State` this crate needs.
#[derive(Debug, Deserialize)]
struct LocalState {
    #[serde(default)]
    os_crypt: Option<OsCrypt>,
}

/// The `os_crypt` object in `Local State`.
#[derive(Debug, Deserialize)]
struct OsCrypt {
    #[serde(default)]
    encrypted_key: Option<String>,
}

/// Load the profile's 256-bit AES key from `<profile>/Local State`.
///
/// Reads `os_crypt.encrypted_key`, base64-decodes it, strips the `DPAPI`
/// prefix and unprotects the remainder with `CryptUnprotectData`. Returns
/// [`CookieError::NoProfileKey`] when the file or field is absent or
/// malformed, and [`CookieError::Crypto`] when DPAPI fails.
pub fn load_key(profile_dir: &Path) -> Result<[u8; KEY_LEN], CookieError> {
    let path = profile_dir.join(LOCAL_STATE_FILE);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(CookieError::NoProfileKey(format!(
                "{} not found",
                path.display()
            )));
        }
        Err(error) => return Err(CookieError::Io(error)),
    };

    let state: LocalState = serde_json::from_str(&text).map_err(|error| {
        CookieError::NoProfileKey(format!("Local State is not valid JSON: {error}"))
    })?;

    let encoded = state
        .os_crypt
        .and_then(|os_crypt| os_crypt.encrypted_key)
        .ok_or_else(|| {
            CookieError::NoProfileKey("Local State has no os_crypt.encrypted_key".to_owned())
        })?;

    let raw = BASE64.decode(encoded.trim()).map_err(|error| {
        CookieError::NoProfileKey(format!("encrypted_key is not valid base64: {error}"))
    })?;

    let blob = raw.strip_prefix(DPAPI_PREFIX).ok_or_else(|| {
        CookieError::NoProfileKey("encrypted_key does not start with the DPAPI prefix".to_owned())
    })?;

    let key = dpapi_unprotect(blob)?;
    if key.len() != KEY_LEN {
        return Err(CookieError::Crypto(format!(
            "unprotected key is {} bytes, expected {KEY_LEN}",
            key.len()
        )));
    }

    let mut out = [0u8; KEY_LEN];
    out.copy_from_slice(&key);
    Ok(out)
}

/// Encrypt a cookie value into Chromium's `encrypted_value` blob.
///
/// Produces `"v10" || nonce[12] || AES-256-GCM(SHA-256(domain) || value)`.
/// The domain hash prefix matches schema version 24.
pub fn encrypt_value(
    key: &[u8; KEY_LEN],
    domain: &str,
    value: &str,
) -> Result<Vec<u8>, CookieError> {
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| CookieError::Crypto("AES key has an invalid length".to_owned()))?;

    let mut plaintext = Vec::with_capacity(DOMAIN_HASH_LEN + value.len());
    plaintext.extend_from_slice(&domain_hash(domain));
    plaintext.extend_from_slice(value.as_bytes());

    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from(nonce_bytes);

    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_slice())
        .map_err(|_| CookieError::Crypto("AES-GCM encryption failed".to_owned()))?;

    let mut out = Vec::with_capacity(V10_PREFIX.len() + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(V10_PREFIX);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypt a Chromium `encrypted_value` blob back to its plaintext value.
///
/// Handles the `v10` scheme and strips the optional 32-byte domain hash.
/// `v20` (app-bound encryption) and unknown prefixes yield
/// [`CookieError::UnsupportedEncryption`] so callers can skip such cookies.
pub fn decrypt_value(
    key: &[u8; KEY_LEN],
    domain: &str,
    blob: &[u8],
) -> Result<String, CookieError> {
    if blob.is_empty() {
        return Ok(String::new());
    }

    if let Some(body) = blob.strip_prefix(V10_PREFIX.as_slice()) {
        return decrypt_v10(key, domain, body);
    }

    if blob.starts_with(V20_PREFIX.as_slice()) {
        return Err(CookieError::UnsupportedEncryption(
            "app-bound encryption (v20) is not supported".to_owned(),
        ));
    }

    Err(CookieError::UnsupportedEncryption(format!(
        "unrecognised encryption prefix {:02x?}",
        &blob[..blob.len().min(3)]
    )))
}

/// Decrypt the body of a `v10` blob (everything after the 3-byte prefix).
fn decrypt_v10(key: &[u8; KEY_LEN], domain: &str, body: &[u8]) -> Result<String, CookieError> {
    if body.len() < NONCE_LEN + TAG_LEN {
        return Err(CookieError::Crypto("v10 blob is truncated".to_owned()));
    }

    let (nonce_bytes, ciphertext) = body.split_at(NONCE_LEN);
    let nonce = Nonce::from_slice(nonce_bytes);

    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| CookieError::Crypto("AES key has an invalid length".to_owned()))?;

    let mut plaintext = cipher.decrypt(nonce, ciphertext).map_err(|_| {
        CookieError::Crypto("AES-GCM tag check failed (wrong key or corrupt value)".to_owned())
    })?;

    // Schema v24 prefix; older rows have no prefix and are taken verbatim.
    let expected = domain_hash(domain);
    if plaintext.len() >= DOMAIN_HASH_LEN && plaintext[..DOMAIN_HASH_LEN] == expected {
        plaintext.drain(..DOMAIN_HASH_LEN);
    }

    String::from_utf8(plaintext)
        .map_err(|_| CookieError::Crypto("decrypted value is not UTF-8".to_owned()))
}

/// `SHA-256` of a domain, the integrity prefix Chromium v24 prepends.
fn domain_hash(domain: &str) -> [u8; DOMAIN_HASH_LEN] {
    let digest = Sha256::digest(domain.as_bytes());
    let mut out = [0u8; DOMAIN_HASH_LEN];
    out.copy_from_slice(&digest);
    out
}

/// Build the exact base64 value Chromium stores in
/// `os_crypt.encrypted_key` for a raw 32-byte key (`"DPAPI"` + protected
/// blob). Useful to provision a profile or a test fixture.
pub fn local_state_encrypted_key(plain_key: &[u8]) -> Result<String, CookieError> {
    let protected = dpapi_protect(plain_key)?;
    let mut raw = Vec::with_capacity(DPAPI_PREFIX.len() + protected.len());
    raw.extend_from_slice(DPAPI_PREFIX);
    raw.extend_from_slice(&protected);
    Ok(BASE64.encode(raw))
}

/// Protect `input` with `CryptProtectData` (current user scope).
fn dpapi_protect(input: &[u8]) -> Result<Vec<u8>, CookieError> {
    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_ptr() as *mut u8,
    };
    let mut out_blob = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };

    unsafe {
        CryptProtectData(
            &in_blob,
            windows::core::PCWSTR::null(),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out_blob,
        )
        .map_err(|error| CookieError::Crypto(format!("CryptProtectData failed: {error}")))?;
    }

    // SAFETY: on success `out_blob` owns a LocalAlloc'd buffer of `cbData`
    // bytes that we copy out and then release.
    Ok(unsafe { copy_and_free(out_blob) })
}

/// Unprotect a `CryptProtectData` blob (current user scope).
fn dpapi_unprotect(input: &[u8]) -> Result<Vec<u8>, CookieError> {
    let in_blob = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_ptr() as *mut u8,
    };
    let mut out_blob = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };

    unsafe {
        CryptUnprotectData(
            &in_blob,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out_blob,
        )
        .map_err(|error| CookieError::Crypto(format!("CryptUnprotectData failed: {error}")))?;
    }

    // SAFETY: on success `out_blob` owns a LocalAlloc'd buffer of `cbData`
    // bytes that we copy out and then release.
    Ok(unsafe { copy_and_free(out_blob) })
}

/// Copy a `CRYPT_INTEGER_BLOB` payload into a `Vec` and free it with
/// `LocalFree`.
///
/// # Safety
///
/// `blob.pbData` must be either null or a valid `LocalAlloc` buffer of
/// `blob.cbData` bytes that this function may take ownership of.
unsafe fn copy_and_free(blob: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    if blob.pbData.is_null() {
        return Vec::new();
    }

    let out = std::slice::from_raw_parts(blob.pbData, blob.cbData as usize).to_vec();
    let _ = LocalFree(HLOCAL(blob.pbData as *mut core::ffi::c_void));
    out
}
