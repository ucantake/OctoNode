//! Tauri IPC commands.
//!
//! Conventions:
//! * Every command is `async` and returns `Result<T, AppError>`; the error is
//!   serialized as `{ kind, message }` (see `error.rs`).
//! * libgit2 and Argon2 are blocking: they run on `spawn_blocking`, never on
//!   the IPC/async worker threads, so the UI never stalls on a large repo.
//! * The frontend addresses repositories by id; paths are resolved here from
//!   the persisted workspace, never trusted from the webview.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Deserialize;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::git::clone::{self as clone_engine, CloneProgress, CloneTarget, ProgressSink};
use crate::git::context::GitContext;
use crate::git::diff::{self, DiffParams, FULL_FILE_CONTEXT};
use crate::git::graph::{self, GraphOptions};
use crate::git::remote::{self, FetchOutcome};
use crate::git::{self as gitcore, stage};
use crate::hosting::{self, RemoteRepo};
use crate::models::{
    AccountInput, AccountView, BootstrapState, CloneRequest, CloneResult, DiffResult, DiffTarget,
    GraphPage, RepositoryRef, SettingsInput, SettingsView, StageRequest, VaultStatus, Workspace,
};
use crate::paths;
use crate::secrets::{ssh_passphrase_key, token_key};
use crate::state::AppState;

type AppStateArc<'a> = State<'a, Arc<AppState>>;

/// Hard cap on rows per IPC call; the UI pages with ~200-500.
const MAX_GRAPH_PAGE: usize = 5_000;

async fn blocking<T, F>(state: &AppStateArc<'_>, f: F) -> AppResult<T>
where
    T: Send + 'static,
    F: FnOnce(Arc<AppState>) -> AppResult<T> + Send + 'static,
{
    let state = Arc::clone(state.inner());
    tauri::async_runtime::spawn_blocking(move || f(state)).await?
}

// ---------------------------------------------------------------------------
// Workspace / bootstrap
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn init_workspace(state: AppStateArc<'_>) -> AppResult<BootstrapState> {
    blocking(&state, |s| s.bootstrap()).await
}

#[tauri::command]
pub async fn unlock_vault(
    state: AppStateArc<'_>,
    master_password: String,
) -> AppResult<BootstrapState> {
    let password = zeroize::Zeroizing::new(master_password);
    blocking(&state, move |s| {
        s.vault.unlock(&password)?;
        s.bootstrap()
    })
    .await
}

#[tauri::command]
pub async fn lock_vault(state: AppStateArc<'_>) -> AppResult<BootstrapState> {
    blocking(&state, |s| {
        s.vault.lock()?;
        s.bootstrap()
    })
    .await
}

#[tauri::command]
pub async fn set_active_workspace(state: AppStateArc<'_>, workspace_id: Uuid) -> AppResult<()> {
    blocking(&state, move |s| {
        s.update_config(|c| {
            c.workspace_mut(workspace_id)?;
            c.active_workspace_id = Some(workspace_id);
            Ok(())
        })
    })
    .await
}

#[tauri::command]
pub async fn create_workspace(
    state: AppStateArc<'_>,
    name: String,
    account_id: Option<Uuid>,
) -> AppResult<Workspace> {
    blocking(&state, move |s| {
        let name = name.trim().to_owned();
        if name.is_empty() {
            return Err(AppError::InvalidInput(
                "workspace name must not be empty".into(),
            ));
        }
        s.update_config(|c| {
            if let Some(id) = account_id {
                c.account(id)?;
            }
            let ws = Workspace {
                id: Uuid::new_v4(),
                name,
                account_id,
                repositories: Vec::new(),
            };
            c.workspaces.push(ws.clone());
            c.active_workspace_id.get_or_insert(ws.id);
            Ok(ws)
        })
    })
    .await
}

#[tauri::command]
pub async fn delete_workspace(state: AppStateArc<'_>, workspace_id: Uuid) -> AppResult<()> {
    blocking(&state, move |s| {
        s.update_config(|c| {
            let before = c.workspaces.len();
            c.workspaces.retain(|w| w.id != workspace_id);
            if c.workspaces.len() == before {
                return Err(AppError::not_found("workspace", workspace_id));
            }
            if c.active_workspace_id == Some(workspace_id) {
                c.active_workspace_id = c.workspaces.first().map(|w| w.id);
            }
            Ok(())
        })
    })
    .await
}

#[tauri::command]
pub async fn add_repository(
    state: AppStateArc<'_>,
    workspace_id: Uuid,
    path: String,
) -> AppResult<RepositoryRef> {
    blocking(&state, move |s| {
        register_repository(&s, workspace_id, &PathBuf::from(path))
    })
    .await
}

/// Adds an existing repository folder to a workspace (shared by "Open" and
/// "Clone").
fn register_repository(s: &AppState, workspace_id: Uuid, path: &Path) -> AppResult<RepositoryRef> {
    let canonical = paths::canonicalize(path)?;
    let repo = gitcore::open(&canonical)?;
    let root = repo
        .workdir()
        .map(paths::canonicalize)
        .transpose()?
        .unwrap_or_else(|| canonical.clone());
    let (organization, remote_url) = gitcore::origin_info(&repo);
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| paths::display(&root));

    s.update_config(|c| {
        if c.workspaces
            .iter()
            .flat_map(|w| &w.repositories)
            .any(|r| r.path == root)
        {
            return Err(AppError::InvalidInput(format!(
                "{} is already part of a workspace",
                paths::display(&root)
            )));
        }
        let repo_ref = RepositoryRef {
            id: Uuid::new_v4(),
            name,
            path: root,
            organization,
            remote_url,
        };
        c.workspace_mut(workspace_id)?
            .repositories
            .push(repo_ref.clone());
        Ok(repo_ref)
    })
}

#[tauri::command]
pub async fn remove_repository(
    state: AppStateArc<'_>,
    workspace_id: Uuid,
    repo_id: Uuid,
) -> AppResult<()> {
    blocking(&state, move |s| {
        s.update_config(|c| {
            let ws = c.workspace_mut(workspace_id)?;
            let before = ws.repositories.len();
            ws.repositories.retain(|r| r.id != repo_id);
            if ws.repositories.len() == before {
                return Err(AppError::not_found("repository", repo_id));
            }
            Ok(())
        })?;
        s.evict_graph(repo_id)
    })
    .await
}

// ---------------------------------------------------------------------------
// Accounts & secrets
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn create_account(state: AppStateArc<'_>, input: AccountInput) -> AppResult<AccountView> {
    blocking(&state, move |s| {
        let account = input.into_account(Uuid::new_v4())?;
        s.update_config(|c| {
            c.accounts.push(account.clone());
            Ok(())
        })?;
        Ok(s.account_view(account))
    })
    .await
}

#[tauri::command]
pub async fn update_account(
    state: AppStateArc<'_>,
    account_id: Uuid,
    input: AccountInput,
) -> AppResult<AccountView> {
    blocking(&state, move |s| {
        let account = input.into_account(account_id)?;
        s.update_config(|c| {
            let slot = c
                .accounts
                .iter_mut()
                .find(|a| a.id == account_id)
                .ok_or_else(|| AppError::not_found("account", account_id))?;
            *slot = account.clone();
            Ok(())
        })?;
        Ok(s.account_view(account))
    })
    .await
}

#[tauri::command]
pub async fn delete_account(state: AppStateArc<'_>, account_id: Uuid) -> AppResult<()> {
    blocking(&state, move |s| {
        s.update_config(|c| {
            c.account(account_id)?;
            c.accounts.retain(|a| a.id != account_id);
            // Workspaces fall back to the anonymous context; they are not
            // silently moved to another account.
            for ws in c
                .workspaces
                .iter_mut()
                .filter(|w| w.account_id == Some(account_id))
            {
                ws.account_id = None;
            }
            Ok(())
        })?;
        // Config first: a dangling secret is harmless, a dangling account
        // without its secret is merely "missing token".
        s.vault.delete(&token_key(account_id))?;
        s.vault.delete(&ssh_passphrase_key(account_id))
    })
    .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AccountSecretKind {
    Token,
    SshPassphrase,
}

/// Stores (or with `value: null`, removes) a secret for an account.
#[tauri::command]
pub async fn set_account_secret(
    state: AppStateArc<'_>,
    account_id: Uuid,
    kind: AccountSecretKind,
    value: Option<String>,
) -> AppResult<AccountView> {
    let value = value.map(zeroize::Zeroizing::new);
    blocking(&state, move |s| {
        let account = s.read_config(|c| c.account(account_id).cloned())?;
        let key = match kind {
            AccountSecretKind::Token => token_key(account_id),
            AccountSecretKind::SshPassphrase => ssh_passphrase_key(account_id),
        };
        match value.as_deref().map(|v| v.trim()) {
            Some(v) if !v.is_empty() => s.vault.set(&key, v)?,
            _ => s.vault.delete(&key)?,
        }
        Ok(s.account_view(account))
    })
    .await
}

/// Writes the workspace account's identity + SSH command into the repo's
/// local `.git/config` so terminal git matches the app.
#[tauri::command]
pub async fn bind_repository_identity(state: AppStateArc<'_>, repo_id: Uuid) -> AppResult<()> {
    blocking(&state, move |s| {
        let (path, account) = s.read_config(|c| {
            let (repo, account) = c.resolve_repo(repo_id)?;
            Ok((repo.path.clone(), account.cloned()))
        })?;
        let account = account.ok_or_else(|| {
            AppError::InvalidInput("this workspace is not bound to an account".into())
        })?;
        GitContext::for_account(Some(&account), &s.vault)?.bind_repository(&path)
    })
    .await
}

// ---------------------------------------------------------------------------
// Graph & diff
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn get_commit_graph(
    state: AppStateArc<'_>,
    repo_id: Uuid,
    offset: usize,
    limit: usize,
) -> AppResult<GraphPage> {
    blocking(&state, move |s| {
        let path = s.read_config(|c| Ok(c.resolve_repo(repo_id)?.0.path.clone()))?;
        let repo = gitcore::open(&path)?;
        let fp = graph::fingerprint(&repo)?;
        let graph = match s.cached_graph(repo_id, fp)? {
            Some(g) => g,
            None => {
                let started = std::time::Instant::now();
                let g = Arc::new(graph::build_graph(&repo, GraphOptions::default())?);
                tracing::debug!(
                    commits = g.rows.len(),
                    ms = started.elapsed().as_millis() as u64,
                    "commit graph laid out"
                );
                s.store_graph(repo_id, Arc::clone(&g))?;
                g
            }
        };
        Ok(graph.page(offset, limit.clamp(1, MAX_GRAPH_PAGE)))
    })
    .await
}

#[tauri::command]
pub async fn get_diff(
    state: AppStateArc<'_>,
    repo_id: Uuid,
    target: DiffTarget,
    context_lines: Option<u32>,
    ignore_whitespace: Option<bool>,
) -> AppResult<DiffResult> {
    blocking(&state, move |s| {
        let path = s.read_config(|c| Ok(c.resolve_repo(repo_id)?.0.path.clone()))?;
        let repo = gitcore::open(&path)?;
        let params = DiffParams {
            context_lines: context_lines.unwrap_or(3).min(FULL_FILE_CONTEXT),
            ignore_whitespace: ignore_whitespace.unwrap_or(false),
        };
        let d = diff::compute(&repo, &target, params, None)?;
        diff::to_result(&d, target, params)
    })
    .await
}

#[tauri::command]
pub async fn stage_changes(state: AppStateArc<'_>, request: StageRequest) -> AppResult<()> {
    blocking(&state, move |s| {
        let path = s.read_config(|c| Ok(c.resolve_repo(request.repo_id)?.0.path.clone()))?;
        let repo = gitcore::open(&path)?;
        stage::apply(&repo, &request)
    })
    .await
}

#[tauri::command]
pub async fn fetch_remote(
    state: AppStateArc<'_>,
    repo_id: Uuid,
    remote: String,
) -> AppResult<FetchOutcome> {
    let state = Arc::clone(state.inner());
    let (path, account) = state.read_config(|c| {
        let (repo, account) = c.resolve_repo(repo_id)?;
        Ok((repo.path.clone(), account.cloned()))
    })?;
    let ctx = GitContext::for_account(account.as_ref(), &state.vault)?;
    let outcome = remote::fetch(&ctx, path, remote).await?;
    state.evict_graph(repo_id)?;
    Ok(outcome)
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

fn settings_view(s: &AppState) -> AppResult<SettingsView> {
    let configured = s.read_config(|c| Ok(c.clone_directory.clone()))?;
    let effective = match &configured {
        Some(p) => p.clone(),
        None => paths::default_clone_dir()?,
    };
    Ok(SettingsView {
        clone_directory: paths::display(&effective),
        clone_directory_is_default: configured.is_none(),
        vault: s.vault.status()?,
        config_dir: paths::display(&s.paths.config_dir),
        data_dir: paths::display(&s.paths.data_dir),
        app_version: env!("CARGO_PKG_VERSION"),
    })
}

#[tauri::command]
pub async fn get_settings(state: AppStateArc<'_>) -> AppResult<SettingsView> {
    blocking(&state, |s| settings_view(&s)).await
}

#[tauri::command]
pub async fn update_settings(
    state: AppStateArc<'_>,
    input: SettingsInput,
) -> AppResult<SettingsView> {
    blocking(&state, move |s| {
        let dir = match input.clone_directory.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(raw) => {
                let p = PathBuf::from(raw);
                if !p.is_absolute() {
                    return Err(AppError::InvalidInput(format!(
                        "clone folder must be an absolute path: {raw:?}"
                    )));
                }
                Some(p)
            }
        };
        s.update_config(|c| {
            c.clone_directory = dir;
            Ok(())
        })?;
        settings_view(&s)
    })
    .await
}

/// Re-keys the encrypted-file vault; takes effect immediately, no restart.
#[tauri::command]
pub async fn change_master_password(
    state: AppStateArc<'_>,
    current_password: String,
    new_password: String,
) -> AppResult<VaultStatus> {
    let current = zeroize::Zeroizing::new(current_password);
    let new = zeroize::Zeroizing::new(new_password);
    blocking(&state, move |s| {
        s.vault.change_master_password(&current, &new)?;
        tracing::info!("master password changed");
        s.vault.status()
    })
    .await
}

// ---------------------------------------------------------------------------
// Remote repositories
// ---------------------------------------------------------------------------

/// Repositories the account can clone, from the GitHub / GitLab API.
#[tauri::command]
pub async fn list_remote_repositories(
    state: AppStateArc<'_>,
    account_id: Uuid,
) -> AppResult<Vec<RemoteRepo>> {
    let state = Arc::clone(state.inner());
    let account = state.read_config(|c| c.account(account_id).cloned())?;
    let token = state.vault.get(&token_key(account_id))?.ok_or_else(|| {
        AppError::InvalidInput(format!(
            "{} has no access token; add one to browse its repositories, or clone by URL",
            account.label
        ))
    })?;
    hosting::list_repositories(&account, &token).await
}

/// Clones a remote repository into the workspace, as the workspace's account.
/// Progress is emitted as `clone-progress` events carrying `request.cloneId`.
#[tauri::command]
pub async fn clone_repository(
    app: AppHandle,
    state: AppStateArc<'_>,
    request: CloneRequest,
) -> AppResult<CloneResult> {
    let state = Arc::clone(state.inner());
    let url = request.url.trim().to_owned();
    clone_engine::validate_remote_url(&url)?;

    let (account, configured_parent) = state.read_config(|c| {
        let ws = c
            .workspaces
            .iter()
            .find(|w| w.id == request.workspace_id)
            .ok_or_else(|| AppError::not_found("workspace", request.workspace_id))?;
        let account = match ws.account_id {
            Some(id) => Some(c.account(id)?.clone()),
            None => None,
        };
        Ok((account, c.clone_directory.clone()))
    })?;

    let parent = match request.parent_directory.as_deref().map(str::trim) {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => match configured_parent {
            Some(p) => p,
            None => paths::default_clone_dir()?,
        },
    };
    if !parent.is_absolute() {
        return Err(AppError::InvalidInput(
            "the clone folder must be an absolute path".into(),
        ));
    }
    let folder = match request.folder_name.as_deref().map(str::trim) {
        Some(f) if !f.is_empty() => f.to_owned(),
        _ => clone_engine::folder_name_from_url(&url).ok_or_else(|| {
            AppError::InvalidInput("cannot derive a folder name from the URL; enter one".into())
        })?,
    };
    let target = CloneTarget::prepare(&parent, &folder)?;
    let ctx = GitContext::for_account(account.as_ref(), &state.vault)?;

    let cancel = Arc::new(AtomicBool::new(false));
    state.register_clone(request.clone_id, Arc::clone(&cancel))?;
    let sink: ProgressSink = Arc::new(move |p: CloneProgress| {
        if let Err(e) = app.emit("clone-progress", &p) {
            tracing::debug!(error = %e, "could not emit clone progress");
        }
    });

    let outcome = clone_engine::clone_repository(
        ctx,
        url,
        &target,
        request.clone_id,
        Arc::clone(&cancel),
        sink,
    )
    .await;
    state.unregister_clone(request.clone_id)?;
    let transport = outcome?;

    let dest = target.dest.clone();
    let bind = request.bind_identity && account.is_some();
    let s = Arc::clone(&state);
    let repository = tauri::async_runtime::spawn_blocking(move || -> AppResult<RepositoryRef> {
        if bind {
            GitContext::for_account(account.as_ref(), &s.vault)?.bind_repository(&dest)?;
        }
        register_repository(&s, request.workspace_id, &dest)
    })
    .await??;
    Ok(CloneResult {
        repository,
        transport,
    })
}

/// Requests cancellation of a running clone; it stops at the next progress
/// callback and its partial folder is removed.
#[tauri::command]
pub async fn cancel_clone(state: AppStateArc<'_>, clone_id: Uuid) -> AppResult<bool> {
    Ok(match state.clone_cancel_flag(clone_id)? {
        Some(flag) => {
            flag.store(true, Ordering::Relaxed);
            true
        }
        None => false,
    })
}

/// Opens the repository's working folder in the OS file manager. The path is
/// resolved from the saved workspace, never taken from the webview, so the
/// UI cannot ask to open arbitrary locations.
#[tauri::command]
pub async fn open_repository_folder(
    app: AppHandle,
    state: AppStateArc<'_>,
    repo_id: Uuid,
) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;

    let path = state.read_config(|c| Ok(c.resolve_repo(repo_id)?.0.path.clone()))?;
    if !path.is_dir() {
        return Err(AppError::InvalidInput(format!(
            "the folder {} no longer exists; it may have been moved or deleted",
            paths::display(&path)
        )));
    }
    app.opener()
        .open_path(paths::display(&path), None::<&str>)
        .map_err(|e| AppError::Process(format!("could not open the file manager: {e}")))
}
