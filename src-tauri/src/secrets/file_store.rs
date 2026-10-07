//! Encrypted-file fallback vault.
//!
//! Format (JSON, versioned):
//! ```json
//! { "version": 1,
//!   "kdf": { "algorithm": "argon2id", "memKib": 65536, "iterations": 3, "parallelism": 1, "salt": "<b64>" },
//!   "nonce": "<b64, 24 bytes>",
//!   "ciphertext": "<b64>" }
//! ```
//! The plaintext is a JSON object `{ key: secret }`. Each save uses a fresh
//! random nonce; the KDF parameters are read back from the file so they can be
//! raised later without breaking existing vaults.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use argon2::{Algorithm, Argon2, Params, Version};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use chacha20poly1305::aead::rand_core::RngCore;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use super::Secret;
use crate::error::{AppError, AppResult};
use crate::paths::atomic_write;

const FORMAT_VERSION: u32 = 1;
/// Binds the ciphertext to this format; a downgraded header fails to decrypt.
const AAD: &[u8] = b"octonode-vault-v1";
const MIN_PASSWORD_LEN: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KdfParams {
    pub mem_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

impl Default for KdfParams {
    /// OWASP-recommended Argon2id profile (64 MiB, t=3, p=1): ~0.3-0.8 s on
    /// typical desktop hardware, paid once per unlock.
    fn default() -> Self {
        Self {
            mem_kib: 64 * 1024,
            iterations: 3,
            parallelism: 1,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KdfHeader {
    algorithm: String,
    #[serde(flatten)]
    params: KdfParams,
    salt: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VaultFile {
    version: u32,
    kdf: KdfHeader,
    nonce: String,
    ciphertext: String,
}

struct Unlocked {
    key: Zeroizing<[u8; 32]>,
    salt: Vec<u8>,
    params: KdfParams,
    secrets: BTreeMap<String, Secret>,
}

pub struct FileStore {
    path: PathBuf,
    new_vault_params: KdfParams,
    unlocked: Option<Unlocked>,
}

impl FileStore {
    pub fn new(path: PathBuf, new_vault_params: KdfParams) -> Self {
        Self {
            path,
            new_vault_params,
            unlocked: None,
        }
    }

    pub fn exists(&self) -> bool {
        self.path.is_file()
    }

    pub fn is_unlocked(&self) -> bool {
        self.unlocked.is_some()
    }

    pub fn lock(&mut self) {
        // `Zeroizing` wipes key and secrets on drop.
        self.unlocked = None;
    }

    /// Opens an existing vault, or creates an empty one on first use.
    pub fn unlock(&mut self, password: &str) -> AppResult<()> {
        if self.exists() {
            self.unlocked = Some(self.open_existing(password)?);
            return Ok(());
        }

        if password.chars().count() < MIN_PASSWORD_LEN {
            return Err(AppError::InvalidInput(format!(
                "master password must be at least {MIN_PASSWORD_LEN} characters"
            )));
        }
        let mut salt = vec![0u8; 16];
        OsRng.fill_bytes(&mut salt);
        let params = self.new_vault_params;
        let key = derive_key(password, &salt, params)?;
        self.unlocked = Some(Unlocked {
            key,
            salt,
            params,
            secrets: BTreeMap::new(),
        });
        self.persist()
    }

    /// Re-encrypts the vault under a new master password, without restarting
    /// and without losing any secret. The current password is verified
    /// against the file on disk (not just the in-memory key), a fresh salt is
    /// generated, and the current KDF profile is applied, so changing the
    /// password also upgrades an older vault's KDF cost. The write is atomic:
    /// a crash leaves either the old or the new vault, never a mix.
    pub fn change_password(&mut self, current: &str, new: &str) -> AppResult<()> {
        let state = self.unlocked.as_ref().ok_or(AppError::VaultLocked)?;
        if new.chars().count() < MIN_PASSWORD_LEN {
            return Err(AppError::InvalidInput(format!(
                "master password must be at least {MIN_PASSWORD_LEN} characters"
            )));
        }
        if new == current {
            return Err(AppError::InvalidInput(
                "the new master password must differ from the current one".into(),
            ));
        }
        // Wrong password → InvalidPassword. Also catches a vault file that was
        // replaced on disk since unlock.
        let on_disk = self.open_existing(current)?;
        if on_disk.secrets.len() != state.secrets.len() {
            return Err(AppError::Internal(
                "vault on disk differs from the unlocked vault; unlock again".into(),
            ));
        }

        let mut salt = vec![0u8; 16];
        OsRng.fill_bytes(&mut salt);
        let params = self.new_vault_params;
        let key = derive_key(new, &salt, params)?;

        let previous = self.unlocked.take().ok_or(AppError::VaultLocked)?;
        self.unlocked = Some(Unlocked {
            key,
            salt,
            params,
            secrets: previous.secrets.clone(),
        });
        if let Err(e) = self.persist() {
            // Disk still holds the old vault; keep memory consistent with it.
            self.unlocked = Some(previous);
            return Err(e);
        }
        Ok(())
    }

    pub fn get(&self, key: &str) -> AppResult<Option<Secret>> {
        let state = self.unlocked.as_ref().ok_or(AppError::VaultLocked)?;
        Ok(state.secrets.get(key).cloned())
    }

    pub fn set(&mut self, key: &str, value: &str) -> AppResult<()> {
        let state = self.unlocked.as_mut().ok_or(AppError::VaultLocked)?;
        state
            .secrets
            .insert(key.to_owned(), Zeroizing::new(value.to_owned()));
        self.persist()
    }

    pub fn delete(&mut self, key: &str) -> AppResult<()> {
        let state = self.unlocked.as_mut().ok_or(AppError::VaultLocked)?;
        if state.secrets.remove(key).is_some() {
            self.persist()?;
        }
        Ok(())
    }

    fn open_existing(&self, password: &str) -> AppResult<Unlocked> {
        let raw = fs::read(&self.path)?;
        let file: VaultFile = serde_json::from_slice(&raw)?;
        if file.version != FORMAT_VERSION {
            return Err(AppError::Unsupported(format!(
                "vault format version {} (expected {FORMAT_VERSION})",
                file.version
            )));
        }
        if file.kdf.algorithm != "argon2id" {
            return Err(AppError::Unsupported(format!("KDF {}", file.kdf.algorithm)));
        }

        let salt = decode(&file.kdf.salt, "salt")?;
        let nonce_bytes = decode(&file.nonce, "nonce")?;
        if nonce_bytes.len() != 24 {
            return Err(AppError::Internal("corrupt vault: bad nonce length".into()));
        }
        let ciphertext = decode(&file.ciphertext, "ciphertext")?;

        let key = derive_key(password, &salt, file.kdf.params)?;
        let cipher = XChaCha20Poly1305::new(key.as_ref().into());
        // AEAD failure means wrong password *or* tampering; both are reported
        // the same way to avoid an oracle.
        let plaintext = Zeroizing::new(
            cipher
                .decrypt(
                    XNonce::from_slice(&nonce_bytes),
                    Payload {
                        msg: &ciphertext,
                        aad: AAD,
                    },
                )
                .map_err(|_| AppError::InvalidPassword)?,
        );

        let mut map: BTreeMap<String, String> = serde_json::from_slice(&plaintext)?;
        let secrets = map
            .iter_mut()
            .map(|(k, v)| (k.clone(), Zeroizing::new(std::mem::take(v))))
            .collect();
        Ok(Unlocked {
            key,
            salt,
            params: file.kdf.params,
            secrets,
        })
    }

    fn persist(&self) -> AppResult<()> {
        let state = self.unlocked.as_ref().ok_or(AppError::VaultLocked)?;

        let plain_map: BTreeMap<&str, &str> = state
            .secrets
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let plaintext = Zeroizing::new(serde_json::to_vec(&plain_map)?);

        let cipher = XChaCha20Poly1305::new(state.key.as_ref().into());
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ciphertext = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: &plaintext,
                    aad: AAD,
                },
            )
            .map_err(|_| AppError::Internal("vault encryption failed".into()))?;

        let file = VaultFile {
            version: FORMAT_VERSION,
            kdf: KdfHeader {
                algorithm: "argon2id".into(),
                params: state.params,
                salt: B64.encode(&state.salt),
            },
            nonce: B64.encode(nonce),
            ciphertext: B64.encode(ciphertext),
        };
        atomic_write(&self.path, &serde_json::to_vec_pretty(&file)?)
    }
}

fn derive_key(password: &str, salt: &[u8], p: KdfParams) -> AppResult<Zeroizing<[u8; 32]>> {
    let params = Params::new(p.mem_kib, p.iterations, p.parallelism, Some(32))
        .map_err(|e| AppError::Internal(format!("invalid KDF parameters: {e}")))?;
    let mut key = Zeroizing::new([0u8; 32]);
    let mut pwd = Zeroizing::new(password.as_bytes().to_vec());
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(&pwd, salt, key.as_mut())
        .map_err(|e| AppError::Internal(format!("key derivation failed: {e}")))?;
    pwd.zeroize();
    Ok(key)
}

fn decode(s: &str, what: &str) -> AppResult<Vec<u8>> {
    B64.decode(s)
        .map_err(|e| AppError::Internal(format!("corrupt vault {what}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fast_params() -> KdfParams {
        KdfParams {
            mem_kib: 1024,
            iterations: 1,
            parallelism: 1,
        }
    }

    #[test]
    fn roundtrip_and_wrong_password() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("secrets.vault");

        let mut store = FileStore::new(path.clone(), fast_params());
        assert!(matches!(store.get("k"), Err(AppError::VaultLocked)));
        assert!(
            store.unlock("short").is_err(),
            "short passwords are rejected"
        );
        store.unlock("correct horse battery").expect("create");
        store.set("account:1:token", "glpat-secret").expect("set");
        store.lock();

        let raw = fs::read_to_string(&path).expect("read vault");
        assert!(
            !raw.contains("glpat-secret"),
            "secret must not be stored in clear"
        );

        let mut reopened = FileStore::new(path.clone(), fast_params());
        assert!(matches!(
            reopened.unlock("wrong password!"),
            Err(AppError::InvalidPassword)
        ));
        reopened.unlock("correct horse battery").expect("unlock");
        let v = reopened
            .get("account:1:token")
            .expect("get")
            .expect("present");
        assert_eq!(v.as_str(), "glpat-secret");

        reopened
            .change_password("wrong password!", "a brand new pass")
            .expect_err("current password is verified");
        reopened
            .change_password("correct horse battery", "short")
            .expect_err("new password length is enforced");
        reopened
            .change_password("correct horse battery", "a brand new pass")
            .expect("change password");
        // Still usable in-process without a restart.
        assert_eq!(
            reopened
                .get("account:1:token")
                .expect("get")
                .expect("kept")
                .as_str(),
            "glpat-secret"
        );
        let mut fresh = FileStore::new(path.clone(), fast_params());
        assert!(matches!(
            fresh.unlock("correct horse battery"),
            Err(AppError::InvalidPassword)
        ));
        fresh
            .unlock("a brand new pass")
            .expect("unlock with new password");
        assert_eq!(
            fresh
                .get("account:1:token")
                .expect("get")
                .expect("kept")
                .as_str(),
            "glpat-secret"
        );

        reopened.delete("account:1:token").expect("delete");
        assert!(reopened.get("account:1:token").expect("get").is_none());
    }
}
