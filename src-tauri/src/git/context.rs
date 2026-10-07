//! Per-account git context isolation.
//!
//! A [`GitContext`] is built for every operation that touches the network or
//! writes commits. It guarantees that, for a repository bound to account A:
//!
//! * libgit2 transports only ever receive A's SSH key / token, and the token is
//!   only offered to A's own host (never to a mirror or submodule elsewhere);
//! * spawned `git` processes get A's key via `GIT_SSH_COMMAND`, A's token via a
//!   host-scoped `http.<origin>.extraHeader`, A's author/committer identity, and
//!   have system credential helpers disabled so a cached token for account B
//!   on the same host can never be picked up;
//! * nothing is written to the user's global `~/.gitconfig`. Per-repository
//!   binding (`bind_repository`) is opt-in and only touches `.git/config`.
//!
//! Isolation is enforced per call, not via process-global state, so two
//! workspaces on different accounts can run operations concurrently.

use std::cell::Cell;
use std::path::Path;
use std::sync::Mutex;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use git2::{Cred, CredentialType, FetchOptions, RemoteCallbacks, Repository, Signature};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::error::{AppError, AppResult};
use crate::models::{Account, GitHostType, GitIdentity, SshSettings};
use crate::paths::shell_quote_for_git;
use crate::secrets::{ssh_passphrase_key, token_key, Secret, SecretVault};

/// libgit2 re-invokes the credential callback after every rejected attempt.
/// Without a cap, a wrong key loops forever.
const MAX_AUTH_ATTEMPTS: usize = 3;

/// Why the credential callback could not authenticate. Recorded so the error
/// shown to the user names the account and the fix, instead of libgit2's
/// generic "authentication failed".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthProblem {
    MissingToken {
        url: String,
    },
    VaultLocked {
        url: String,
    },
    NoSshKey {
        url: String,
    },
    TokenHostMismatch {
        url: String,
    },
    /// Credentials were offered and the server kept rejecting them.
    Rejected {
        url: String,
    },
}

pub struct GitContext {
    pub account_id: Option<Uuid>,
    /// Account label for error messages.
    pub account_label: Option<String>,
    pub host: GitHostType,
    /// Hostname the token may be sent to.
    pub token_host: Option<String>,
    pub identity: Option<GitIdentity>,
    pub ssh: SshSettings,
    token: Option<Secret>,
    ssh_passphrase: Option<Secret>,
    /// Secrets could not be read because the fallback vault is locked.
    secrets_locked: bool,
    /// Set by the credential callback; read by [`GitContext::explain`].
    auth_problem: Mutex<Option<AuthProblem>>,
}

impl GitContext {
    /// Context for repositories not bound to any account: the user's own git
    /// configuration and ssh-agent apply.
    pub fn anonymous() -> Self {
        Self {
            account_id: None,
            account_label: None,
            host: GitHostType::Local,
            token_host: None,
            identity: None,
            ssh: SshSettings {
                private_key_path: None,
                use_agent: true,
            },
            token: None,
            ssh_passphrase: None,
            secrets_locked: false,
            auth_problem: Mutex::new(None),
        }
    }

    pub fn for_account(account: Option<&Account>, vault: &SecretVault) -> AppResult<Self> {
        let Some(account) = account else {
            return Ok(Self::anonymous());
        };

        let mut secrets_locked = false;
        let mut read = |key: String| match vault.get(&key) {
            Ok(v) => Ok(v),
            // A locked vault is not fatal: SSH-agent / unencrypted keys may
            // still work. We remember it to produce a precise error later.
            Err(AppError::VaultLocked) => {
                secrets_locked = true;
                Ok(None)
            }
            Err(e) => Err(e),
        };
        let token = read(token_key(account.id))?;
        let ssh_passphrase = read(ssh_passphrase_key(account.id))?;

        Ok(Self {
            account_id: Some(account.id),
            account_label: Some(account.label.clone()),
            host: account.host,
            token_host: account.remote_host(),
            identity: Some(account.identity.clone()),
            ssh: account.ssh.clone(),
            token,
            ssh_passphrase,
            secrets_locked,
            auth_problem: Mutex::new(None),
        })
    }

    /// Commit signature: the account identity, or the repository's configured
    /// identity for unbound repositories.
    pub fn signature(&self, repo: &Repository) -> AppResult<Signature<'static>> {
        match &self.identity {
            Some(id) => Ok(Signature::now(&id.name, &id.email)?),
            None => Ok(repo.signature()?.to_owned()),
        }
    }

    fn token_allowed_for(&self, url: &str) -> bool {
        let (Some(expected), Ok(parsed)) = (&self.token_host, url::Url::parse(url)) else {
            return false;
        };
        parsed.scheme() == "https"
            && parsed
                .host_str()
                .is_some_and(|h| h.eq_ignore_ascii_case(expected))
    }

    /// libgit2 callbacks carrying this account's credentials.
    pub fn remote_callbacks(&self) -> RemoteCallbacks<'_> {
        let mut callbacks = RemoteCallbacks::new();
        let attempts = Cell::new(0usize);

        callbacks.credentials(move |url, username_from_url, allowed| {
            let fail = |problem: AuthProblem| {
                let msg = self.describe(&problem);
                if let Ok(mut slot) = self.auth_problem.lock() {
                    *slot = Some(problem);
                }
                Err(git2::Error::from_str(&msg))
            };

            let n = attempts.get() + 1;
            attempts.set(n);
            if n > MAX_AUTH_ATTEMPTS {
                return fail(AuthProblem::Rejected {
                    url: url.to_owned(),
                });
            }

            if allowed.contains(CredentialType::SSH_KEY) {
                let user = username_from_url.unwrap_or("git");
                if let Some(key) = &self.ssh.private_key_path {
                    return Cred::ssh_key(
                        user,
                        None,
                        key,
                        self.ssh_passphrase.as_ref().map(|p| p.as_str()),
                    );
                }
                if self.ssh.use_agent {
                    return Cred::ssh_key_from_agent(user);
                }
                return fail(AuthProblem::NoSshKey {
                    url: url.to_owned(),
                });
            }

            if allowed.contains(CredentialType::USER_PASS_PLAINTEXT) {
                if !self.token_allowed_for(url) {
                    return fail(AuthProblem::TokenHostMismatch {
                        url: url.to_owned(),
                    });
                }
                if let Some(token) = &self.token {
                    return Cred::userpass_plaintext(
                        self.host.https_token_username(),
                        token.as_str(),
                    );
                }
                return fail(if self.secrets_locked {
                    AuthProblem::VaultLocked {
                        url: url.to_owned(),
                    }
                } else {
                    AuthProblem::MissingToken {
                        url: url.to_owned(),
                    }
                });
            }

            if allowed.contains(CredentialType::USERNAME) {
                return Cred::username(username_from_url.unwrap_or("git"));
            }

            Err(git2::Error::from_str("unsupported authentication method"))
        });

        callbacks
    }

    fn label(&self) -> String {
        match &self.account_label {
            Some(l) => format!("the account \u{201c}{l}\u{201d}"),
            None => "this workspace (no account)".into(),
        }
    }

    /// Actionable, user-facing explanation of an authentication problem.
    pub fn describe(&self, problem: &AuthProblem) -> String {
        let who = self.label();
        match problem {
            AuthProblem::MissingToken { url } => format!(
                "{url} requires authentication (private repository?), but {who} has no access token. \
                 Add a token in the account settings (Edit account), or clone over SSH."
            ),
            AuthProblem::VaultLocked { url } => format!(
                "{url} requires authentication, but the secret vault is locked, so the token of {who} \
                 cannot be read. Unlock the vault and try again."
            ),
            AuthProblem::NoSshKey { url } => format!(
                "{url} requires an SSH key, but {who} has no SSH key configured and ssh-agent use is off. \
                 Set a key in the account settings (Edit account)."
            ),
            AuthProblem::TokenHostMismatch { url } => format!(
                "{url} is not on the host of {who}{}, so its token is not sent there. \
                 Use a workspace bound to an account on that host, or a public URL.",
                self.token_host.as_deref().map(|h| format!(" ({h})")).unwrap_or_default()
            ),
            AuthProblem::Rejected { url } => format!(
                "{url} rejected the credentials of {who}. Check that the access token is valid and has \
                 the `repo` (GitHub) or `read_repository` (GitLab) scope, or that the SSH key is \
                 registered with the host."
            ),
        }
    }

    /// Converts a libgit2 error from an operation that used this context's
    /// callbacks into an [`AppError`], turning authentication failures into
    /// [`AppError::AuthRequired`] with an explanation.
    pub fn explain(&self, e: git2::Error) -> AppError {
        let recorded = self.auth_problem.lock().ok().and_then(|mut p| p.take());
        if let Some(problem) = recorded {
            return AppError::AuthRequired(self.describe(&problem));
        }
        if e.code() == git2::ErrorCode::Auth {
            return AppError::AuthRequired(format!(
                "authentication failed for {}: {}",
                self.label(),
                e.message()
            ));
        }
        AppError::Git(e)
    }

    pub fn fetch_options(&self) -> FetchOptions<'_> {
        let mut opts = FetchOptions::new();
        opts.remote_callbacks(self.remote_callbacks());
        opts.prune(git2::FetchPrune::On);
        opts.download_tags(git2::AutotagOption::Auto);
        // Honor http.proxy / HTTPS_PROXY like the git CLI does.
        let mut proxy = git2::ProxyOptions::new();
        proxy.auto();
        opts.proxy_options(proxy);
        opts
    }

    /// `ssh` command line pinned to this account's key, or `None` to let the
    /// user's ssh/agent decide.
    pub fn ssh_command(&self) -> Option<String> {
        let key = self.ssh.private_key_path.as_ref()?;
        // IdentitiesOnly stops ssh from offering *other* keys from the agent
        // first, which is exactly how the wrong GitHub account gets picked.
        // BatchMode makes a missing passphrase fail instead of hanging.
        Some(format!(
            "ssh -i {} -o IdentitiesOnly=yes -o BatchMode=yes",
            shell_quote_for_git(key)
        ))
    }

    /// Applies isolation to a `git` child process.
    pub fn apply_to_command(&self, cmd: &mut tokio::process::Command) {
        cmd.env("GIT_TERMINAL_PROMPT", "0")
            // Never pop up a GUI askpass / Git Credential Manager window.
            .env("GIT_ASKPASS", "")
            .env("SSH_ASKPASS", "")
            .env("GCM_INTERACTIVE", "never");

        if let Some(ssh) = self.ssh_command() {
            cmd.env("GIT_SSH_COMMAND", ssh)
                .env("GIT_SSH_VARIANT", "ssh");
        }

        if let Some(id) = &self.identity {
            cmd.env("GIT_AUTHOR_NAME", &id.name)
                .env("GIT_AUTHOR_EMAIL", &id.email)
                .env("GIT_COMMITTER_NAME", &id.name)
                .env("GIT_COMMITTER_EMAIL", &id.email);
        }

        // Process-scoped config (git >= 2.31). Env values are only readable by
        // the same user, unlike `-c` arguments which show up in `ps`.
        let mut config: Vec<(String, Zeroizing<String>)> = Vec::new();
        if let Some(id) = &self.identity {
            config.push(("user.name".into(), Zeroizing::new(id.name.clone())));
            config.push(("user.email".into(), Zeroizing::new(id.email.clone())));
        }
        if self.account_id.is_some() {
            // An empty value resets the helper list: no credentials from the
            // OS store (possibly another account on the same host) leak in.
            config.push(("credential.helper".into(), Zeroizing::new(String::new())));
        }
        if let (Some(token), Some(host)) = (&self.token, &self.token_host) {
            let basic = Zeroizing::new(B64.encode(format!(
                "{}:{}",
                self.host.https_token_username(),
                token.as_str()
            )));
            config.push((
                format!("http.https://{host}/.extraHeader"),
                Zeroizing::new(format!("Authorization: Basic {}", basic.as_str())),
            ));
        }
        cmd.env("GIT_CONFIG_COUNT", config.len().to_string());
        for (i, (key, value)) in config.iter().enumerate() {
            cmd.env(format!("GIT_CONFIG_KEY_{i}"), key)
                .env(format!("GIT_CONFIG_VALUE_{i}"), value.as_str());
        }
    }

    /// Persists the identity (and SSH key) into the repository's own
    /// `.git/config`, so terminal `git` in that folder behaves the same as the
    /// app. Opt-in; never touches global config.
    pub fn bind_repository(&self, repo_path: &Path) -> AppResult<()> {
        let repo = Repository::open(repo_path)?;
        let mut cfg = repo.config()?.open_level(git2::ConfigLevel::Local)?;
        if let Some(id) = &self.identity {
            cfg.set_str("user.name", &id.name)?;
            cfg.set_str("user.email", &id.email)?;
        }
        match self.ssh_command() {
            Some(cmd) => cfg.set_str("core.sshCommand", &cmd)?,
            None => match cfg.remove("core.sshCommand") {
                Ok(()) => {}
                Err(e) if e.code() == git2::ErrorCode::NotFound => {}
                Err(e) => return Err(e.into()),
            },
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ctx(host: GitHostType, token_host: &str) -> GitContext {
        GitContext {
            account_id: Some(Uuid::new_v4()),
            account_label: Some("Work".into()),
            host,
            token_host: Some(token_host.into()),
            identity: Some(GitIdentity {
                name: "Work Me".into(),
                email: "me@corp.example".into(),
            }),
            ssh: SshSettings {
                private_key_path: Some(PathBuf::from("/keys/id work")),
                use_agent: false,
            },
            token: Some(Zeroizing::new("tok".into())),
            ssh_passphrase: None,
            secrets_locked: false,
            auth_problem: Mutex::new(None),
        }
    }

    #[test]
    fn token_is_scoped_to_account_host() {
        let c = ctx(GitHostType::GitLabSelfHosted, "git.corp.example");
        assert!(c.token_allowed_for("https://git.corp.example/team/app.git"));
        assert!(!c.token_allowed_for("https://github.com/team/app.git"));
        assert!(!c.token_allowed_for("http://git.corp.example/team/app.git"));
        assert!(!c.token_allowed_for("https://git.corp.example.evil.io/x.git"));
    }

    #[test]
    fn ssh_command_pins_identity() {
        let c = ctx(GitHostType::GitHub, "github.com");
        let cmd = c.ssh_command().expect("key configured");
        assert!(cmd.contains("IdentitiesOnly=yes"));
        assert!(cmd.contains("\"/keys/id work\""));
    }

    #[test]
    fn auth_problems_are_explained_with_the_account() {
        let mut c = ctx(GitHostType::GitHub, "github.com");
        c.token = None;
        let msg = c.describe(&AuthProblem::MissingToken {
            url: "https://github.com/acme/private.git".into(),
        });
        assert!(
            msg.contains("\u{201c}Work\u{201d}") && msg.contains("no access token"),
            "{msg}"
        );

        // A recorded problem turns the generic libgit2 error into AuthRequired.
        *c.auth_problem.lock().expect("lock") = Some(AuthProblem::MissingToken {
            url: "https://github.com/acme/private.git".into(),
        });
        let err = c.explain(git2::Error::from_str("whatever libgit2 says"));
        assert_eq!(err.kind(), "authRequired");
        assert!(
            !err.to_string().starts_with("git error"),
            "no internal prefix: {err}"
        );
        // Consumed: a later unrelated error stays a git error.
        assert_eq!(c.explain(git2::Error::from_str("x")).kind(), "git");
    }
}
