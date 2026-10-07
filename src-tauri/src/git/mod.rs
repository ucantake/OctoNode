//! Git core: everything that talks to libgit2 or the `git` executable.

pub mod cli;
pub mod context;
pub mod diff;
pub mod graph;
pub mod remote;
pub mod stage;

#[cfg(test)]
mod integration_tests;

use std::path::Path;

use git2::Repository;

use crate::error::{AppError, AppResult};

/// Opens a repository at exactly `path` (no upward discovery, so a stale
/// workspace entry cannot silently resolve to a parent repository).
pub fn open(path: &Path) -> AppResult<Repository> {
    Repository::open_ext(
        path,
        git2::RepositoryOpenFlags::NO_SEARCH,
        std::iter::empty::<&std::ffi::OsStr>(),
    )
    .map_err(|e| match e.code() {
        git2::ErrorCode::NotFound => {
            AppError::InvalidInput(format!("{} is not a git repository", path.display()))
        }
        _ => AppError::Git(e),
    })
}

/// Best-effort `(organization, remote_url)` from `origin`, for the sidebar
/// tree: `git@github.com:acme/api.git` → `acme`.
pub fn origin_info(repo: &Repository) -> (Option<String>, Option<String>) {
    let Ok(remote) = repo.find_remote("origin") else {
        return (None, None);
    };
    let Some(url) = remote.url().map(str::to_owned) else {
        return (None, None);
    };
    (organization_from_url(&url), Some(url))
}

pub fn organization_from_url(url: &str) -> Option<String> {
    // scp-like syntax: user@host:org/sub/repo.git
    let path = if !url.contains("://") {
        url.split_once(':').map(|(_, p)| p.to_owned())
    } else {
        url::Url::parse(url).ok().map(|u| u.path().to_owned())
    }?;
    let segments: Vec<&str> = path
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    if segments.len() < 2 {
        return None;
    }
    // GitLab supports nested groups: keep everything but the repo name.
    Some(segments[..segments.len() - 1].join("/"))
}

#[cfg(test)]
mod tests {
    use super::organization_from_url;

    #[test]
    fn org_parsing() {
        assert_eq!(
            organization_from_url("git@github.com:acme/api.git").as_deref(),
            Some("acme")
        );
        assert_eq!(
            organization_from_url("https://gitlab.corp.io/platform/infra/tools.git").as_deref(),
            Some("platform/infra")
        );
        assert_eq!(organization_from_url("/srv/repos/local.git"), None);
    }
}
