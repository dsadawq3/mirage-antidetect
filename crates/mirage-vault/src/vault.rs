//! Encrypted on-disk storage for Mirage profiles.
//!
//! Every profile is one `<id>.mprof` file inside the vault directory with
//! the layout defined by the project contract:
//! `MAGIC "MGPF" || u32 LE version || nonce[12] || AES-256-GCM(json Profile)`.
//! Writes are atomic (sidecar `.tmp` file + rename), and ids are validated
//! as canonical lowercase uuids before they ever reach the filesystem.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use mirage_core::Profile;
use rand::RngCore;

use crate::crypto::{self, MAGIC, NONCE_LEN, TAG_LEN, VERSION};
use crate::error::VaultError;

/// Byte size of the `version` header field (u32, little-endian).
const VERSION_LEN: usize = 4;
/// Per-vault salt file, created on first open.
const SALT_FILE: &str = ".vault.salt";
/// Extension of encrypted profile files.
const PROFILE_EXT: &str = "mprof";
/// Canonical uuid string length (8-4-4-4-12 hex groups with dashes).
const UUID_LEN: usize = 36;
/// Byte positions of the dashes inside a canonical uuid.
const UUID_DASH_POSITIONS: [usize; 4] = [8, 13, 18, 23];

/// An opened vault: the directory of encrypted `.mprof` files plus the key
/// derived from the master password.
pub struct Vault {
    dir: PathBuf,
    key: [u8; 32],
}

// Manual impl: the derived key must never leak through Debug output.
impl std::fmt::Debug for Vault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vault")
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

impl Drop for Vault {
    fn drop(&mut self) {
        // Best-effort wipe of the derived key; it is not used after this.
        self.key.fill(0);
    }
}

impl Vault {
    /// Open (or create) the vault in `dir`, unlocked by `master_password`.
    ///
    /// On first open a random 16-byte salt is generated and persisted in
    /// `.vault.salt`; afterwards the salt is reused, so the same password
    /// always derives the same key. The password itself is never stored.
    pub fn open(dir: impl Into<PathBuf>, master_password: &str) -> Result<Self, VaultError> {
        let dir = dir.into();
        fs::create_dir_all(&dir).map_err(|source| VaultError::Io { source })?;

        let salt_path = dir.join(SALT_FILE);
        let salt = read_or_create_salt(&salt_path)?;
        let key = crypto::derive_key(master_password, &salt);

        Ok(Self { dir, key })
    }

    /// Encrypt and persist `profile` as `<dir>/<id>.mprof`.
    ///
    /// The write is atomic: the full payload lands in a sidecar `.tmp`
    /// file first and is moved over the old profile only when complete.
    pub fn save(&self, profile: &Profile) -> Result<(), VaultError> {
        validate_id(&profile.id)?;

        let json = serde_json::to_vec(profile)?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng); // fresh 96-bit nonce per save
        let ciphertext = cipher_for(&self.key)?
            .encrypt(&nonce, json.as_slice())
            .map_err(|_| VaultError::Crypto)?;

        // Layout: MAGIC || u32 LE version || nonce || ciphertext (tag included).
        let mut file = Vec::with_capacity(MAGIC.len() + VERSION_LEN + NONCE_LEN + ciphertext.len());
        file.extend_from_slice(&MAGIC);
        file.extend_from_slice(&VERSION.to_le_bytes());
        file.extend_from_slice(&nonce);
        file.extend_from_slice(&ciphertext);

        atomic_write(&self.profile_path(&profile.id), &file)
    }

    /// Load, decrypt and deserialize the profile with the given id.
    pub fn load(&self, id: &str) -> Result<Profile, VaultError> {
        validate_id(id)?;
        let path = self.profile_path(id);

        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == ErrorKind::NotFound => {
                return Err(VaultError::NotFound { id: id.to_owned() });
            }
            Err(source) => return Err(VaultError::Io { source }),
        };

        // Minimum size: MAGIC(4) + version(4) + nonce(12) + GCM tag(16).
        if bytes.len() < MAGIC.len() + VERSION_LEN + NONCE_LEN + TAG_LEN {
            return Err(VaultError::Corrupted {
                path,
                reason: format!("file truncated: {} bytes", bytes.len()),
            });
        }

        let (magic, rest) = bytes.split_at(MAGIC.len());
        let (version_bytes, rest) = rest.split_at(VERSION_LEN);
        let (nonce_bytes, ciphertext) = rest.split_at(NONCE_LEN);

        if magic != MAGIC.as_slice() {
            return Err(VaultError::Corrupted {
                path,
                reason: "invalid magic bytes".to_owned(),
            });
        }

        let mut version_arr = [0u8; VERSION_LEN];
        version_arr.copy_from_slice(version_bytes);
        let version = u32::from_le_bytes(version_arr);
        if version != VERSION {
            return Err(VaultError::Corrupted {
                path,
                reason: format!("unsupported format version {version}"),
            });
        }

        let mut nonce_arr = [0u8; NONCE_LEN];
        nonce_arr.copy_from_slice(nonce_bytes);
        let nonce = Nonce::from(nonce_arr);

        // AES-GCM cannot tell a wrong key from a tampered payload: both
        // fail the tag check. A wrong password is the common case, so the
        // failure is reported as WrongKey.
        let plaintext = cipher_for(&self.key)?
            .decrypt(&nonce, ciphertext)
            .map_err(|_| VaultError::WrongKey { id: id.to_owned() })?;

        let profile: Profile = match serde_json::from_slice(&plaintext) {
            Ok(profile) => profile,
            Err(source) => {
                return Err(VaultError::Corrupted {
                    path,
                    reason: format!("payload is not valid profile JSON: {source}"),
                });
            }
        };

        if profile.id != id {
            return Err(VaultError::Corrupted {
                path,
                reason: format!(
                    "profile id `{}` does not match its file name `{}`",
                    profile.id, id
                ),
            });
        }

        Ok(profile)
    }

    /// Ids of all profiles stored in the vault, sorted.
    ///
    /// Only regular files with the `.mprof` extension are considered, so
    /// `.vault.salt` and leftover `.tmp` sidecars never show up.
    pub fn list(&self) -> Result<Vec<String>, VaultError> {
        let mut ids = Vec::new();
        for entry in fs::read_dir(&self.dir).map_err(|source| VaultError::Io { source })? {
            let entry = entry.map_err(|source| VaultError::Io { source })?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some(PROFILE_EXT) {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                ids.push(stem.to_owned());
            }
        }
        ids.sort();
        Ok(ids)
    }

    /// Delete the stored profile with the given id.
    pub fn delete(&self, id: &str) -> Result<(), VaultError> {
        validate_id(id)?;
        let path = self.profile_path(id);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == ErrorKind::NotFound => {
                Err(VaultError::NotFound { id: id.to_owned() })
            }
            Err(source) => Err(VaultError::Io { source }),
        }
    }

    /// Path of the `.mprof` file for an id (callers validate the id first).
    fn profile_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{}.{}", id, PROFILE_EXT))
    }
}

/// Reject ids that are not canonical lowercase uuids.
///
/// Only `[a-f0-9-]` with the exact 8-4-4-4-12 shape is accepted. This runs
/// before any path is built, so traversal payloads like `../evil`, absolute
/// paths and separators never reach the filesystem.
fn validate_id(id: &str) -> Result<(), VaultError> {
    let reject = || VaultError::InvalidId { id: id.to_owned() };

    if id.len() != UUID_LEN {
        return Err(reject());
    }

    for (index, byte) in id.as_bytes().iter().enumerate() {
        let ok = if UUID_DASH_POSITIONS.contains(&index) {
            *byte == b'-'
        } else {
            byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()
        };
        if !ok {
            return Err(reject());
        }
    }

    Ok(())
}

/// Build the AES-256-GCM cipher for the vault key.
///
/// Only fails for key lengths other than 32 bytes, which the fixed
/// `[u8; 32]` field type rules out.
fn cipher_for(key: &[u8; 32]) -> Result<Aes256Gcm, VaultError> {
    Aes256Gcm::new_from_slice(key).map_err(|_| VaultError::Crypto)
}

/// Read the vault salt, generating and persisting it on first open.
///
/// A salt file with a length other than 16 bytes is reported as corrupted:
/// regenerating it silently would render every existing profile permanently
/// undecryptable.
fn read_or_create_salt(path: &Path) -> Result<[u8; 16], VaultError> {
    match fs::read(path) {
        Ok(bytes) => {
            if bytes.len() == 16 {
                let mut salt = [0u8; 16];
                salt.copy_from_slice(&bytes);
                Ok(salt)
            } else {
                Err(VaultError::Corrupted {
                    path: path.to_path_buf(),
                    reason: format!(
                        "salt file must hold exactly 16 bytes, found {}",
                        bytes.len()
                    ),
                })
            }
        }
        // First open of this vault: generate a fresh random salt.
        Err(source) if source.kind() == ErrorKind::NotFound => {
            let mut salt = [0u8; 16];
            rand::rngs::OsRng.fill_bytes(&mut salt);
            atomic_write(path, &salt)?;
            Ok(salt)
        }
        Err(source) => Err(VaultError::Io { source }),
    }
}

/// Atomically replace `path` with `data`.
///
/// The payload is fully written to `<path>.tmp` first and only then moved
/// into place. Windows cannot rename onto an existing file, so the previous
/// version is removed before the rename (best effort; a missing previous
/// file is the normal first-write case).
fn atomic_write(path: &Path, data: &[u8]) -> Result<(), VaultError> {
    let mut tmp_name = path.as_os_str().to_os_string();
    tmp_name.push(".tmp");
    let tmp_path = PathBuf::from(tmp_name);

    if let Err(source) = fs::write(&tmp_path, data) {
        // Best-effort cleanup of a partial sidecar; `.tmp` files are also
        // ignored by `Vault::list`.
        let _ = fs::remove_file(&tmp_path);
        return Err(VaultError::Io { source });
    }

    // Remove the previous version so the rename can succeed on Windows.
    if let Err(source) = fs::remove_file(path) {
        if source.kind() != ErrorKind::NotFound {
            // Keep the sidecar: it is the only complete copy of the new payload.
            return Err(VaultError::Io { source });
        }
    }

    fs::rename(&tmp_path, path).map_err(|source| {
        // Keep the sidecar: the previous file is already gone at this point
        // and the sidecar is the only surviving copy of the new payload.
        VaultError::Io { source }
    })
}
