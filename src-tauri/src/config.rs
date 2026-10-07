//! Persistent, non-secret configuration (`config.json`).

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::models::{Account, RepositoryRef, Workspace};
use crate::paths::atomic_write;
use crate::secrets::SecretBackendPreference;

const CONFIG_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    pub version: u32,
    #[serde(default)]
    pub accounts: Vec<Account>,
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    #[serde(default)]
    pub active_workspace_id: Option<Uuid>,
    #[serde(default)]
    pub secret_backend: SecretBackendPreference,
    /// Parent folder for new clones; `None` = `~/Projects`.
    #[serde(default)]
    pub clone_directory: Option<std::path::PathBuf>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            accounts: Vec::new(),
            workspaces: Vec::new(),
            active_workspace_id: None,
            secret_backend: SecretBackendPreference::Auto,
            clone_directory: None,
        }
    }
}

impl AppConfig {
    /// Loads the config; a missing file yields defaults. A corrupt file is
    /// moved aside (never silently overwritten) and defaults are used.
    pub fn load(path: &Path) -> AppResult<Self> {
        let bytes = match fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e.into()),
        };
        match serde_json::from_slice::<Self>(&bytes) {
            Ok(cfg) if cfg.version <= CONFIG_VERSION => Ok(cfg),
            Ok(cfg) => Err(AppError::Unsupported(format!(
                "config version {} is newer than this build supports ({CONFIG_VERSION})",
                cfg.version
            ))),
            Err(e) => {
                let backup = path.with_extension("json.corrupt");
                tracing::error!(error = %e, backup = %backup.display(), "corrupt config, moving aside");
                fs::rename(path, &backup)?;
                Ok(Self::default())
            }
        }
    }

    pub fn save(&self, path: &Path) -> AppResult<()> {
        atomic_write(path, &serde_json::to_vec_pretty(self)?)
    }

    pub fn account(&self, id: Uuid) -> AppResult<&Account> {
        self.accounts
            .iter()
            .find(|a| a.id == id)
            .ok_or_else(|| AppError::not_found("account", id))
    }

    pub fn workspace_mut(&mut self, id: Uuid) -> AppResult<&mut Workspace> {
        self.workspaces
            .iter_mut()
            .find(|w| w.id == id)
            .ok_or_else(|| AppError::not_found("workspace", id))
    }

    /// Resolves a repository and the account whose context it runs in.
    pub fn resolve_repo(&self, repo_id: Uuid) -> AppResult<(&RepositoryRef, Option<&Account>)> {
        for ws in &self.workspaces {
            if let Some(repo) = ws.repositories.iter().find(|r| r.id == repo_id) {
                let account = match ws.account_id {
                    Some(id) => Some(self.account(id)?),
                    None => None,
                };
                return Ok((repo, account));
            }
        }
        Err(AppError::not_found("repository", repo_id))
    }
}
