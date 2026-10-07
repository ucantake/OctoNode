//! OS credential store backend.

use keyring::{Entry, Error as KeyringError};
use zeroize::Zeroizing;

use super::Secret;
use crate::error::{AppError, AppResult};

/// Service name shown in Keychain Access / Credential Manager / Seahorse.
const SERVICE: &str = "dev.octonode.app";
const PROBE_USER: &str = "__octonode_probe__";

pub struct KeyringStore {
    service: &'static str,
}

impl KeyringStore {
    pub fn new() -> Self {
        Self { service: SERVICE }
    }

    /// Checks that the platform store is reachable without writing anything.
    ///
    /// A read of a non-existent entry returns `NoEntry` when the store works;
    /// on headless Linux it fails with `PlatformFailure` (no D-Bus session) or
    /// `NoStorageAccess` (no Secret Service provider / locked collection).
    pub fn probe(&self) -> AppResult<()> {
        let entry = Entry::new(self.service, PROBE_USER).map_err(describe)?;
        match entry.get_password() {
            Ok(_) | Err(KeyringError::NoEntry) => Ok(()),
            Err(e) => Err(describe(e)),
        }
    }

    pub fn get(&self, key: &str) -> AppResult<Option<Secret>> {
        let entry = Entry::new(self.service, key).map_err(describe)?;
        match entry.get_password() {
            Ok(v) => Ok(Some(Zeroizing::new(v))),
            Err(KeyringError::NoEntry) => Ok(None),
            Err(e) => Err(describe(e)),
        }
    }

    pub fn set(&self, key: &str, value: &str) -> AppResult<()> {
        Entry::new(self.service, key)
            .and_then(|entry| entry.set_password(value))
            .map_err(describe)
    }

    pub fn delete(&self, key: &str) -> AppResult<()> {
        let entry = Entry::new(self.service, key).map_err(describe)?;
        match entry.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(e) => Err(describe(e)),
        }
    }
}

impl Default for KeyringStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Maps keyring errors to actionable messages.
fn describe(e: KeyringError) -> AppError {
    let hint = match &e {
        KeyringError::PlatformFailure(_) if cfg!(target_os = "linux") => {
            " (is a Secret Service provider such as gnome-keyring or KWallet running on the D-Bus session bus?)"
        }
        KeyringError::NoStorageAccess(_) => " (the credential store is locked or access was denied)",
        _ => "",
    };
    AppError::SecretStore(format!("{e}{hint}"))
}
