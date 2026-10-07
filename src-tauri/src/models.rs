//! Domain model and IPC data-transfer types.
//!
//! All types serialize as camelCase and are mirrored 1:1 in
//! `src/types/models.ts`. Keep the two files in sync.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

// ---------------------------------------------------------------------------
// Accounts & workspaces
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GitHostType {
    GitHub,
    GitLabCloud,
    /// Self-managed GitLab; requires `Account::api_base_url`.
    GitLabSelfHosted,
    /// No remote host: local repositories only (identity + SSH still apply).
    Local,
}

impl GitHostType {
    pub fn default_api_base(self) -> Option<&'static str> {
        match self {
            Self::GitHub => Some("https://api.github.com"),
            Self::GitLabCloud => Some("https://gitlab.com/api/v4"),
            Self::GitLabSelfHosted | Self::Local => None,
        }
    }

    /// Username to pair with a personal access token for HTTPS transport.
    pub fn https_token_username(self) -> &'static str {
        match self {
            Self::GitHub => "x-access-token",
            Self::GitLabCloud | Self::GitLabSelfHosted => "oauth2",
            Self::Local => "git",
        }
    }
}

/// Author identity written into commits made in this account's context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitIdentity {
    pub name: String,
    pub email: String,
}

impl GitIdentity {
    pub fn validate(&self) -> AppResult<()> {
        let name = self.name.trim();
        let email = self.email.trim();
        if name.is_empty() {
            return Err(AppError::InvalidInput(
                "author name must not be empty".into(),
            ));
        }
        // Deliberately loose: git itself accepts almost anything, but these
        // characters would corrupt the commit header.
        if email.is_empty() || !email.contains('@') || email.contains(['<', '>', '\n']) {
            return Err(AppError::InvalidInput(format!(
                "invalid author email: {email:?}"
            )));
        }
        if name.contains(['<', '>', '\n']) {
            return Err(AppError::InvalidInput(format!(
                "invalid author name: {name:?}"
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshSettings {
    /// Private key used for this account. Stored as a native path; never
    /// string-concatenated (see `paths.rs`).
    pub private_key_path: Option<PathBuf>,
    /// Fall back to the running ssh-agent / Pageant when no key path is set.
    pub use_agent: bool,
}

/// Persisted account. The access token is NOT part of this struct; it lives in
/// the secret vault under `token_key(account.id)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: Uuid,
    pub label: String,
    pub host: GitHostType,
    /// REST API root, e.g. `https://git.example.com/api/v4`.
    pub api_base_url: Option<String>,
    pub username: String,
    pub avatar_url: Option<String>,
    pub identity: GitIdentity,
    pub ssh: SshSettings,
    /// Accent color (`#rrggbb`) for the sidebar.
    pub color: Option<String>,
}

impl Account {
    pub fn effective_api_base(&self) -> Option<&str> {
        self.api_base_url
            .as_deref()
            .or_else(|| self.host.default_api_base())
    }

    /// Hostname used to match remotes (`git@host:org/repo`, `https://host/...`).
    pub fn remote_host(&self) -> Option<String> {
        match self.host {
            GitHostType::GitHub => Some("github.com".into()),
            GitHostType::GitLabCloud => Some("gitlab.com".into()),
            GitHostType::GitLabSelfHosted => self
                .api_base_url
                .as_deref()
                .and_then(|u| url::Url::parse(u).ok())
                .and_then(|u| u.host_str().map(str::to_owned)),
            GitHostType::Local => None,
        }
    }
}

/// Input for creating/updating an account from the UI.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInput {
    pub label: String,
    pub host: GitHostType,
    pub api_base_url: Option<String>,
    pub username: String,
    pub avatar_url: Option<String>,
    pub identity: GitIdentity,
    pub ssh: SshSettings,
    pub color: Option<String>,
}

impl AccountInput {
    /// Validates and normalizes user input into a persisted [`Account`].
    pub fn into_account(self, id: Uuid) -> AppResult<Account> {
        let label = self.label.trim().to_owned();
        if label.is_empty() {
            return Err(AppError::InvalidInput(
                "account label must not be empty".into(),
            ));
        }
        self.identity.validate()?;

        let api_base_url = match (self.host, self.api_base_url.as_deref().map(str::trim)) {
            (GitHostType::GitLabSelfHosted, None | Some("")) => {
                return Err(AppError::InvalidInput(
                    "self-hosted GitLab requires an API base URL".into(),
                ))
            }
            (GitHostType::Local, _) | (_, None | Some("")) => None,
            (_, Some(raw)) => Some(normalize_api_url(raw)?),
        };

        if let Some(color) = &self.color {
            let ok = color.len() == 7
                && color.starts_with('#')
                && color[1..].chars().all(|c| c.is_ascii_hexdigit());
            if !ok {
                return Err(AppError::InvalidInput(format!("invalid color {color:?}")));
            }
        }

        Ok(Account {
            id,
            label,
            host: self.host,
            api_base_url,
            username: self.username.trim().to_owned(),
            avatar_url: self.avatar_url.filter(|s| !s.trim().is_empty()),
            identity: GitIdentity {
                name: self.identity.name.trim().to_owned(),
                email: self.identity.email.trim().to_owned(),
            },
            ssh: self.ssh,
            color: self.color,
        })
    }
}

/// Accepts `https://host[/path]`; plain `http` only for loopback (dev GitLab).
fn normalize_api_url(raw: &str) -> AppResult<String> {
    let parsed = url::Url::parse(raw)
        .map_err(|e| AppError::InvalidInput(format!("invalid API URL {raw:?}: {e}")))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| AppError::InvalidInput(format!("API URL has no host: {raw:?}")))?;
    let loopback = matches!(host, "localhost" | "127.0.0.1" | "[::1]");
    match parsed.scheme() {
        "https" => {}
        "http" if loopback => {}
        other => {
            return Err(AppError::InvalidInput(format!(
                "API URL must use https (got {other}://)"
            )))
        }
    }
    Ok(parsed.as_str().trim_end_matches('/').to_owned())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryRef {
    pub id: Uuid,
    pub name: String,
    /// Canonical absolute path (no `\\?\` prefix on Windows).
    pub path: PathBuf,
    /// Organization / group for the sidebar tree (derived from the remote).
    pub organization: Option<String>,
    pub remote_url: Option<String>,
}

/// A workspace binds a set of repositories to (at most) one account. Every git
/// operation in those repositories runs in that account's isolated context.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: Uuid,
    pub name: String,
    pub account_id: Option<Uuid>,
    pub repositories: Vec<RepositoryRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AccountStatus {
    /// Token present (or not needed for local accounts).
    Ready,
    MissingToken,
    /// Token presence unknown until the fallback vault is unlocked.
    Locked,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    #[serde(flatten)]
    pub account: Account,
    pub status: AccountStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SecretBackendKind {
    OsKeyring,
    EncryptedFile,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultStatus {
    pub backend: SecretBackendKind,
    pub locked: bool,
    /// For the file backend: whether a vault file exists (unlock vs. create).
    pub initialized: bool,
    /// Why the OS keyring was not used, if it was not.
    pub fallback_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformInfo {
    /// `"linux" | "windows" | "macos"`
    pub os: &'static str,
    pub path_separator: char,
    /// The key the UI renders as "Mod": `"Meta"` on macOS, `"Control"` elsewhere.
    pub primary_modifier: &'static str,
}

impl PlatformInfo {
    pub fn current() -> Self {
        let os = std::env::consts::OS;
        Self {
            os,
            path_separator: std::path::MAIN_SEPARATOR,
            primary_modifier: if os == "macos" { "Meta" } else { "Control" },
        }
    }
}

/// Everything the UI needs on startup, in one round-trip.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapState {
    pub accounts: Vec<AccountView>,
    pub workspaces: Vec<Workspace>,
    pub active_workspace_id: Option<Uuid>,
    pub vault: VaultStatus,
    pub platform: PlatformInfo,
    pub config_dir: String,
}

// ---------------------------------------------------------------------------
// Commit graph
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RefKind {
    LocalBranch,
    RemoteBranch,
    Tag,
    /// Detached HEAD.
    Head,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefBadge {
    pub name: String,
    pub kind: RefKind,
    /// The branch HEAD points to.
    pub is_head: bool,
}

/// A line segment entering a row from the row above: `(fromLane, toLane, color)`.
/// Serialized as a 3-tuple to keep 50k-row payloads small.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GraphEdge(pub u16, pub u16, pub u8);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitNode {
    pub id: String,
    pub short_id: String,
    pub summary: String,
    pub author_name: String,
    pub author_email: String,
    /// Seconds since the Unix epoch (author time).
    pub time: i64,
    pub parents: Vec<String>,
    /// Column of the commit dot.
    pub lane: u16,
    pub color: u8,
    /// Segments drawn between the previous row's center and this row's center.
    pub edges: Vec<GraphEdge>,
    pub refs: Vec<RefBadge>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphPage {
    pub total: usize,
    pub offset: usize,
    /// Widest row in the whole graph; lets the UI size the gutter once.
    pub max_lanes: u16,
    /// History was cut at `GraphOptions::max_commits`.
    pub truncated: bool,
    pub rows: Vec<CommitNode>,
}

// ---------------------------------------------------------------------------
// Diffs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum DiffTarget {
    /// Index -> working tree (unstaged changes, incl. untracked files).
    WorkingTree,
    /// HEAD -> index (staged changes).
    Index,
    /// First parent -> commit (empty tree for root commits).
    Commit { id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiffLineKind {
    Context,
    Addition,
    Deletion,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub old_lineno: Option<u32>,
    pub new_lineno: Option<u32>,
    /// Line text without the trailing newline (lossy UTF-8 for display only;
    /// staging always re-reads the raw bytes from git).
    pub content: String,
    /// `\ No newline at end of file` follows this line.
    pub no_newline: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hunk {
    pub header: String,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
    Copied,
    TypeChange,
    Untracked,
    Conflicted,
    Unmodified,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub status: FileStatus,
    pub is_binary: bool,
    /// Lines were dropped because the file exceeded the per-file line budget.
    pub truncated: bool,
    pub additions: u32,
    pub deletions: u32,
    pub hunks: Vec<Hunk>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffResult {
    pub target: DiffTarget,
    pub context_lines: u32,
    pub files: Vec<FileDiff>,
}

/// Selection of lines within one hunk. `lines: None` selects the whole hunk.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HunkSelection {
    pub hunk_index: usize,
    /// Header as rendered; used to detect a stale view.
    pub header: String,
    /// Indices into `Hunk::lines`.
    pub lines: Option<Vec<usize>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StageAction {
    Stage,
    Unstage,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StageRequest {
    pub repo_id: Uuid,
    pub path: String,
    pub action: StageAction,
    /// Must equal the `contextLines` the view was rendered with, otherwise hunk
    /// boundaries (and therefore indices) differ.
    pub context_lines: u32,
    /// Empty = whole file.
    pub hunks: Vec<HunkSelection>,
}
