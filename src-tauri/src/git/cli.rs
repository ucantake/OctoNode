//! `git` CLI fallback via `tokio::process`.
//!
//! Used where libgit2 is weaker than OpenSSH + git: `~/.ssh/config` host
//! aliases, `ProxyJump`, FIDO (`-sk`) keys, and exotic auth. Isolation comes
//! from [`GitContext::apply_to_command`].

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use super::context::GitContext;
use crate::error::{AppError, AppResult};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug)]
pub struct GitOutput {
    pub stdout: String,
    pub stderr: String,
}

/// Runs `git <args>` in `repo_path` under the given context.
pub async fn run_git(ctx: &GitContext, repo_path: &Path, args: &[&str]) -> AppResult<GitOutput> {
    let mut cmd = Command::new(git_executable());
    cmd.args(args)
        .current_dir(repo_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Make output parseable regardless of the user's locale.
        .env("LC_ALL", "C")
        .env("GIT_PAGER", "cat")
        // If the future is dropped (timeout, app shutdown) the child dies too.
        .kill_on_drop(true);

    #[cfg(windows)]
    {
        // CREATE_NO_WINDOW: don't flash a console window for each git call.
        cmd.creation_flags(0x0800_0000);
    }

    ctx.apply_to_command(&mut cmd);

    let output = tokio::time::timeout(DEFAULT_TIMEOUT, cmd.output())
        .await
        .map_err(|_| AppError::Process(format!("git {} timed out", args.join(" "))))?
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AppError::Process("git executable not found on PATH".into())
            } else {
                AppError::Io(e)
            }
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !output.status.success() {
        return Err(AppError::Process(format!(
            "git {} failed ({}): {}",
            args.first().copied().unwrap_or_default(),
            output.status,
            stderr.trim()
        )));
    }
    Ok(GitOutput { stdout, stderr })
}

/// `git` resolves through PATH on all platforms (`git.exe` on Windows via
/// PATHEXT handling in `std::process`). `OCTONODE_GIT` overrides it, e.g. for a
/// portable Git for Windows install.
fn git_executable() -> std::ffi::OsString {
    std::env::var_os("OCTONODE_GIT").unwrap_or_else(|| "git".into())
}
