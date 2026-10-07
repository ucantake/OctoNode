//! Network operations: libgit2 first, `git` CLI fallback for SSH edge cases.

use std::path::{Path, PathBuf};

use git2::{ErrorClass, Repository};
use serde::Serialize;

use super::cli::run_git;
use super::context::GitContext;
use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FetchTransport {
    Libgit2,
    GitCli,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchOutcome {
    pub remote: String,
    pub transport: FetchTransport,
    pub received_objects: usize,
}

/// Fetches `remote_name` with the account's credentials.
pub async fn fetch(
    ctx: &GitContext,
    repo_path: PathBuf,
    remote_name: String,
) -> AppResult<FetchOutcome> {
    validate_remote_name(&remote_name)?;

    // libgit2 is blocking. `block_in_place` hands this worker's other tasks to
    // the rest of Tauri's multi-threaded runtime while the fetch runs, and lets
    // us borrow `ctx` (secrets are never cloned into a 'static task).
    let libgit2_result =
        tokio::task::block_in_place(|| fetch_libgit2(ctx, &repo_path, &remote_name));

    match libgit2_result {
        Ok(received) => Ok(FetchOutcome {
            remote: remote_name,
            transport: FetchTransport::Libgit2,
            received_objects: received,
        }),
        Err(AppError::Git(e)) if should_fallback(&e) => {
            tracing::info!(error = %e, "libgit2 fetch failed, retrying with git CLI");
            run_git(ctx, &repo_path, &["fetch", "--prune", "--", &remote_name]).await?;
            Ok(FetchOutcome {
                remote: remote_name,
                transport: FetchTransport::GitCli,
                received_objects: 0,
            })
        }
        Err(e) => Err(e),
    }
}

fn fetch_libgit2(ctx: &GitContext, repo_path: &Path, remote_name: &str) -> AppResult<usize> {
    let repo = Repository::open(repo_path)?;
    let mut remote = repo.find_remote(remote_name)?;
    let mut opts = ctx.fetch_options();
    // Empty refspec list = the remote's configured refspecs.
    remote.fetch::<&str>(&[], Some(&mut opts), None)?;
    Ok(remote.stats().received_objects())
}

/// SSH transport problems (unsupported key type, ssh_config aliases, proxy
/// commands) are what the CLI handles better. Auth failures over HTTPS are not
/// retried: the CLI would use the very same token.
fn should_fallback(e: &git2::Error) -> bool {
    matches!(e.class(), ErrorClass::Ssh) || e.message().contains("unsupported URL protocol")
}

/// Remote names are passed to a child process; reject anything that could be
/// parsed as an option.
fn validate_remote_name(name: &str) -> AppResult<()> {
    let ok = !name.is_empty()
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'));
    if ok {
        Ok(())
    } else {
        Err(AppError::InvalidInput(format!(
            "invalid remote name {name:?}"
        )))
    }
}
