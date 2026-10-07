//! Shared application state (managed by Tauri as `Arc<AppState>`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use uuid::Uuid;

use crate::config::AppConfig;
use crate::error::AppResult;
use crate::git::graph::CommitGraph;
use crate::models::{AccountStatus, AccountView, BootstrapState, GitHostType, PlatformInfo};
use crate::paths::{self, AppPaths};
use crate::secrets::{token_key, SecretVault};

pub struct AppState {
    pub paths: AppPaths,
    config: RwLock<AppConfig>,
    pub vault: SecretVault,
    /// Laid-out commit graphs per repository, invalidated by ref fingerprint.
    graphs: Mutex<HashMap<Uuid, Arc<CommitGraph>>>,
}

impl AppState {
    pub fn initialize(paths: AppPaths) -> AppResult<Self> {
        let config = AppConfig::load(&paths.config_file())?;
        let vault = SecretVault::open(&paths, config.secret_backend);
        Ok(Self {
            paths,
            config: RwLock::new(config),
            vault,
            graphs: Mutex::new(HashMap::new()),
        })
    }

    pub fn read_config<R>(&self, f: impl FnOnce(&AppConfig) -> AppResult<R>) -> AppResult<R> {
        let cfg = self.config.read()?;
        f(&cfg)
    }

    /// Transactional update: `f` mutates a copy, which replaces the live
    /// config only after it was durably written to disk.
    pub fn update_config<R>(&self, f: impl FnOnce(&mut AppConfig) -> AppResult<R>) -> AppResult<R> {
        let mut guard = self.config.write()?;
        let mut draft = guard.clone();
        let result = f(&mut draft)?;
        draft.save(&self.paths.config_file())?;
        *guard = draft;
        Ok(result)
    }

    pub fn cached_graph(
        &self,
        repo_id: Uuid,
        fingerprint: u64,
    ) -> AppResult<Option<Arc<CommitGraph>>> {
        let graphs = self.graphs.lock()?;
        Ok(graphs
            .get(&repo_id)
            .filter(|g| g.fingerprint == fingerprint)
            .cloned())
    }

    pub fn store_graph(&self, repo_id: Uuid, graph: Arc<CommitGraph>) -> AppResult<()> {
        self.graphs.lock()?.insert(repo_id, graph);
        Ok(())
    }

    pub fn evict_graph(&self, repo_id: Uuid) -> AppResult<()> {
        self.graphs.lock()?.remove(&repo_id);
        Ok(())
    }

    pub fn account_status(&self, host: GitHostType, account_id: Uuid) -> AccountStatus {
        if host == GitHostType::Local {
            return AccountStatus::Ready;
        }
        match self.vault.get(&token_key(account_id)) {
            Ok(Some(_)) => AccountStatus::Ready,
            Ok(None) => AccountStatus::MissingToken,
            Err(crate::error::AppError::VaultLocked) => AccountStatus::Locked,
            Err(e) => {
                tracing::warn!(error = %e, "could not read token status");
                AccountStatus::MissingToken
            }
        }
    }

    pub fn account_view(&self, account: crate::models::Account) -> AccountView {
        let status = self.account_status(account.host, account.id);
        AccountView { account, status }
    }

    pub fn bootstrap(&self) -> AppResult<BootstrapState> {
        let (accounts, workspaces, active_workspace_id) = self.read_config(|c| {
            Ok((
                c.accounts.clone(),
                c.workspaces.clone(),
                c.active_workspace_id,
            ))
        })?;
        Ok(BootstrapState {
            accounts: accounts.into_iter().map(|a| self.account_view(a)).collect(),
            workspaces,
            active_workspace_id,
            vault: self.vault.status()?,
            platform: PlatformInfo::current(),
            config_dir: paths::display(&self.paths.config_dir),
        })
    }
}
