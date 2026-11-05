//! Integration tests for the encrypted profile vault.

use mirage_core::Profile;
use mirage_vault::{Vault, VaultError};

/// Contract sample id (canonical lowercase uuid shape).
const ID_A: &str = "6f9619ff-8b86-d011-b42d-00cf4fc964ff";
/// Second id; sorts after `ID_A`.
const ID_B: &str = "7a1c2d3e-4f5a-6b7c-8d9e-0f1a2b3c4d5e";

/// The contract's canonical Profile/Fingerprint sample as JSON, with the
/// id and name left as placeholders. Building profiles through serde keeps
/// these tests tied to the contract wire format (what the vault actually
/// encrypts) instead of mirage-core's Rust field names.
const PROFILE_JSON: &str = r#"{
  "id": "__ID__",
  "name": "__NAME__",
  "tags": ["work"],
  "fingerprint": {
    "os": "windows",
    "osVersion": "11",
    "platform": "Win32",
    "ua": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
    "chromeVersion": "131.0.6778.86",
    "languages": ["en-US", "en"],
    "timezone": "Europe/London",
    "geo": { "lat": 51.5074, "lon": -0.1278 },
    "screen": {
      "width": 1920, "height": 1080,
      "availWidth": 1920, "availHeight": 1040,
      "dpr": 1, "colorDepth": 24
    },
    "hardwareConcurrency": 8,
    "deviceMemory": 8,
    "webgl": {
      "vendor": "Google Inc. (NVIDIA)",
      "renderer": "ANGLE (NVIDIA, NVIDIA GeForce GTX 1660 SUPER Direct3D11 vs_5_0 ps_5_0, D3D11)",
      "maxTextureSize": 16384,
      "maxViewportDims": [32767, 32767],
      "maxRenderbufferSize": 16384
    },
    "canvasSeed": 1234567890,
    "audioSeed": 987654321,
    "fonts": ["Arial", "Calibri"],
    "webRTC": "real",
    "clientHints": {
      "brands": [
        { "brand": "Not_A Brand", "version": "131" },
        { "brand": "Google Chrome", "version": "131" },
        { "brand": "Chromium", "version": "131" }
      ],
      "mobile": false,
      "platform": "Windows",
      "platformVersion": "15.0.0",
      "architecture": "x86",
      "bitness": "64",
      "uaFullVersion": "131.0.6778.86"
    }
  },
  "proxy": { "scheme": "socks5", "host": "1.2.3.4", "port": 1080, "username": null, "password": null },
  "notes": "",
  "createdAt": "2026-11-03T12:00:00Z",
  "updatedAt": "2026-11-03T12:00:00Z"
}"#;

/// Build a profile from the contract sample with a custom id and name.
fn profile(id: &str, name: &str) -> Profile {
    let json = PROFILE_JSON.replace("__ID__", id).replace("__NAME__", name);
    serde_json::from_str(&json).expect("contract sample profile must deserialize")
}

#[test]
fn save_load_roundtrip() {
    let dir = tempfile::tempdir().expect("tempdir created");
    let vault = Vault::open(dir.path(), "correct horse battery staple").expect("vault opens");

    let original = profile(ID_A, "Shop-01");
    vault.save(&original).expect("profile saved");

    // The file exists under the contract name and carries the magic bytes.
    let file = dir.path().join(format!("{}.mprof", ID_A));
    let raw = std::fs::read(&file).expect("mprof readable");
    assert!(raw.starts_with(b"MGPF"));
    // Ciphertext must not contain the plaintext profile.
    assert!(!raw.windows(4).any(|w| w == b"Shop".as_slice()));

    let loaded = vault.load(ID_A).expect("profile loads");
    assert_eq!(loaded, original);
}

#[test]
fn list_and_delete() {
    let dir = tempfile::tempdir().expect("tempdir created");
    let vault = Vault::open(dir.path(), "pw").expect("vault opens");

    assert!(vault.list().expect("list").is_empty());

    // Save in non-sorted order; list() must return sorted ids.
    vault.save(&profile(ID_B, "B")).expect("save B");
    vault.save(&profile(ID_A, "A")).expect("save A");

    assert_eq!(
        vault.list().expect("list"),
        vec![ID_A.to_string(), ID_B.to_string()]
    );

    vault.delete(ID_A).expect("delete A");
    assert_eq!(vault.list().expect("list"), vec![ID_B.to_string()]);

    // Deleting or loading a missing profile reports NotFound.
    match vault.delete(ID_A) {
        Err(VaultError::NotFound { id }) => assert_eq!(id, ID_A),
        other => panic!("expected NotFound from delete, got {other:?}"),
    }
    match vault.load(ID_A) {
        Err(VaultError::NotFound { .. }) => {}
        other => panic!("expected NotFound from load, got {other:?}"),
    }
}

#[test]
fn wrong_password_reports_wrong_key() {
    let dir = tempfile::tempdir().expect("tempdir created");
    {
        let vault = Vault::open(dir.path(), "hunter2").expect("vault opens");
        vault.save(&profile(ID_A, "Shop-01")).expect("save");
    } // Vault dropped: key wiped.

    // Reopening with another password reuses the stored salt and derives a
    // different key, so the file must fail to decrypt.
    let vault = Vault::open(dir.path(), "hunter3").expect("vault reopens with any password");
    match vault.load(ID_A) {
        Err(VaultError::WrongKey { id }) => assert_eq!(id, ID_A),
        other => panic!("expected WrongKey, got {other:?}"),
    }
}

#[test]
fn garbage_ciphertext_is_corrupted_not_panic() {
    let dir = tempfile::tempdir().expect("tempdir created");
    let vault = Vault::open(dir.path(), "pw").expect("vault opens");
    vault.save(&profile(ID_A, "Shop-01")).expect("save");

    let path = dir.path().join(format!("{}.mprof", ID_A));

    // Valid MAGIC and version, but the ciphertext is truncated below the
    // 16-byte GCM tag: structurally broken -> Corrupted, no panic.
    let mut garbage = Vec::new();
    garbage.extend_from_slice(b"MGPF");
    garbage.extend_from_slice(&1u32.to_le_bytes());
    garbage.extend_from_slice(&[0xA5u8; 12]);
    garbage.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
    std::fs::write(&path, &garbage).expect("garbage written");

    match vault.load(ID_A) {
        Err(VaultError::Corrupted { reason, .. }) => {
            assert!(
                reason.to_lowercase().contains("truncat"),
                "reason: {reason}"
            );
        }
        other => panic!("expected Corrupted, got {other:?}"),
    }

    // A full-length random payload cannot be told apart from a wrong key by
    // AES-GCM itself (the tag check fails either way), so it surfaces as
    // WrongKey; structural damage is what surfaces as Corrupted.
    let mut long_garbage = Vec::new();
    long_garbage.extend_from_slice(b"MGPF");
    long_garbage.extend_from_slice(&1u32.to_le_bytes());
    long_garbage.extend_from_slice(&[0x11u8; 12]);
    long_garbage.extend_from_slice(&[0x5Au8; 128]);
    std::fs::write(&path, &long_garbage).expect("garbage written");

    match vault.load(ID_A) {
        Err(VaultError::WrongKey { .. }) => {}
        other => panic!("expected WrongKey for undecryptable payload, got {other:?}"),
    }
}

#[test]
fn bad_magic_is_corrupted() {
    let dir = tempfile::tempdir().expect("tempdir created");
    let vault = Vault::open(dir.path(), "pw").expect("vault opens");
    vault.save(&profile(ID_A, "Shop-01")).expect("save");

    let path = dir.path().join(format!("{}.mprof", ID_A));

    let mut bad = Vec::new();
    bad.extend_from_slice(b"XXXX"); // wrong magic
    bad.extend_from_slice(&1u32.to_le_bytes());
    bad.extend_from_slice(&[0u8; 12]);
    bad.extend_from_slice(&[0u8; 128]);
    std::fs::write(&path, &bad).expect("bad magic written");

    match vault.load(ID_A) {
        Err(VaultError::Corrupted { reason, .. }) => {
            assert!(reason.contains("magic"), "reason: {reason}");
        }
        other => panic!("expected Corrupted, got {other:?}"),
    }
}

#[test]
fn unsupported_version_is_corrupted() {
    let dir = tempfile::tempdir().expect("tempdir created");
    let vault = Vault::open(dir.path(), "pw").expect("vault opens");
    vault.save(&profile(ID_A, "Shop-01")).expect("save");

    let path = dir.path().join(format!("{}.mprof", ID_A));

    let mut bad = Vec::new();
    bad.extend_from_slice(b"MGPF");
    bad.extend_from_slice(&99u32.to_le_bytes()); // unknown future version
    bad.extend_from_slice(&[0u8; 12]);
    bad.extend_from_slice(&[0u8; 128]);
    std::fs::write(&path, &bad).expect("bad version written");

    match vault.load(ID_A) {
        Err(VaultError::Corrupted { reason, .. }) => {
            assert!(reason.contains("version"), "reason: {reason}");
        }
        other => panic!("expected Corrupted, got {other:?}"),
    }
}

#[test]
fn save_overwrites_existing_profile() {
    let dir = tempfile::tempdir().expect("tempdir created");
    let vault = Vault::open(dir.path(), "pw").expect("vault opens");

    let first = profile(ID_A, "Shop-01");
    vault.save(&first).expect("first save");
    let second = profile(ID_A, "Shop-01-renamed");
    vault.save(&second).expect("second save");

    let loaded = vault.load(ID_A).expect("reload after overwrite");
    assert_eq!(loaded, second);
    assert_ne!(loaded, first);

    // Exactly one profile file and no leftover sidecar from the atomic write.
    assert_eq!(vault.list().expect("list"), vec![ID_A.to_string()]);
    assert!(!dir.path().join(format!("{}.mprof.tmp", ID_A)).exists());
}

#[test]
fn path_traversal_and_malformed_ids_are_rejected() {
    let dir = tempfile::tempdir().expect("tempdir created");
    let vault = Vault::open(dir.path(), "pw").expect("vault opens");

    let evil = profile("../evil", "nope");
    match vault.save(&evil) {
        Err(VaultError::InvalidId { id }) => assert_eq!(id, "../evil"),
        other => panic!("expected InvalidId from save, got {other:?}"),
    }

    for bad in [
        "../evil",
        "..\\evil",
        "6F9619FF-8B86-D011-B42D-00CF4FC964FF", // uppercase
        "6f9619ff8b86d011b42d00cf4fc964ff",     // no dashes
        "6f9619ff-8b86-d011-b42d-00cf4fc964f",  // 35 chars
        "6f9619ff_8b86_d011_b42d_00cf4fc964ff", // wrong separators
        "",
        ".",
        "..",
    ] {
        match vault.load(bad) {
            Err(VaultError::InvalidId { id }) => assert_eq!(id, bad),
            other => panic!("expected InvalidId from load for `{bad}`, got {other:?}"),
        }
        match vault.delete(bad) {
            Err(VaultError::InvalidId { .. }) => {}
            other => panic!("expected InvalidId from delete for `{bad}`, got {other:?}"),
        }
    }

    // Nothing escaped the vault directory and nothing was written.
    assert!(vault.list().expect("list").is_empty());
    assert!(!dir
        .path()
        .parent()
        .expect("tempdir has a parent")
        .join("evil.mprof")
        .exists());
}

#[test]
fn vault_reopens_with_same_password_and_salt() {
    let dir = tempfile::tempdir().expect("tempdir created");
    {
        let vault = Vault::open(dir.path(), "stable password").expect("vault opens");
        vault.save(&profile(ID_A, "Shop-01")).expect("save");
    }

    // The salt must be reused (not regenerated), so the same password
    // derives the same key and the profile stays readable.
    let vault = Vault::open(dir.path(), "stable password").expect("vault reopens");
    let loaded = vault.load(ID_A).expect("profile survives reopen");
    assert_eq!(loaded, profile(ID_A, "Shop-01"));
    assert!(dir.path().join(".vault.salt").is_file());
}

#[test]
fn malformed_salt_file_is_corrupted() {
    let dir = tempfile::tempdir().expect("tempdir created");
    std::fs::write(dir.path().join(".vault.salt"), b"short").expect("write salt");

    match Vault::open(dir.path(), "pw") {
        Err(VaultError::Corrupted { reason, .. }) => {
            assert!(reason.contains("16"), "reason: {reason}");
        }
        other => panic!("expected Corrupted, got {other:?}"),
    }
}
