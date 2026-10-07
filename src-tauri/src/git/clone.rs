//! Cloning remote repositories under an account's isolated context.
//!
//! * libgit2 (`RepoBuilder`) with the account's credential callbacks, progress
//!   reporting and cooperative cancellation (the transfer callback returns
//!   `false` once the cancel flag is set);
//! * `git clone` CLI fallback for SSH setups libgit2 cannot handle;
//! * the destination is removed again on failure or cancellation, so a
//!   half-cloned folder never lingers or blocks a retry.

use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use git2::build::{CheckoutBuilder, RepoBuilder};
use git2::FetchOptions;
use serde::Serialize;
use uuid::Uuid;

use super::cli::run_git;
use super::context::GitContext;
use super::remote::should_fallback;
use crate::error::{AppError, AppResult};
use crate::paths::validate_folder_name;

/// UI updates are throttled to this rate; libgit2 calls back per object.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ClonePhase {
    Receiving,
    Resolving,
    Checkout,
    /// libgit2 gave up on the SSH transport; `git clone` is running (no
    /// granular progress).
    Fallback,
    Done,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloneProgress {
    pub clone_id: Uuid,
    pub phase: ClonePhase,
    pub received_objects: usize,
    pub total_objects: usize,
    pub indexed_deltas: usize,
    pub total_deltas: usize,
    pub received_bytes: usize,
    pub checkout_done: usize,
    pub checkout_total: usize,
}

impl CloneProgress {
    fn new(clone_id: Uuid, phase: ClonePhase) -> Self {
        Self {
            clone_id,
            phase,
            received_objects: 0,
            total_objects: 0,
            indexed_deltas: 0,
            total_deltas: 0,
            received_bytes: 0,
            checkout_done: 0,
            checkout_total: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CloneTransport {
    Libgit2,
    GitCli,
}

pub type ProgressSink = Arc<dyn Fn(CloneProgress) + Send + Sync>;

/// Accepts network URLs only: `https://`, `ssh://`, `git://` and scp-like
/// `user@host:path`. Local paths, `file://` and helper transports such as
/// `ext::` are rejected: a clone URL comes from the user or a hosting API and
/// must never execute commands or read arbitrary local repositories.
pub fn validate_remote_url(url: &str) -> AppResult<()> {
    let url = url.trim();
    let bad = |why: &str| {
        Err(AppError::InvalidInput(format!(
            "unsupported clone URL {url:?}: {why}"
        )))
    };
    if url.is_empty()
        || url.starts_with('-')
        || url.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return bad("empty, starts with '-' or contains whitespace");
    }
    if let Some((scheme, rest)) = url.split_once("://") {
        return match scheme.to_ascii_lowercase().as_str() {
            "https" | "ssh" | "git" if !rest.is_empty() => Ok(()),
            "http" => bad("plain http would send credentials unencrypted; use https"),
            _ => bad("only https://, ssh:// and git:// are allowed"),
        };
    }
    if url.contains("::") {
        return bad("remote helper transports are not allowed");
    }
    // scp-like: [user@]host:path. A '/' or '\\' before the colon means a
    // local path (./a:b); a one-letter "host" is a Windows drive (C:\\x).
    let Some((host, path)) = url.split_once(':') else {
        return bad("not a recognized remote URL");
    };
    let hostname = host.rsplit('@').next().unwrap_or(host);
    let windows_drive = hostname.len() == 1 && hostname.chars().all(|c| c.is_ascii_alphabetic());
    if hostname.is_empty() || host.contains(['/', '\\']) || path.is_empty() || windows_drive {
        return bad("not a recognized remote URL");
    }
    Ok(())
}

/// `https://github.com/acme/api.git` → `api`; `git@host:group/sub/tool` → `tool`.
pub fn folder_name_from_url(url: &str) -> Option<String> {
    let path = match url.split_once("://") {
        Some((_, rest)) => rest.split_once('/').map(|(_, p)| p)?,
        None => url.split_once(':').map(|(_, p)| p)?,
    };
    let last = path.trim_end_matches('/').rsplit('/').next()?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    (!name.is_empty() && validate_folder_name(name).is_ok()).then(|| name.to_owned())
}

/// The folder a clone goes into, and how to undo it.
#[derive(Debug)]
pub struct CloneTarget {
    pub dest: PathBuf,
    /// An empty folder already existed there; restore it on cleanup.
    existed_empty: bool,
}

impl CloneTarget {
    /// Creates `parent` if needed. `parent/folder` must not exist, or be an
    /// empty directory (as `git clone` itself requires).
    pub fn prepare(parent: &Path, folder: &str) -> AppResult<Self> {
        validate_folder_name(folder)?;
        fs::create_dir_all(parent)?;
        let dest = parent.join(folder);
        let existed_empty = match fs::read_dir(&dest) {
            Ok(mut entries) => {
                if entries.next().is_some() {
                    return Err(AppError::InvalidInput(format!(
                        "{} already exists and is not empty",
                        dest.display()
                    )));
                }
                true
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(e.into()),
        };
        Ok(Self {
            dest,
            existed_empty,
        })
    }

    /// Removes whatever a failed or cancelled clone left behind.
    pub fn cleanup(&self) {
        if let Err(e) = fs::remove_dir_all(&self.dest) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(error = %e, dest = %self.dest.display(), "could not remove partial clone");
            }
        }
        if self.existed_empty {
            let _ = fs::create_dir_all(&self.dest);
        }
    }
}

/// Clones `url` into `target.dest`. On any error (including cancellation)
/// the destination is cleaned up before returning.
pub async fn clone_repository(
    ctx: GitContext,
    url: String,
    target: &CloneTarget,
    clone_id: Uuid,
    cancel: Arc<AtomicBool>,
    sink: ProgressSink,
) -> AppResult<CloneTransport> {
    let dest = target.dest.clone();
    let (ctx, result) = {
        let (url, cancel, sink) = (url.clone(), Arc::clone(&cancel), Arc::clone(&sink));
        tokio::task::spawn_blocking(move || {
            let r = clone_libgit2(&ctx, &url, &dest, clone_id, &cancel, &sink);
            (ctx, r)
        })
        .await
        .map_err(|e| AppError::Internal(format!("clone task failed: {e}")))?
    };

    let result = match result {
        Ok(()) => Ok(CloneTransport::Libgit2),
        Err(AppError::Git(e)) if should_fallback(&e) && !cancel.load(Ordering::Relaxed) => {
            tracing::info!(error = %e, "libgit2 clone failed, retrying with git CLI");
            target.cleanup();
            sink(CloneProgress::new(clone_id, ClonePhase::Fallback));
            clone_cli(&ctx, &url, target)
                .await
                .map(|()| CloneTransport::GitCli)
        }
        Err(e) => Err(e),
    };

    match result {
        Ok(transport) => {
            sink(CloneProgress::new(clone_id, ClonePhase::Done));
            Ok(transport)
        }
        Err(e) => {
            target.cleanup();
            Err(e)
        }
    }
}

fn clone_libgit2(
    ctx: &GitContext,
    url: &str,
    dest: &Path,
    clone_id: Uuid,
    cancel: &AtomicBool,
    sink: &ProgressSink,
) -> AppResult<()> {
    if cancel.load(Ordering::Relaxed) {
        return Err(AppError::Cancelled);
    }
    let last_emit = Cell::new(Instant::now() - PROGRESS_INTERVAL);
    let throttled = |p: CloneProgress, force: bool| {
        if force || last_emit.get().elapsed() >= PROGRESS_INTERVAL {
            last_emit.set(Instant::now());
            sink(p);
        }
    };

    let mut callbacks = ctx.remote_callbacks();
    callbacks.transfer_progress(|stats| {
        let phase = if stats.received_objects() < stats.total_objects() {
            ClonePhase::Receiving
        } else {
            ClonePhase::Resolving
        };
        throttled(
            CloneProgress {
                received_objects: stats.received_objects(),
                total_objects: stats.total_objects(),
                indexed_deltas: stats.indexed_deltas(),
                total_deltas: stats.total_deltas(),
                received_bytes: stats.received_bytes(),
                ..CloneProgress::new(clone_id, phase)
            },
            false,
        );
        // Returning false aborts the transfer.
        !cancel.load(Ordering::Relaxed)
    });

    let mut fetch = FetchOptions::new();
    fetch.remote_callbacks(callbacks);
    fetch.download_tags(git2::AutotagOption::All);
    // Honor http.proxy / HTTPS_PROXY like the git CLI does.
    let mut proxy = git2::ProxyOptions::new();
    proxy.auto();
    fetch.proxy_options(proxy);

    let mut checkout = CheckoutBuilder::new();
    checkout.progress(|_, done, total| {
        throttled(
            CloneProgress {
                checkout_done: done,
                checkout_total: total,
                ..CloneProgress::new(clone_id, ClonePhase::Checkout)
            },
            done == total,
        );
    });

    let result = RepoBuilder::new()
        .fetch_options(fetch)
        .with_checkout(checkout)
        .clone(url, dest);

    // The transfer callback is the only point libgit2 can abort at, and it is
    // not called for every transport (local clones skip it); a cancel that
    // arrives outside it still discards the result.
    match result {
        _ if cancel.load(Ordering::Relaxed) => Err(AppError::Cancelled),
        Ok(_) => Ok(()),
        // Authentication problems become AuthRequired (and are therefore
        // never retried with the CLI, which would bypass account isolation).
        Err(e) => Err(ctx.explain(e)),
    }
}

async fn clone_cli(ctx: &GitContext, url: &str, target: &CloneTarget) -> AppResult<()> {
    let parent = target
        .dest
        .parent()
        .ok_or_else(|| AppError::Internal("clone destination has no parent".into()))?;
    let dest = target
        .dest
        .to_str()
        .ok_or_else(|| AppError::Unsupported("destination path is not valid UTF-8".into()))?;
    // `--` ends option parsing: neither URL nor path can be read as a flag.
    run_git(ctx, parent, &["clone", "--", url, dest]).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_url_validation() {
        for ok in [
            "https://github.com/acme/api.git",
            "ssh://git@gitlab.com:2222/group/x.git",
            "git@github.com:acme/api.git",
            "github.com:acme/api",
            "git://example.org/x.git",
        ] {
            assert!(validate_remote_url(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "-uhttps://x",
            "file:///etc",
            "/srv/repos/x.git",
            "./a:b",
            "C:\\repos\\x",
            "C:/repos/x",
            "ext::sh -c touch% /tmp/pwned",
            "http://github.com/acme/api.git",
            "https://",
            "https://github.com/a b",
        ] {
            assert!(validate_remote_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn folder_names_from_urls() {
        assert_eq!(
            folder_name_from_url("https://github.com/acme/api.git").as_deref(),
            Some("api")
        );
        assert_eq!(
            folder_name_from_url("git@git.corp:group/sub/tool").as_deref(),
            Some("tool")
        );
        assert_eq!(
            folder_name_from_url("ssh://h/x/repo.git/").as_deref(),
            Some("repo")
        );
        assert_eq!(folder_name_from_url("https://github.com/"), None);
    }

    fn bare_origin() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().expect("tempdir");
        let work = git2::Repository::init(dir.path().join("work")).expect("init");
        std::fs::write(dir.path().join("work/readme.md"), "hello\n").expect("write");
        let mut index = work.index().expect("index");
        index.add_path(Path::new("readme.md")).expect("add");
        let tree = work
            .find_tree(index.write_tree().expect("tree"))
            .expect("find");
        let sig = git2::Signature::now("T", "t@example.com").expect("sig");
        work.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .expect("commit");
        let bare = dir.path().join("origin.git");
        git2::Repository::clone(dir.path().join("work").to_str().expect("utf8"), &bare)
            .expect("bare clone");
        let url = bare.to_str().expect("utf8").to_owned();
        (dir, url)
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime")
    }

    #[test]
    fn clones_with_progress_and_reports_done() {
        let (_origin_dir, url) = bare_origin();
        let parent = tempfile::tempdir().expect("tempdir");
        let target = CloneTarget::prepare(parent.path(), "copy").expect("prepare");
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink: ProgressSink = {
            let events = Arc::clone(&events);
            Arc::new(move |p: CloneProgress| events.lock().expect("lock").push(p.phase))
        };
        let transport = runtime()
            .block_on(clone_repository(
                GitContext::anonymous(),
                url,
                &target,
                Uuid::new_v4(),
                Arc::new(AtomicBool::new(false)),
                sink,
            ))
            .expect("clone");
        assert_eq!(transport, CloneTransport::Libgit2);
        assert_eq!(
            std::fs::read_to_string(target.dest.join("readme.md")).expect("checked out"),
            "hello\n"
        );
        assert_eq!(events.lock().expect("lock").last(), Some(&ClonePhase::Done));
    }

    #[test]
    fn cancelled_clone_leaves_nothing_behind() {
        let (_origin_dir, url) = bare_origin();
        let parent = tempfile::tempdir().expect("tempdir");
        let target = CloneTarget::prepare(parent.path(), "copy").expect("prepare");
        let err = runtime()
            .block_on(clone_repository(
                GitContext::anonymous(),
                url,
                &target,
                Uuid::new_v4(),
                Arc::new(AtomicBool::new(true)),
                Arc::new(|_| {}),
            ))
            .expect_err("cancelled");
        assert_eq!(err.kind(), "cancelled");
        assert!(!target.dest.exists(), "partial clone removed");
    }

    #[test]
    fn refuses_non_empty_destination() {
        let parent = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(parent.path().join("taken")).expect("mkdir");
        std::fs::write(parent.path().join("taken/file"), "x").expect("write");
        assert!(CloneTarget::prepare(parent.path(), "taken").is_err());
        std::fs::create_dir(parent.path().join("empty")).expect("mkdir");
        assert!(CloneTarget::prepare(parent.path(), "empty").is_ok());
    }

    /// Real network clone with progress and a mid-transfer cancel:
    /// `OCTONODE_LIVE_CLONE_URL=https://github.com/<repo>.git cargo test live_clone -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_clone() {
        let Ok(url) = std::env::var("OCTONODE_LIVE_CLONE_URL") else {
            return;
        };
        let parent = tempfile::tempdir().expect("tempdir");
        let rt = runtime();

        let phases = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink: ProgressSink = {
            let phases = Arc::clone(&phases);
            Arc::new(move |p: CloneProgress| {
                phases
                    .lock()
                    .expect("lock")
                    .push((p.phase, p.received_objects, p.total_objects))
            })
        };
        let target = CloneTarget::prepare(parent.path(), "full").expect("prepare");
        let t = rt
            .block_on(clone_repository(
                GitContext::anonymous(),
                url.clone(),
                &target,
                Uuid::new_v4(),
                Arc::new(AtomicBool::new(false)),
                sink,
            ))
            .expect("clone");
        let phases = phases.lock().expect("lock");
        println!(
            "transport {t:?}, {} progress events, last {:?}",
            phases.len(),
            phases.last()
        );
        assert!(phases
            .iter()
            .any(|p| p.0 == ClonePhase::Receiving || p.0 == ClonePhase::Resolving));

        // Cancel as soon as the first objects arrive.
        let cancel = Arc::new(AtomicBool::new(false));
        let sink: ProgressSink = {
            let cancel = Arc::clone(&cancel);
            Arc::new(move |p: CloneProgress| {
                if p.received_objects > 0 {
                    cancel.store(true, Ordering::Relaxed);
                }
            })
        };
        let target = CloneTarget::prepare(parent.path(), "cancelled").expect("prepare");
        let err = rt
            .block_on(clone_repository(
                GitContext::anonymous(),
                url,
                &target,
                Uuid::new_v4(),
                Arc::clone(&cancel),
                sink,
            ))
            .expect_err("cancelled");
        assert_eq!(err.kind(), "cancelled");
        assert!(!target.dest.exists());
        println!("mid-transfer cancel ok");
    }

    /// A private (or missing) HTTPS repository with an account that has no
    /// token must fail as `authRequired` with an actionable message:
    /// `OCTONODE_LIVE_PRIVATE_URL=https://github.com/<owner>/<private>.git cargo test live_private -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_private_without_token() {
        let Ok(url) = std::env::var("OCTONODE_LIVE_PRIVATE_URL") else {
            return;
        };
        let account = crate::models::Account {
            id: Uuid::new_v4(),
            label: "Personal GitHub".into(),
            host: crate::models::GitHostType::GitHub,
            api_base_url: None,
            username: "me".into(),
            avatar_url: None,
            identity: crate::models::GitIdentity {
                name: "Me".into(),
                email: "me@example.com".into(),
            },
            ssh: crate::models::SshSettings::default(),
            color: None,
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = crate::paths::AppPaths::from_dirs(
            dir.path().join("c"),
            dir.path().join("d"),
            dir.path().join("x"),
        )
        .expect("paths");
        let vault = crate::secrets::SecretVault::open(
            &paths,
            crate::secrets::SecretBackendPreference::EncryptedFile,
        );
        vault.unlock("unit-test-password").expect("unlock");
        let ctx = GitContext::for_account(Some(&account), &vault).expect("ctx");
        let target = CloneTarget::prepare(dir.path(), "private").expect("prepare");
        let err = runtime()
            .block_on(clone_repository(
                ctx,
                url,
                &target,
                Uuid::new_v4(),
                Arc::new(AtomicBool::new(false)),
                Arc::new(|_| {}),
            ))
            .expect_err("must need auth");
        println!("{}: {err}", err.kind());
        assert_eq!(err.kind(), "authRequired");
        assert!(!target.dest.exists());
    }
}
