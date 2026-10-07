//! OS credential store backend.
//!
//! On Linux, keyring-rs stores items in the Secret Service collection that has
//! the `default` alias. Many real setups have a working Secret Service but *no*
//! such alias: KeePassXC's integration, some KWallet bridges, a gnome-keyring
//! whose login keyring was never created, WSL. There every lookup fails with
//! "Secret Service: no result found". Instead of giving up, [`KeyringStore::open`]
//! then targets an existing persistent collection by label (keyring-rs'
//! `target`), preferring the conventional `login` keyring.

use keyring::{Entry, Error as KeyringError};
use zeroize::Zeroizing;

use super::Secret;
use crate::error::{AppError, AppResult};

/// Service name shown in Keychain Access / Credential Manager / Seahorse.
const SERVICE: &str = "dev.octonode.app";
const PROBE_USER: &str = "__octonode_probe__";
/// Overrides the Secret Service collection (by label), e.g. `Passwords`.
#[cfg(target_os = "linux")]
const COLLECTION_ENV: &str = "OCTONODE_KEYRING_COLLECTION";

pub struct KeyringStore {
    service: &'static str,
    /// Secret Service collection label; `None` = the `default` alias.
    /// Always `None` on macOS and Windows.
    target: Option<String>,
}

impl KeyringStore {
    /// Connects to the platform store, or explains why it is unusable.
    /// Read-only: never creates items or collections.
    pub fn open() -> AppResult<Self> {
        #[cfg(target_os = "linux")]
        if let Some(label) = std::env::var(COLLECTION_ENV)
            .ok()
            .filter(|s| !s.trim().is_empty())
        {
            // keyring-rs *creates* a missing target collection on first write
            // (with a password prompt), so a typo must fail here instead.
            match linux::collection_labels() {
                Ok(labels) if labels.contains(&label) => {}
                Ok(labels) => {
                    return Err(AppError::SecretStore(format!(
                        "{COLLECTION_ENV}={label:?} matches no Secret Service collection \
                         (labels are case-sensitive; available: {labels:?})"
                    )))
                }
                Err(detail) => return Err(AppError::SecretStore(detail)),
            }
            let store = Self::with_target(Some(label));
            store.probe()?;
            return Ok(store);
        }

        let store = Self::with_target(None);
        match store.probe() {
            Ok(()) => Ok(store),
            #[cfg(target_os = "linux")]
            Err(e) => match linux::fallback_collection() {
                Ok(Some(label)) => {
                    tracing::info!(
                        collection = %label,
                        "Secret Service has no default collection; using an existing one"
                    );
                    let store = Self::with_target(Some(label));
                    store.probe()?;
                    Ok(store)
                }
                Ok(None) => Err(e),
                Err(detail) => {
                    tracing::debug!(%detail, "could not inspect Secret Service collections");
                    Err(e)
                }
            },
            #[cfg(not(target_os = "linux"))]
            Err(e) => Err(e),
        }
    }

    fn with_target(target: Option<String>) -> Self {
        Self {
            service: SERVICE,
            target,
        }
    }

    /// Collection in use (Linux), for diagnostics.
    pub fn target(&self) -> Option<&str> {
        self.target.as_deref()
    }

    fn entry(&self, user: &str) -> Result<Entry, KeyringError> {
        match &self.target {
            Some(t) => Entry::new_with_target(t, self.service, user),
            None => Entry::new(self.service, user),
        }
    }

    /// A read of a non-existent entry returns `NoEntry` when the store works;
    /// on headless Linux it fails with `PlatformFailure` (no D-Bus session) or
    /// `NoStorageAccess` (no usable collection, or access denied).
    fn probe(&self) -> AppResult<()> {
        let entry = self.entry(PROBE_USER).map_err(describe)?;
        match entry.get_password() {
            Ok(_) | Err(KeyringError::NoEntry) => Ok(()),
            Err(e) => Err(describe(e)),
        }
    }

    pub fn get(&self, key: &str) -> AppResult<Option<Secret>> {
        let entry = self.entry(key).map_err(describe)?;
        match entry.get_password() {
            Ok(v) => Ok(Some(Zeroizing::new(v))),
            Err(KeyringError::NoEntry) => Ok(None),
            Err(e) => Err(describe(e)),
        }
    }

    pub fn set(&self, key: &str, value: &str) -> AppResult<()> {
        self.entry(key)
            .and_then(|entry| entry.set_password(value))
            .map_err(describe)
    }

    pub fn delete(&self, key: &str) -> AppResult<()> {
        let entry = self.entry(key).map_err(describe)?;
        match entry.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(e) => Err(describe(e)),
        }
    }
}

/// Maps keyring errors to actionable messages.
fn describe(e: KeyringError) -> AppError {
    let message = e.to_string();
    let hint = match &e {
        KeyringError::PlatformFailure(_) if cfg!(target_os = "linux") => {
            " (no Secret Service on the D-Bus session bus: start gnome-keyring, KWallet or KeePassXC's Secret Service integration)"
        }
        KeyringError::NoStorageAccess(_) if message.contains("no result found") => {
            " (the Secret Service has no usable collection: create a \"login\" keyring, e.g. in Seahorse)"
        }
        KeyringError::NoStorageAccess(_) => " (the credential store is locked or access was denied)",
        _ => "",
    };
    AppError::SecretStore(format!("{message}{hint}"))
}

#[cfg(target_os = "linux")]
mod linux {
    use dbus_secret_service::{EncryptionType, SecretService};

    /// Picks a persistent collection when the `default` alias is missing:
    /// a collection labeled `login` (gnome-keyring's convention) first, then
    /// the first other non-session collection. Returns `Ok(None)` when the
    /// alias exists (the original error has another cause) or nothing fits.
    pub fn fallback_collection() -> Result<Option<String>, String> {
        let ss = SecretService::connect(EncryptionType::Plain).map_err(|e| e.to_string())?;
        if ss.get_default_collection().is_ok() {
            return Ok(None);
        }
        let mut labels: Vec<String> = labels_of(&ss)?
            .into_iter()
            .filter(|l| !l.trim().is_empty())
            // The session collection lives in memory and is wiped at logout.
            .filter(|l| !l.eq_ignore_ascii_case("session"))
            .collect();
        // Exact labels are returned: keyring-rs matches them case-sensitively.
        labels.sort_by_key(|l| !l.eq_ignore_ascii_case("login"));
        Ok(labels.into_iter().next())
    }

    pub fn collection_labels() -> Result<Vec<String>, String> {
        let ss = SecretService::connect(EncryptionType::Plain).map_err(|e| e.to_string())?;
        labels_of(&ss)
    }

    fn labels_of(ss: &SecretService) -> Result<Vec<String>, String> {
        Ok(ss
            .get_all_collections()
            .map_err(|e| e.to_string())?
            .iter()
            .filter_map(|c| c.get_label().ok())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Against the real OS store (writes and deletes one item):
    /// `cargo test live_keyring -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_keyring_roundtrip() {
        let store = KeyringStore::open().expect("keyring usable");
        println!("collection: {:?}", store.target());
        let key = "__octonode_live_test__";
        store.set(key, "s3cret").expect("set");
        let got = store.get(key).expect("get").expect("present");
        assert_eq!(got.as_str(), "s3cret");
        store.delete(key).expect("delete");
        assert!(store.get(key).expect("get after delete").is_none());
    }
}
