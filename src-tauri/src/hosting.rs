//! Hosting APIs: lists the repositories an account can clone.
//!
//! * GitHub: `GET /user/repos` (owned, collaborator and organization repos).
//! * GitLab (cloud and self-hosted): `GET /projects?membership=true`.
//!
//! The token is sent only to the account's own API base URL. Pagination is
//! followed up to [`MAX_PAGES`] × 100 repositories, most recently active first.

use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::models::{Account, GitHostType};

const PER_PAGE: usize = 100;
const MAX_PAGES: usize = 10;
const TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteRepo {
    /// `owner/name` (GitHub) or `group/subgroup/name` (GitLab).
    pub full_name: String,
    pub name: String,
    /// Owner login or GitLab namespace path; drives the sidebar grouping.
    pub namespace: String,
    pub description: Option<String>,
    pub https_url: String,
    pub ssh_url: String,
    pub web_url: String,
    pub default_branch: Option<String>,
    pub private: bool,
    /// RFC 3339 timestamp of the last push / activity.
    pub updated_at: Option<String>,
}

// --- GitHub wire format -------------------------------------------------------

#[derive(Deserialize)]
struct GhOwner {
    login: String,
}

#[derive(Deserialize)]
struct GhRepo {
    full_name: String,
    name: String,
    owner: GhOwner,
    description: Option<String>,
    clone_url: String,
    ssh_url: String,
    html_url: String,
    default_branch: Option<String>,
    #[serde(default)]
    private: bool,
    pushed_at: Option<String>,
}

impl From<GhRepo> for RemoteRepo {
    fn from(r: GhRepo) -> Self {
        Self {
            full_name: r.full_name,
            name: r.name,
            namespace: r.owner.login,
            description: r.description.filter(|d| !d.is_empty()),
            https_url: r.clone_url,
            ssh_url: r.ssh_url,
            web_url: r.html_url,
            default_branch: r.default_branch,
            private: r.private,
            updated_at: r.pushed_at,
        }
    }
}

// --- GitLab wire format -------------------------------------------------------

#[derive(Deserialize)]
struct GlNamespace {
    full_path: String,
}

#[derive(Deserialize)]
struct GlProject {
    path_with_namespace: String,
    name: String,
    namespace: GlNamespace,
    description: Option<String>,
    http_url_to_repo: String,
    ssh_url_to_repo: String,
    web_url: String,
    default_branch: Option<String>,
    visibility: Option<String>,
    last_activity_at: Option<String>,
}

impl From<GlProject> for RemoteRepo {
    fn from(p: GlProject) -> Self {
        Self {
            full_name: p.path_with_namespace,
            name: p.name,
            namespace: p.namespace.full_path,
            description: p.description.filter(|d| !d.is_empty()),
            https_url: p.http_url_to_repo,
            ssh_url: p.ssh_url_to_repo,
            web_url: p.web_url,
            default_branch: p.default_branch,
            // Missing visibility (older GitLab) is treated as private: safer.
            private: p.visibility.as_deref() != Some("public"),
            updated_at: p.last_activity_at,
        }
    }
}

/// Lists the repositories visible to `account` using its access token.
pub async fn list_repositories(account: &Account, token: &str) -> AppResult<Vec<RemoteRepo>> {
    let base = match account.host {
        GitHostType::Local => None,
        _ => account.effective_api_base(),
    }
    .ok_or_else(|| {
        AppError::Unsupported("local accounts have no hosting API to list repositories from".into())
    })?;
    let base = base.trim_end_matches('/');
    let client = client(token)?;

    let mut out = Vec::new();
    for page in 1..=MAX_PAGES {
        let batch: Vec<RemoteRepo> = match account.host {
            GitHostType::GitHub => {
                let url = format!(
                    "{base}/user/repos?per_page={PER_PAGE}&page={page}&sort=pushed\
                     &affiliation=owner,collaborator,organization_member"
                );
                get_json::<Vec<GhRepo>>(&client, &url, account)
                    .await?
                    .into_iter()
                    .map(RemoteRepo::from)
                    .collect()
            }
            GitHostType::GitLabCloud | GitHostType::GitLabSelfHosted => {
                let url = format!(
                    "{base}/projects?membership=true&archived=false&per_page={PER_PAGE}\
                     &page={page}&order_by=last_activity_at&sort=desc"
                );
                get_json::<Vec<GlProject>>(&client, &url, account)
                    .await?
                    .into_iter()
                    .map(RemoteRepo::from)
                    .collect()
            }
            GitHostType::Local => Vec::new(), // rejected above
        };
        let last_page = batch.len() < PER_PAGE;
        out.extend(batch);
        if last_page {
            break;
        }
    }
    Ok(out)
}

fn client(token: &str) -> AppResult<reqwest::Client> {
    let mut headers = HeaderMap::new();
    let mut auth = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
        AppError::InvalidInput("the access token contains invalid characters".into())
    })?;
    // Keeps the token out of reqwest's debug output / logs.
    auth.set_sensitive(true);
    headers.insert(AUTHORIZATION, auth);
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static(concat!("OctoNode/", env!("CARGO_PKG_VERSION"))),
    );
    headers.insert(
        "X-GitHub-Api-Version",
        HeaderValue::from_static("2022-11-28"),
    );
    reqwest::Client::builder()
        .default_headers(headers)
        .timeout(TIMEOUT)
        // Never follow a redirect to another host with the token attached.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| AppError::Internal(format!("HTTP client: {e}")))
}

async fn get_json<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
    account: &Account,
) -> AppResult<T> {
    let resp = client.get(url).send().await.map_err(|e| {
        AppError::Remote(format!(
            "could not reach {}: {}",
            account.effective_api_base().unwrap_or_default(),
            without_url(&e)
        ))
    })?;
    let status = resp.status();
    if !status.is_success() {
        let hint = match status.as_u16() {
            401 => "the access token was rejected; update it in the account settings",
            403 => "access denied; the token may lack the `repo` (GitHub) or `read_api` (GitLab) scope, or the rate limit was hit",
            404 => "API endpoint not found; check the account's API base URL",
            _ => "unexpected response from the hosting API",
        };
        return Err(AppError::Remote(format!("{status}: {hint}")));
    }
    resp.json::<T>()
        .await
        .map_err(|e| AppError::Remote(format!("unexpected API response: {}", without_url(&e))))
}

/// reqwest errors embed the request URL; strip it (it can carry query data).
fn without_url(e: &reqwest::Error) -> String {
    let mut msg = e.to_string();
    if let Some(url) = e.url() {
        msg = msg.replace(url.as_str(), "<api>");
    }
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_repo() {
        let json = r#"[{"full_name":"acme/api","name":"api","owner":{"login":"acme"},
            "description":"","clone_url":"https://github.com/acme/api.git",
            "ssh_url":"git@github.com:acme/api.git","html_url":"https://github.com/acme/api",
            "default_branch":"main","private":true,"pushed_at":"2026-01-01T00:00:00Z",
            "unknown_field": 1}]"#;
        let repos: Vec<GhRepo> = serde_json::from_str(json).expect("parse");
        let r = RemoteRepo::from(repos.into_iter().next().expect("one repo"));
        assert_eq!(r.namespace, "acme");
        assert_eq!(r.ssh_url, "git@github.com:acme/api.git");
        assert!(r.private);
        assert_eq!(r.description, None, "empty description is dropped");
    }

    #[test]
    fn parses_gitlab_project() {
        let json = r#"[{"path_with_namespace":"platform/infra/tools","name":"tools",
            "namespace":{"full_path":"platform/infra"},"description":"Infra tools",
            "http_url_to_repo":"https://git.corp.io/platform/infra/tools.git",
            "ssh_url_to_repo":"git@git.corp.io:platform/infra/tools.git",
            "web_url":"https://git.corp.io/platform/infra/tools","default_branch":null,
            "visibility":"internal","last_activity_at":"2026-01-01T00:00:00Z"}]"#;
        let projects: Vec<GlProject> = serde_json::from_str(json).expect("parse");
        let r = RemoteRepo::from(projects.into_iter().next().expect("one project"));
        assert_eq!(r.namespace, "platform/infra");
        assert!(r.private, "internal visibility is not public");
        assert_eq!(r.default_branch, None, "empty repositories have no branch");
    }

    /// End-to-end over HTTP against a local mock GitLab: auth header,
    /// pagination (100 + 1 projects), and 401 mapping.
    #[test]
    fn lists_gitlab_projects_with_pagination() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let server = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for stream in listener.incoming().take(3) {
                let mut stream = stream.expect("conn");
                let mut reader = BufReader::new(stream.try_clone().expect("clone"));
                let mut request_line = String::new();
                reader.read_line(&mut request_line).expect("read");
                let mut auth = String::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).expect("header");
                    if line.trim().is_empty() {
                        break;
                    }
                    if line.to_ascii_lowercase().starts_with("authorization:") {
                        auth = line.trim().to_owned();
                    }
                }
                seen.push((request_line.clone(), auth.clone()));
                let project = |i: usize| {
                    format!(
                        r#"{{"path_with_namespace":"g/p{i}","name":"p{i}","namespace":{{"full_path":"g"}},
                        "description":null,"http_url_to_repo":"https://h/g/p{i}.git",
                        "ssh_url_to_repo":"git@h:g/p{i}.git","web_url":"https://h/g/p{i}",
                        "default_branch":"main","visibility":"public","last_activity_at":null}}"#
                    )
                };
                let (status, body) = if !auth.ends_with("Bearer good-token") {
                    ("401 Unauthorized", "{}".to_owned())
                } else if request_line.contains("&page=1&") {
                    (
                        "200 OK",
                        format!("[{}]", (0..100).map(project).collect::<Vec<_>>().join(",")),
                    )
                } else {
                    ("200 OK", format!("[{}]", project(100)))
                };
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .expect("respond");
            }
            seen
        });

        let account = Account {
            id: uuid::Uuid::new_v4(),
            label: "Mock".into(),
            host: GitHostType::GitLabSelfHosted,
            api_base_url: Some(format!("http://127.0.0.1:{port}/api/v4")),
            username: "me".into(),
            avatar_url: None,
            identity: crate::models::GitIdentity {
                name: "Me".into(),
                email: "me@example.com".into(),
            },
            ssh: crate::models::SshSettings::default(),
            color: None,
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let repos = rt
            .block_on(list_repositories(&account, "good-token"))
            .expect("list");
        assert_eq!(repos.len(), 101, "followed pagination to the short page");
        assert_eq!(repos[100].full_name, "g/p100");
        assert!(!repos[0].private);

        let err = rt
            .block_on(list_repositories(&account, "bad-token"))
            .expect_err("401");
        assert_eq!(err.kind(), "remote");
        assert!(err.to_string().contains("401"), "{err}");

        let seen = server.join().expect("server");
        assert!(seen[0]
            .0
            .starts_with("GET /api/v4/projects?membership=true"));
        assert!(seen[0].1.ends_with("Bearer good-token"));
    }
}
