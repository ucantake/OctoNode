//! Secret storage with automatic fallback.
//!
//! Primary backend: the OS credential store through `keyring`
//! (macOS Keychain, Windows Credential Manager, Linux Secret Service).
//!
//! Fallback backend: an Argon2id + XChaCha20-Poly1305 encrypted file, unlocked
//! with a master password. It is selected when the OS store is unreachable —
//! typically a headless Linux box or a minimal window manager without
//! `gnome-keyring`/`kwallet` (no `org.freedesktop.secrets` on the session bus,
//! or no session bus at all) — or when the user forces it in settings.
//!
//! Callers only see [`SecretVault`]; they never need to know which backend is
//! active, except for reacting to [`AppError::VaultLocked`].

mod file_store;
mod keyring_store;

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::error::{AppError, AppResult};
use crate::models::{SecretBackendKind, VaultStatus};
use crate::paths::AppPaths;

pub use file_store::{FileStore, KdfParams};
pub use keyring_store::KeyringStore;

/// Secrets are wiped from memory when dropped.
pub type Secret = Zeroizing<String>;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SecretBackendPreference {
    /// OS keyring when reachable, encrypted file otherwise.
    #[default]
    Auto,
    /// Always use the encrypted file (portable, e.g. on shared machines).
    EncryptedFile,
}

/// Keys are namespaced so the vault can later hold more than tokens.
pub fn token_key(account_id: Uuid) -> String {
    format!("account:{account_id}:token")
}

pub fn ssh_passphrase_key(account_id: Uuid) -> String {
    format!("account:{account_id}:ssh-passphrase")
}

enum Backend {
    Keyring(KeyringStore),
    File(FileStore),
}

pub struct SecretVault {
    backend: Mutex<Backend>,
    fallback_reason: Option<String>,
}

impl SecretVault {
    /// Picks the backend. Never fails: an unusable keyring degrades to the
    /// file backend and the reason is reported to the UI.
    pub fn open(paths: &AppPaths, preference: SecretBackendPreference) -> Self {
        let file = || FileStore::new(paths.vault_file(), KdfParams::default());

        if preference == SecretBackendPreference::EncryptedFile {
            return Self {
                backend: Mutex::new(Backend::File(file())),
                fallback_reason: Some("encrypted file storage selected in settings".into()),
            };
        }

        match KeyringStore::open() {
            Ok(keyring) => {
                tracing::info!(collection = ?keyring.target(), "secret storage: OS keyring");
                Self {
                    backend: Mutex::new(Backend::Keyring(keyring)),
                    fallback_reason: None,
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "OS keyring unavailable, using encrypted file vault");
                Self {
                    backend: Mutex::new(Backend::File(file())),
                    fallback_reason: Some(e.to_string()),
                }
            }
        }
    }

    #[cfg(test)]
    pub fn with_file_store(store: FileStore) -> Self {
        Self {
            backend: Mutex::new(Backend::File(store)),
            fallback_reason: Some("test".into()),
        }
    }

    pub fn status(&self) -> AppResult<VaultStatus> {
        let backend = self.backend.lock()?;
        Ok(match &*backend {
            Backend::Keyring(_) => VaultStatus {
                backend: SecretBackendKind::OsKeyring,
                locked: false,
                initialized: true,
                fallback_reason: None,
            },
            Backend::File(f) => VaultStatus {
                backend: SecretBackendKind::EncryptedFile,
                locked: !f.is_unlocked(),
                initialized: f.exists(),
                fallback_reason: self.fallback_reason.clone(),
            },
        })
    }

    pub fn is_locked(&self) -> AppResult<bool> {
        Ok(self.status()?.locked)
    }

    /// Unlocks (or, on first use, creates) the encrypted file vault.
    /// A no-op for the OS keyring.
    pub fn unlock(&self, master_password: &str) -> AppResult<()> {
        let mut backend = self.backend.lock()?;
        match &mut *backend {
            Backend::Keyring(_) => Ok(()),
            Backend::File(f) => f.unlock(master_password),
        }
    }

    /// Changes the encrypted-file vault's master password in place.
    pub fn change_master_password(&self, current: &str, new: &str) -> AppResult<()> {
        let mut backend = self.backend.lock()?;
        match &mut *backend {
            Backend::Keyring(_) => Err(AppError::Unsupported(
                "secrets are stored in the OS keychain, which has no OctoNode master password"
                    .into(),
            )),
            Backend::File(f) => f.change_password(current, new),
        }
    }

    pub fn lock(&self) -> AppResult<()> {
        let mut backend = self.backend.lock()?;
        if let Backend::File(f) = &mut *backend {
            f.lock();
        }
        Ok(())
    }

    pub fn get(&self, key: &str) -> AppResult<Option<Secret>> {
        let backend = self.backend.lock()?;
        match &*backend {
            Backend::Keyring(k) => k.get(key),
            Backend::File(f) => f.get(key),
        }
    }

    pub fn set(&self, key: &str, value: &str) -> AppResult<()> {
        if value.is_empty() {
            return Err(AppError::InvalidInput("secret must not be empty".into()));
        }
        let mut backend = self.backend.lock()?;
        match &mut *backend {
            Backend::Keyring(k) => k.set(key, value),
            Backend::File(f) => f.set(key, value),
        }
    }

    /// Idempotent: deleting a missing secret succeeds.
    pub fn delete(&self, key: &str) -> AppResult<()> {
        let mut backend = self.backend.lock()?;
        match &mut *backend {
            Backend::Keyring(k) => k.delete(key),
            Backend::File(f) => f.delete(key),
        }
    }
}
