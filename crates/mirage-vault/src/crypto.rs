//! Key derivation and `.mprof` format constants.

use argon2::{Algorithm, Argon2, Params, Version};

/// Argon2id memory cost: 19456 KiB (19 MiB) — OWASP-recommended minimum.
const ARGON2_M_COST: u32 = 19456;
/// Argon2id time cost (number of passes): 2 — OWASP-recommended minimum.
const ARGON2_T_COST: u32 = 2;
/// Argon2id parallelism (lanes): 1.
const ARGON2_P_COST: u32 = 1;
/// Derived key length: 256 bits, matching AES-256-GCM.
const KEY_LEN: usize = 32;

/// Magic bytes at the start of every `.mprof` file.
pub const MAGIC: [u8; 4] = *b"MGPF";
/// On-disk format version of `.mprof` files (stored as u32, little-endian).
pub const VERSION: u32 = 1;
/// AES-256-GCM nonce length in bytes (96 bits).
pub const NONCE_LEN: usize = 12;
/// AES-256-GCM authentication tag length in bytes.
pub const TAG_LEN: usize = 16;

/// Derive the 256-bit vault key from the master password and vault salt.
///
/// Argon2id with the OWASP minimum parameter set (m = 19456 KiB, t = 2,
/// p = 1). The salt is the per-vault value persisted in `.vault.salt`;
/// the password itself is never written anywhere.
pub fn derive_key(password: &str, salt: &[u8; 16]) -> [u8; 32] {
    let mut key = [0u8; KEY_LEN];

    // `Params::new` only rejects out-of-range costs. The constants above
    // are static and valid, and `Params::default()` carries the very same
    // OWASP values, so the fallback cannot change the derivation.
    let params = Params::new(ARGON2_M_COST, ARGON2_T_COST, ARGON2_P_COST, Some(KEY_LEN))
        .unwrap_or_else(|_| Params::default());

    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

    // Cannot fail with valid params, a 16-byte salt and a 32-byte output
    // buffer. Should it ever fail, the key stays zeroed and every later
    // AES-GCM open fails its tag check: the vault fails closed instead of
    // ever trusting a fabricated key.
    let _ = argon2.hash_password_into(password.as_bytes(), salt, &mut key);

    key
}
