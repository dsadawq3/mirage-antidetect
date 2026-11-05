//! API key generation and hashing.
//!
//! A key is 32 random bytes shown to the operator exactly once as 64 lowercase
//! hex characters. Only `sha256(key)` is stored at rest, so a leaked database
//! file does not leak working credentials.

use rand::RngCore;
use sha2::{Digest, Sha256};

/// Generate a fresh API key: 64 lowercase hex chars, 256 bits of entropy.
pub fn generate_key() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Hash a raw key for storage and lookup: lowercase hex sha256.
pub fn hash_key(key: &str) -> String {
    hex::encode(Sha256::digest(key.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_key_matches_known_sha256_vector() {
        // Well-known digest: sha256 of the empty string.
        assert_eq!(
            hash_key(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn generated_keys_are_64_hex_chars_and_unique() {
        let a = generate_key();
        let b = generate_key();
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}
