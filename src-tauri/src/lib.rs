//! OctoNode backend.
//!
//! Module map:
//! * `models`   — domain + IPC types (mirrored in `src/types/models.ts`)
//! * `paths`    — cross-platform directories and path conversions
//! * `config`   — persisted, non-secret settings
//! * `secrets`  — OS keyring with encrypted-file fallback
//! * `git`      — libgit2 core: context isolation, graph, diff, staging, clone, CLI
//! * `hosting`  — GitHub / GitLab APIs (repository listing)
//! * `state`    — shared state and caches
//! * `commands` — the Tauri IPC surface

// Production code must surface errors as `AppError`, never panic on them.
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]

pub mod commands;
pub mod config;
pub mod error;
pub mod git;
pub mod hosting;
pub mod models;
pub mod paths;
pub mod secrets;
pub mod state;

use std::sync::Arc;

use tauri::Manager;
use tracing_subscriber::EnvFilter;

use crate::paths::AppPaths;
use crate::state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("OCTONODE_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let paths = AppPaths::resolve()?;
            let state = AppState::initialize(paths)?;
            app.manage(Arc::new(state));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::init_workspace,
            commands::unlock_vault,
            commands::lock_vault,
            commands::set_active_workspace,
            commands::create_workspace,
            commands::delete_workspace,
            commands::add_repository,
            commands::remove_repository,
            commands::create_account,
            commands::update_account,
            commands::delete_account,
            commands::set_account_secret,
            commands::bind_repository_identity,
            commands::get_commit_graph,
            commands::get_diff,
            commands::stage_changes,
            commands::fetch_remote,
            commands::get_settings,
            commands::update_settings,
            commands::change_master_password,
            commands::list_remote_repositories,
            commands::clone_repository,
            commands::cancel_clone,
            commands::open_repository_folder,
        ])
        .run(tauri::generate_context!());

    if let Err(e) = result {
        tracing::error!(error = %e, "OctoNode terminated with an error");
        std::process::exit(1);
    }
}
