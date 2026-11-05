//! Optional at-rest encryption for stored profile bodies.
//!
//! A profile body is an opaque JSON blob held in the `profiles.body_json`
//! TEXT column. By default it is stored as plaintext, which is only safe when
//! the database file itself is protected (full-disk encryption, restricted
//! filesystem permissions). Setting `MIRAGE_SYNC_KEY` turns on encryption of
//! every body written from that point on:
//!
//! * a value of exactly 64 hex characters is used verbatim as the 256-bit key;
//! * any other value is treated as a passphrase and stretched with Argon2id
//!   over a per-database random salt kept in the `meta` table.
//!
//! The stored envelope is `"enc:v1:" || hex(nonce[12] || ciphertext||tag)`.
//! A plaintext row can never be mistaken for an envelope because a JSON
//! document never begins with `enc:v1:`.
//!
//! Rows written before the key was configured stay plaintext and remain
//! readable: reads pass plaintext through untouched and writes re-encrypt.
//! Once a key is set, a row that *is* an envelope but cannot be opened (wrong
//! key, corruption) is a hard error — the server fails closed.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use rand::RngCore;

/// AES-256-GCM nonce length in bytes (96 bits).
pub const NONCE_LEN: usize = 12;
/// AES-256-GCM authentication tag length in bytes.
pub const TAG_LEN: usize = 16;
/// Prefix marking an encrypted body. Never produced by `serde_json`.
pub const ENVELOPE_PREFIX: &str = "enc:v1:";

/// AES-256 key length in bytes.
const KEY_LEN: usize = 32;
/// Argon2id memory cost: 19456 KiB (19 MiB) — OWASP-recommended minimum.
const ARGON2_M_COST: u32 = 19456;
/// Argon2id time cost (number of passes): 2 — OWASP-recommended minimum.
const ARGON2_T_COST: u32 = 2;
/// Argon2id parallelism (lanes): 1.
const ARGON2_P_COST: u32 = 1;

/// Failures of the encryption layer.
#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("MIRAGE_SYNC_KEY looks like a hex key but is not exactly 64 hex characters")]
    BadHexKey,

    #[error("database holds an encrypted body but MIRAGE_SYNC_KEY is not set")]
    MissingKey,

    #[error("stored body has a malformed encrypted envelope")]
    BadEnvelope,

    #[error("body decryption failed (wrong MIRAGE_SYNC_KEY or corrupt row)")]
    Decrypt,

    #[error("AES-256-GCM is unavailable for the configured key length")]
    Cipher,
}

/// Operator-configured key material, before the per-database salt is known.
#[derive(Debug, Clone)]
pub enum KeyConfig {
    /// No `MIRAGE_SYNC_KEY`: bodies are stored as plaintext.
    Plaintext,
    /// `MIRAGE_SYNC_KEY` decoded to a raw 256-bit key.
    Hex([u8; 32]),
    /// `MIRAGE_SYNC_KEY` taken as a passphrase; combined with the salt later.
    Passphrase(String),
}

impl KeyConfig {
    /// Read the configuration from the process environment.
    ///
    /// An unset or empty `MIRAGE_SYNC_KEY` means plaintext storage.
    pub fn from_env() -> Result<Self, CryptoError> {
        match std::env::var("MIRAGE_SYNC_KEY") {
            Ok(value) if !value.is_empty() => Self::parse(&value),
            _ => Ok(KeyConfig::Plaintext),
        }
    }

    /// Classify a raw `MIRAGE_SYNC_KEY` value.
    ///
    /// Exactly 64 hex characters is a raw key; everything else is a
    /// passphrase (including a 64-character non-hex value).
    pub fn parse(value: &str) -> Result<Self, CryptoError> {
        let looks_hex = value.len() == KEY_LEN * 2 && value.bytes().all(|b| b.is_ascii_hexdigit());
        if looks_hex {
            let bytes = hex::decode(value).map_err(|_| CryptoError::BadHexKey)?;
            let mut key = [0u8; KEY_LEN];
            key.copy_from_slice(&bytes);
            Ok(KeyConfig::Hex(key))
        } else {
            Ok(KeyConfig::Passphrase(value.to_owned()))
        }
    }
}

/// The resolved body cipher held by the storage layer.
#[derive(Clone)]
pub enum BodyCipher {
    /// Bodies are stored exactly as serialized.
    Plaintext,
    /// Bodies are wrapped in an AES-256-GCM envelope.
    Encrypted { key: [u8; KEY_LEN] },
}

// The key must never leak through Debug or other formatting.
impl std::fmt::Debug for BodyCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BodyCipher::Plaintext => f.write_str("BodyCipher::Plaintext"),
            BodyCipher::Encrypted { .. } => f.write_str("BodyCipher::Encrypted(..)"),
        }
    }
}

impl BodyCipher {
    /// Whether bodies are being encrypted.
    pub fn is_encrypted(&self) -> bool {
        matches!(self, BodyCipher::Encrypted { .. })
    }

    /// Wrap a serialized JSON body for storage.
    ///
    /// In plaintext mode the input is returned unchanged; in encrypted mode
    /// the result is an `enc:v1:` envelope.
    pub fn encrypt(&self, plaintext: &str) -> Result<String, CryptoError> {
        match self {
            BodyCipher::Plaintext => Ok(plaintext.to_owned()),
            BodyCipher::Encrypted { key } => {
                let mut nonce = [0u8; NONCE_LEN];
                rand::thread_rng().fill_bytes(&mut nonce);

                let cipher = cipher_for(key)?;
                let ciphertext = cipher
                    .encrypt(Nonce::from_slice(&nonce), plaintext.as_bytes())
                    .map_err(|_| CryptoError::Decrypt)?;

                let mut envelope = Vec::with_capacity(NONCE_LEN + ciphertext.len());
                envelope.extend_from_slice(&nonce);
                envelope.extend_from_slice(&ciphertext);
                Ok(format!("{ENVELOPE_PREFIX}{}", hex::encode(envelope)))
            }
        }
    }

    /// Unwrap a stored body.
    ///
    /// A row without the envelope prefix is returned verbatim (legacy
    /// plaintext). A row with the prefix must decrypt or the call fails.
    pub fn decrypt(&self, stored: &str) -> Result<String, CryptoError> {
        let Some(hex_part) = stored.strip_prefix(ENVELOPE_PREFIX) else {
            return Ok(stored.to_owned());
        };

        let BodyCipher::Encrypted { key } = self else {
            return Err(CryptoError::MissingKey);
        };

        let envelope = hex::decode(hex_part).map_err(|_| CryptoError::BadEnvelope)?;
        if envelope.len() < NONCE_LEN + TAG_LEN {
            return Err(CryptoError::BadEnvelope);
        }
        let (nonce, ciphertext) = envelope.split_at(NONCE_LEN);

        let plaintext = cipher_for(key)?
            .decrypt(Nonce::from_slice(nonce), ciphertext)
            .map_err(|_| CryptoError::Decrypt)?;

        String::from_utf8(plaintext).map_err(|_| CryptoError::BadEnvelope)
    }
}

/// Derive the 256-bit key from a passphrase and the per-database salt.
///
/// Argon2id with the OWASP minimum parameter set. The salt is a random
/// 16-byte value persisted in the `meta` table; the passphrase itself is
/// never written anywhere.
pub fn derive_key(passphrase: &str, salt: &[u8; 16]) -> [u8; KEY_LEN] {
    let mut key = [0u8; KEY_LEN];

    // `Params::new` only rejects out-of-range costs; the constants above are
    // static and valid, and `Params::default()` carries the same OWASP
    // values, so the fallback cannot change the derivation.
    let params = Params::new(ARGON2_M_COST, ARGON2_T_COST, ARGON2_P_COST, Some(KEY_LEN))
        .unwrap_or_else(|_| Params::default());

    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

    // Cannot fail with valid params, a 16-byte salt and a 32-byte output
    // buffer. Should it ever fail, the key stays zeroed and every later GCM
    // open fails its tag check: the store fails closed.
    let _ = argon2.hash_password_into(passphrase.as_bytes(), salt, &mut key);

    key
}

/// Build the AES-256-GCM cipher for a key. Only fails for a non-32-byte key,
/// which the fixed `[u8; 32]` type rules out.
fn cipher_for(key: &[u8; KEY_LEN]) -> Result<Aes256Gcm, CryptoError> {
    Aes256Gcm::new_from_slice(key).map_err(|_| CryptoError::Cipher)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_A: [u8; 32] = [0x11; 32];
    const KEY_B: [u8; 32] = [0x22; 32];

    #[test]
    fn parse_classifies_hex_and_passphrase() {
        let hex_key = "a".repeat(64);
        assert!(matches!(KeyConfig::parse(&hex_key), Ok(KeyConfig::Hex(_))));
        assert!(matches!(
            KeyConfig::parse("correct horse battery staple"),
            Ok(KeyConfig::Passphrase(_))
        ));
        // 64 chars but not hex -> passphrase, not an error.
        let not_hex = "z".repeat(64);
        assert!(matches!(
            KeyConfig::parse(&not_hex),
            Ok(KeyConfig::Passphrase(_))
        ));
        // hex-looking but wrong length -> passphrase.
        assert!(matches!(
            KeyConfig::parse(&"ab".repeat(10)),
            Ok(KeyConfig::Passphrase(_))
        ));
    }

    #[test]
    fn plaintext_cipher_round_trips_verbatim() {
        let cipher = BodyCipher::Plaintext;
        let body = r#"{"proxy":{"password":"hunter2"}}"#;
        let stored = cipher.encrypt(body).expect("encrypt");
        assert_eq!(stored, body);
        assert_eq!(cipher.decrypt(&stored).expect("decrypt"), body);
    }

    #[test]
    fn encrypted_cipher_round_trips_and_hides_plaintext() {
        let cipher = BodyCipher::Encrypted { key: KEY_A };
        let body = r#"{"proxy":{"password":"hunter2"}}"#;
        let stored = cipher.encrypt(body).expect("encrypt");
        assert!(stored.starts_with(ENVELOPE_PREFIX));
        assert!(!stored.contains("hunter2"), "plaintext must not survive");
        assert_eq!(cipher.decrypt(&stored).expect("decrypt"), body);
    }

    #[test]
    fn wrong_key_fails_closed() {
        let stored = BodyCipher::Encrypted { key: KEY_A }
            .encrypt(r#"{"x":1}"#)
            .expect("encrypt");
        let err = BodyCipher::Encrypted { key: KEY_B }.decrypt(&stored);
        assert!(matches!(err, Err(CryptoError::Decrypt)));
    }

    #[test]
    fn encrypted_row_needs_a_key_to_read() {
        let stored = BodyCipher::Encrypted { key: KEY_A }
            .encrypt(r#"{"x":1}"#)
            .expect("encrypt");
        assert!(matches!(
            BodyCipher::Plaintext.decrypt(&stored),
            Err(CryptoError::MissingKey)
        ));
    }

    #[test]
    fn legacy_plaintext_row_still_reads_under_encryption() {
        let cipher = BodyCipher::Encrypted { key: KEY_A };
        let legacy = r#"{"old":true}"#;
        assert_eq!(cipher.decrypt(legacy).expect("decrypt legacy"), legacy);
    }
}
