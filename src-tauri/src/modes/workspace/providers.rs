// Git-host REST client for workspace mode — GitHub and GitLab over their
// HTTP APIs instead of shelling out to `gh` / `glab`.
//
// Boards, ticket comments and PRs used to require a locally installed (and
// already authenticated) CLI. Every host is now reachable with a personal
// access token stored in the OS credential store, keyed per host, so
// self-hosted GitLab / GitHub Enterprise installs work the same way as the
// public ones and no CLI is needed at all.
//
// The token is opt-in: callers ask `stored_token(host)` first and keep their
// existing CLI path as the fallback, so nothing breaks for users who already
// live in `gh`/`glab`.
//
// Endpoint coverage mirrors what the CLI paths did:
//   * issues: list / create
//   * issue comments (GitLab: notes): list / create / update / delete
//   * PRs / MRs: create, read state
//   * identity: who the token belongs to (shown in the UI)

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::shared::platform::credential_store::{credential_store, CredentialStore};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    GitHub,
    GitLab,
}

impl Provider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Provider::GitHub => "github",
            Provider::GitLab => "gitlab",
        }
    }

    /// CLI that used to be required for this provider (fallback path).
    pub fn cli(&self) -> &'static str {
        match self {
            Provider::GitHub => "gh",
            Provider::GitLab => "glab",
        }
    }

    pub fn install_url(&self) -> &'static str {
        match self {
            Provider::GitHub => "https://cli.github.com/",
            Provider::GitLab => "https://gitlab.com/gitlab-org/cli",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Provider::GitHub => "GitHub",
            Provider::GitLab => "GitLab",
        }
    }
}

/// A repository addressed by host + path, parsed from any remote-URL form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRepo {
    pub provider: Provider,
    /// Lowercased host, e.g. `github.com`, `gitlab.example.com`.
    pub host: String,
    /// `owner/repo`, or `group/subgroup/project` on GitLab.
    pub owner_repo: String,
}

impl GitRepo {
    pub fn web_base(&self) -> String {
        format!("https://{}", self.host)
    }
}

/// One request target: provider + API base + project path. `base` is
/// injectable so tests can point ops at a local mock server.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub provider: Provider,
    pub base: String,
    pub host: String,
    pub owner_repo: String,
}

impl Endpoints {
    pub fn for_repo(repo: &GitRepo) -> Self {
        Self {
            provider: repo.provider,
            base: api_base(repo),
            host: repo.host.clone(),
            owner_repo: repo.owner_repo.clone(),
        }
    }

    pub fn new(base: impl Into<String>, provider: Provider, host: &str, owner_repo: &str) -> Self {
        Self {
            provider,
            base: base.into(),
            host: host.to_string(),
            owner_repo: owner_repo.to_string(),
        }
    }

    /// GitLab identifies a project by its URL-encoded path; GitHub by
    /// `owner/repo` verbatim.
    fn project(&self) -> String {
        match self.provider {
            Provider::GitHub => self.owner_repo.clone(),
            Provider::GitLab => self.owner_repo.replace('/', "%2F"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteIssue {
    pub number: String,
    pub title: String,
    pub body: String,
    pub url: String,
    pub labels: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteComment {
    /// Provider-side id of the comment/note, as a string.
    pub rest_id: String,
    pub author: String,
    pub body: String,
    pub created_at: String,
    /// Stable per-comment URL (GitHub). GitLab notes have no per-note URL,
    /// so callers key those by `note_<id>`.
    pub url: Option<String>,
}

// ---------------------------------------------------------------------------
// URL parsing
// ---------------------------------------------------------------------------

/// Parse any git remote / web URL into (provider, host, project path).
///
/// Handles `https://`, `http://`, `ssh://`, `git://` and `git@host:path`
/// forms, a trailing `.git`, and URLs pointing at an issue / PR / MR deep
/// link (everything after the project path is dropped).
pub fn parse_repo(url: &str) -> Option<GitRepo> {
    let raw = url.trim();
    if raw.is_empty() {
        return None;
    }

    let (host, path) = if let Some(rest) = raw.strip_prefix("git@") {
        let (host, path) = rest.split_once(':')?;
        (host, path)
    } else {
        let without_scheme = raw
            .strip_prefix("https://")
            .or_else(|| raw.strip_prefix("http://"))
            .or_else(|| raw.strip_prefix("ssh://"))
            .or_else(|| raw.strip_prefix("git://"))
            .unwrap_or(raw);
        // A leading `user@` belongs to the credential part, not the path.
        let without_user = match without_scheme.split_once('/') {
            Some((head, _)) if head.contains('@') => {
                without_scheme.split_once('@').map(|(_, r)| r)?
            }
            _ => without_scheme,
        };
        let (host, path) = without_user.split_once('/')?;
        (host, path)
    };

    let host = host.trim().trim_end_matches('/').to_lowercase();
    if host.is_empty() {
        return None;
    }

    // Drop query/fragment, then keep the project path segments that come
    // before any issue / PR / MR sub-resource.
    let path = path
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/').filter(|s| !s.is_empty()) {
        if matches!(
            segment,
            "issues" | "pulls" | "merge_requests" | "pull" | "commit" | "tree" | "-"
        ) {
            break;
        }
        segments.push(segment);
    }
    if segments.is_empty() {
        return None;
    }

    let provider = provider_for(&host, raw)?;
    // GitHub is always owner/repo; GitLab allows nested groups.
    let owner_repo = match provider {
        Provider::GitHub => {
            if segments.len() < 2 {
                return None;
            }
            segments[..2].join("/")
        }
        Provider::GitLab => {
            if segments.len() < 2 {
                return None;
            }
            segments.join("/")
        }
    };

    Some(GitRepo {
        provider,
        host,
        owner_repo,
    })
}

fn provider_for(host: &str, raw: &str) -> Option<Provider> {
    if host.contains("github") {
        return Some(Provider::GitHub);
    }
    if host.contains("gitlab") {
        return Some(Provider::GitLab);
    }
    // Self-hosted instances that don't carry the product name in the host
    // (e.g. code.company.com) — fall back to any hint in the raw URL.
    let lowered = raw.to_lowercase();
    if lowered.contains("/github") || lowered.contains("github.") {
        return Some(Provider::GitHub);
    }
    if lowered.contains("gitlab") {
        return Some(Provider::GitLab);
    }
    None
}

/// API root for a host: public SaaS keeps its dedicated API host,
/// everything else uses the instance's own API path.
pub fn api_base(repo: &GitRepo) -> String {
    match (repo.provider, repo.host.as_str()) {
        (Provider::GitHub, "github.com") => "https://api.github.com".to_string(),
        (Provider::GitHub, host) => format!("https://{host}/api/v3"),
        (Provider::GitLab, "gitlab.com") => "https://gitlab.com/api/v4".to_string(),
        (Provider::GitLab, host) => format!("https://{host}/api/v4"),
    }
}

// ---------------------------------------------------------------------------
// Tokens (per host, in the OS credential store)
// ---------------------------------------------------------------------------

fn token_key(host: &str) -> String {
    format!("git:{host}")
}

pub async fn stored_token(host: &str) -> Option<String> {
    let store = credential_store();
    store
        .get(&token_key(host))
        .await
        .ok()
        .flatten()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

pub async fn store_token(host: &str, token: &str) -> Result<(), String> {
    let token = token.trim();
    if token.is_empty() {
        return Err("Token is empty".to_string());
    }
    credential_store()
        .store(&token_key(host), token)
        .await
}

pub async fn clear_token(host: &str) -> Result<(), String> {
    credential_store().delete(&token_key(host)).await
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiErrorKind {
    /// No token stored for this host.
    NoToken,
    /// Token rejected by the host (401 / bad credentials).
    Rejected,
    /// Authenticated but not allowed to see this project (403/404).
    NoAccess,
    /// Transport-level failure.
    Network,
    Other,
}

#[derive(Debug, Clone)]
pub struct ApiError {
    pub kind: ApiErrorKind,
    pub provider: Provider,
    pub host: String,
    pub message: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message())
    }
}

impl ApiError {
    pub fn new(kind: ApiErrorKind, provider: Provider, host: &str, message: impl Into<String>) -> Self {
        Self {
            kind,
            provider,
            host: host.to_string(),
            message: message.into(),
        }
    }

    pub fn no_token(provider: Provider, host: &str) -> Self {
        Self::new(
            ApiErrorKind::NoToken,
            provider,
            host,
            format!(
                "No {} token stored for {}.",
                provider.label(),
                host
            ),
        )
    }

    pub fn message(&self) -> String {
        match self.kind {
            ApiErrorKind::NoToken => format!(
                "No {} access token for {}. Add one in the board's Git account settings to connect without CLIs.",
                self.provider.label(),
                self.host
            ),
            ApiErrorKind::Rejected => format!(
                "{} rejected the stored token for {}. Check that it is valid and has not expired, then save it again.",
                self.provider.label(),
                self.host
            ),
            ApiErrorKind::NoAccess => format!(
                "The {} token can't access this project on {}. Check the token's scopes or repository access.",
                self.provider.label(),
                self.host
            ),
            ApiErrorKind::Network => format!(
                "Network error talking to {}: {}",
                self.host, self.message
            ),
            ApiErrorKind::Other => self.message.clone(),
        }
    }

    /// Map onto the existing CLI-error taxonomy so toasts and typed UI
    /// states keep working when the CLI path is replaced by the API path.
    pub fn to_cli_error(&self) -> super::cli_errors::CliError {
        use super::cli_errors::CliError;
        match self.kind {
            ApiErrorKind::NoToken => CliError::TokenMissing {
                provider: self.provider.label().to_string(),
                host: self.host.clone(),
            },
            ApiErrorKind::Rejected => CliError::TokenRejected {
                provider: self.provider.label().to_string(),
                host: self.host.clone(),
            },
            ApiErrorKind::Network => CliError::NetworkError {
                msg: self.message.clone(),
            },
            ApiErrorKind::NoAccess => CliError::NoAccess {
                tool: format!("{} token", self.provider.label()),
                repo: self.host.clone(),
            },
            ApiErrorKind::Other => CliError::Other {
                stderr: self.message(),
            },
        }
    }
}

fn classify_status(status: u16, provider: Provider, host: &str, body: &str) -> ApiError {
    let kind = match status {
        401 => ApiErrorKind::Rejected,
        403 => {
            // GitHub returns 403 for both "no scope" and rate limits.
            if body.to_lowercase().contains("rate limit") {
                ApiErrorKind::Other
            } else {
                ApiErrorKind::NoAccess
            }
        }
        404 => ApiErrorKind::NoAccess,
        400 | 422 => ApiErrorKind::Other,
        s if s >= 500 => ApiErrorKind::Network,
        _ => ApiErrorKind::Other,
    };
    let detail = extract_message(body);
    ApiError::new(
        kind,
        provider,
        host,
        if detail.is_empty() {
            format!("HTTP {status}")
        } else {
            format!("HTTP {status}: {detail}")
        },
    )
}

fn extract_message(body: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return body.trim().chars().take(300).collect();
    };
    if let Some(msg) = value.get("message").and_then(|m| m.as_str()) {
        return msg.to_string();
    }
    if let Some(msg) = value.get("error").and_then(|m| m.as_str()) {
        return msg.to_string();
    }
    // GitLab validation errors come back as {"message": {field: [..]}}.
    if let Some(obj) = value.get("message").and_then(|m| m.as_object()) {
        return obj
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<_>>()
            .join("; ");
    }
    String::new()
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

const USER_AGENT: &str = "ZeroAny-Workbench";

fn client() -> Result<reqwest::Client, ApiError> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| ApiError::new(ApiErrorKind::Network, Provider::GitHub, "", e.to_string()))
}

async fn send(
    ep: &Endpoints,
    token: &str,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value, ApiError> {
    let url = format!("{}{}", ep.base, path);
    let client = client().map_err(|mut e| {
        e.provider = ep.provider;
        e.host = ep.host.clone();
        e
    })?;
    let mut request = client.request(method, &url);
    // An empty token means "public data only" — omit the header entirely so
    // GitHub doesn't reject the request as bad credentials.
    let has_token = !token.trim().is_empty();
    match ep.provider {
        Provider::GitHub => {
            request = request
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28");
            if has_token {
                request = request.header("Authorization", format!("Bearer {token}"));
            }
        }
        Provider::GitLab => {
            request = request.header("Accept", "application/json");
            if has_token {
                request = request.header("PRIVATE-TOKEN", token);
            }
        }
    }
    if let Some(value) = body {
        request = request.json(&value);
    }

    let response = request.send().await.map_err(|e| {
        ApiError::new(
            ApiErrorKind::Network,
            ep.provider,
            &ep.host,
            e.to_string(),
        )
    })?;
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err(classify_status(status, ep.provider, &ep.host, &text));
    }
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str::<Value>(&text).map_err(|e| {
        ApiError::new(
            ApiErrorKind::Other,
            ep.provider,
            &ep.host,
            format!("Could not parse response: {e}"),
        )
    })
}

// ---------------------------------------------------------------------------
// Ops
// ---------------------------------------------------------------------------

/// Identity behind the token — surfaced in the UI so users can see which
/// account a host is connected as.
pub async fn current_user(ep: &Endpoints, token: &str) -> Result<String, ApiError> {
    let value = send(ep, token, reqwest::Method::GET, "/user", None).await?;
    let login = match ep.provider {
        Provider::GitHub => value.get("login").and_then(|v| v.as_str()),
        Provider::GitLab => value.get("username").and_then(|v| v.as_str()),
    };
    Ok(login.unwrap_or("unknown").to_string())
}

pub async fn default_branch(ep: &Endpoints, token: &str) -> Result<String, ApiError> {
    let path = match ep.provider {
        Provider::GitHub => format!("/repos/{}", ep.project()),
        Provider::GitLab => format!("/projects/{}", ep.project()),
    };
    let value = send(ep, token, reqwest::Method::GET, &path, None).await?;
    Ok(value
        .get("default_branch")
        .and_then(|v| v.as_str())
        .unwrap_or("main")
        .to_string())
}

pub async fn list_issues(ep: &Endpoints, token: &str) -> Result<Vec<RemoteIssue>, ApiError> {
    let path = match ep.provider {
        Provider::GitHub => format!(
            "/repos/{}/issues?state=open&per_page=100&sort=created&direction=desc",
            ep.project()
        ),
        Provider::GitLab => format!(
            "/projects/{}/issues?state=opened&per_page=100&order_by=created_at",
            ep.project()
        ),
    };
    let value = send(ep, token, reqwest::Method::GET, &path, None).await?;
    let items = value.as_array().cloned().unwrap_or_default();
    Ok(items
        .iter()
        .filter_map(|item| issue_from_value(ep.provider, item))
        .collect())
}

fn issue_from_value(provider: Provider, item: &Value) -> Option<RemoteIssue> {
    match provider {
        Provider::GitHub => {
            // The issues endpoint also returns pull requests; `gh issue list`
            // never shows those, so filter them out here.
            if item.get("pull_request").is_some() {
                return None;
            }
            let number = item.get("number")?.as_i64()?;
            Some(RemoteIssue {
                number: number.to_string(),
                title: item.get("title")?.as_str()?.to_string(),
                body: item
                    .get("body")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                url: item
                    .get("html_url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                labels: labels_from_value(item.get("labels")),
                created_at: item
                    .get("created_at")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            })
        }
        Provider::GitLab => {
            let iid = item.get("iid")?.as_i64()?;
            Some(RemoteIssue {
                number: iid.to_string(),
                title: item.get("title")?.as_str()?.to_string(),
                body: item
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                url: item
                    .get("web_url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                labels: labels_from_value(item.get("labels")),
                created_at: item
                    .get("created_at")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            })
        }
    }
}

fn labels_from_value(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|label| match label {
                    // GitHub returns objects, GitLab returns plain strings.
                    Value::String(s) => Some(s.clone()),
                    other => other
                        .get("name")
                        .and_then(|n| n.as_str())
                        .map(|s| s.to_string()),
                })
                .collect()
        })
        .unwrap_or_default()
}

pub async fn create_issue(
    ep: &Endpoints,
    token: &str,
    title: &str,
    body: &str,
    labels: &[String],
) -> Result<RemoteIssue, ApiError> {
    let (path, payload) = match ep.provider {
        Provider::GitHub => (
            format!("/repos/{}/issues", ep.project()),
            json!({ "title": title, "body": body, "labels": labels }),
        ),
        Provider::GitLab => (
            format!("/projects/{}/issues", ep.project()),
            json!({ "title": title, "description": body, "labels": labels.join(",") }),
        ),
    };
    let value = send(ep, token, reqwest::Method::POST, &path, Some(payload)).await?;
    issue_from_value(ep.provider, &value).ok_or_else(|| {
        ApiError::new(
            ApiErrorKind::Other,
            ep.provider,
            &ep.host,
            "Host did not return the created issue",
        )
    })
}

pub async fn list_issue_comments(
    ep: &Endpoints,
    token: &str,
    issue_number: &str,
) -> Result<Vec<RemoteComment>, ApiError> {
    let path = match ep.provider {
        Provider::GitHub => format!(
            "/repos/{}/issues/{}/comments?per_page=100",
            ep.project(),
            issue_number
        ),
        Provider::GitLab => format!(
            "/projects/{}/issues/{}/notes?per_page=100&sort=asc",
            ep.project(),
            issue_number
        ),
    };
    let value = send(ep, token, reqwest::Method::GET, &path, None).await?;
    let items = value.as_array().cloned().unwrap_or_default();
    Ok(items
        .iter()
        .filter_map(|item| comment_from_value(ep.provider, item))
        .collect())
}

fn comment_from_value(provider: Provider, item: &Value) -> Option<RemoteComment> {
    match provider {
        Provider::GitHub => Some(RemoteComment {
            rest_id: item.get("id")?.as_i64()?.to_string(),
            author: item
                .get("user")
                .and_then(|u| u.get("login"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string(),
            body: item
                .get("body")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            created_at: item
                .get("created_at")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            url: item
                .get("html_url")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        }),
        Provider::GitLab => {
            // System notes (label/state changes) are not real comments.
            if item
                .get("system")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                return None;
            }
            Some(RemoteComment {
                rest_id: item.get("id")?.as_i64()?.to_string(),
                author: item
                    .get("author")
                    .and_then(|a| a.get("username"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                body: item
                    .get("body")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                created_at: item
                    .get("created_at")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                url: None,
            })
        }
    }
}

pub async fn post_issue_comment(
    ep: &Endpoints,
    token: &str,
    issue_number: &str,
    body: &str,
) -> Result<RemoteComment, ApiError> {
    let (path, payload) = match ep.provider {
        Provider::GitHub => (
            format!("/repos/{}/issues/{}/comments", ep.project(), issue_number),
            json!({ "body": body }),
        ),
        Provider::GitLab => (
            format!("/projects/{}/issues/{}/notes", ep.project(), issue_number),
            json!({ "body": body }),
        ),
    };
    // GitHub echoes the created comment (with html_url); GitLab answers 201
    // with the note body only.
    let value = send(ep, token, reqwest::Method::POST, &path, Some(payload)).await?;
    if value.is_object() {
        if let Some(comment) = comment_from_value(ep.provider, &value) {
            return Ok(comment);
        }
    }
    Ok(RemoteComment {
        rest_id: String::new(),
        author: String::new(),
        body: body.to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        url: None,
    })
}

pub async fn update_issue_comment(
    ep: &Endpoints,
    token: &str,
    issue_number: &str,
    comment_id: &str,
    body: &str,
) -> Result<(), ApiError> {
    let (path, method) = match ep.provider {
        Provider::GitHub => (
            format!("/repos/{}/issues/comments/{}", ep.project(), comment_id),
            reqwest::Method::PATCH,
        ),
        Provider::GitLab => (
            format!(
                "/projects/{}/issues/{}/notes/{}",
                ep.project(),
                issue_number,
                comment_id
            ),
            reqwest::Method::PUT,
        ),
    };
    let payload = match ep.provider {
        Provider::GitHub => json!({ "body": body }),
        Provider::GitLab => json!({ "body": body }),
    };
    send(ep, token, method, &path, Some(payload)).await?;
    Ok(())
}

pub async fn delete_issue_comment(
    ep: &Endpoints,
    token: &str,
    issue_number: &str,
    comment_id: &str,
) -> Result<(), ApiError> {
    let path = match ep.provider {
        Provider::GitHub => format!("/repos/{}/issues/comments/{}", ep.project(), comment_id),
        Provider::GitLab => format!(
            "/projects/{}/issues/{}/notes/{}",
            ep.project(),
            issue_number,
            comment_id
        ),
    };
    send(ep, token, reqwest::Method::DELETE, &path, None).await?;
    Ok(())
}

/// Create a PR (GitHub) / MR (GitLab) and return its web URL.
pub async fn create_pull_request(
    ep: &Endpoints,
    token: &str,
    head: &str,
    base: &str,
    title: &str,
    body: &str,
) -> Result<String, ApiError> {
    let (path, payload) = match ep.provider {
        Provider::GitHub => (
            format!("/repos/{}/pulls", ep.project()),
            json!({ "title": title, "head": head, "base": base, "body": body }),
        ),
        Provider::GitLab => (
            format!("/projects/{}/merge_requests", ep.project()),
            json!({
                "title": title,
                "source_branch": head,
                "target_branch": base,
                "description": body,
            }),
        ),
    };
    let value = send(ep, token, reqwest::Method::POST, &path, Some(payload)).await?;
    let url = match ep.provider {
        Provider::GitHub => value.get("html_url").and_then(|v| v.as_str()),
        Provider::GitLab => value.get("web_url").and_then(|v| v.as_str()),
    };
    url.map(|s| s.to_string()).ok_or_else(|| {
        ApiError::new(
            ApiErrorKind::Other,
            ep.provider,
            &ep.host,
            "Host did not return a pull-request URL",
        )
    })
}

/// Normalised PR state: `open` | `merged` | `closed`.
pub async fn pull_request_state(
    ep: &Endpoints,
    token: &str,
    number: &str,
) -> Result<String, ApiError> {
    let path = match ep.provider {
        Provider::GitHub => format!("/repos/{}/pulls/{}", ep.project(), number),
        Provider::GitLab => format!("/projects/{}/merge_requests/{}", ep.project(), number),
    };
    let value = send(ep, token, reqwest::Method::GET, &path, None).await?;
    let state = value
        .get("state")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    let merged = match ep.provider {
        // GitHub: merged_at set → merged. GitLab: state == "merged".
        Provider::GitHub => !value
            .get("merged_at")
            .map(|v| v.is_null())
            .unwrap_or(true),
        Provider::GitLab => state == "merged",
    };
    Ok(if merged {
        "merged".to_string()
    } else if state == "open" || state == "opened" {
        "open".to_string()
    } else if state == "closed" {
        "closed".to_string()
    } else {
        String::new()
    })
}

/// Pull-request / issue number from a web URL (last numeric path segment).
pub fn number_from_url(url: &str) -> Option<String> {
    url.split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_end_matches('/')
        .rsplit('/')
        .find(|segment| !segment.is_empty() && segment.chars().all(|c| c.is_ascii_digit()))
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    fn repo(url: &str) -> GitRepo {
        parse_repo(url).expect("parse failed")
    }

    #[test]
    fn parses_every_remote_url_shape() {
        for url in [
            "https://github.com/digyear/ZeroAny-Workbench.git",
            "https://github.com/digyear/ZeroAny-Workbench",
            "http://github.com/digyear/ZeroAny-Workbench/",
            "git@github.com:digyear/ZeroAny-Workbench.git",
            "ssh://git@github.com/digyear/ZeroAny-Workbench.git",
            "git://github.com/digyear/ZeroAny-Workbench.git",
        ] {
            let r = repo(url);
            assert_eq!(r.provider, Provider::GitHub, "{url}");
            assert_eq!(r.host, "github.com", "{url}");
            assert_eq!(r.owner_repo, "digyear/ZeroAny-Workbench", "{url}");
        }
    }

    #[test]
    fn drops_deep_links_and_queries() {
        let r = repo("https://github.com/digyear/ZeroAny-Workbench/issues/85#issuecomment-1");
        assert_eq!(r.owner_repo, "digyear/ZeroAny-Workbench");
        let r = repo("https://gitlab.com/lute/station/-/merge_requests/42");
        assert_eq!(r.owner_repo, "lute/station");
    }

    #[test]
    fn keeps_gitlab_subgroups() {
        let r = repo("git@gitlab.com:lute/platform/station.git");
        assert_eq!(r.provider, Provider::GitLab);
        assert_eq!(r.owner_repo, "lute/platform/station");
    }

    #[test]
    fn recognises_self_hosted_instances() {
        let gh = repo("https://github.example.com/acme/api.git");
        assert_eq!(gh.provider, Provider::GitHub);
        assert_eq!(api_base(&gh), "https://github.example.com/api/v3");

        let gl = repo("https://gitlab.internal.lute.io/acme/api.git");
        assert_eq!(gl.provider, Provider::GitLab);
        assert_eq!(api_base(&gl), "https://gitlab.internal.lute.io/api/v4");
    }

    #[test]
    fn api_bases_match_each_product() {
        assert_eq!(
            api_base(&repo("https://github.com/a/b")),
            "https://api.github.com"
        );
        assert_eq!(
            api_base(&repo("https://gitlab.com/a/b")),
            "https://gitlab.com/api/v4"
        );
    }

    #[test]
    fn rejects_unparseable_input() {
        assert!(parse_repo("").is_none());
        assert!(parse_repo("not a url").is_none());
        assert!(parse_repo("https://example.com/onlyowner").is_none());
    }

    #[test]
    fn reads_pr_and_issue_numbers_from_urls() {
        assert_eq!(
            number_from_url("https://github.com/a/b/pull/42").as_deref(),
            Some("42")
        );
        assert_eq!(
            number_from_url("https://github.com/a/b/issues/7#issuecomment-9").as_deref(),
            Some("7")
        );
        assert_eq!(number_from_url("https://github.com/a/b").as_deref(), None);
    }

    /// Minimal single-shot HTTP server: answers one request with `status` +
    /// `body`, and hands the raw request text back to the test.
    fn mock_server(status: u16, body: String) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 8192];
                let read = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..read]).to_string();
                let _ = tx.send(request);
                let response = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.as_bytes().len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        (format!("http://{addr}"), rx)
    }

    #[tokio::test]
    async fn github_issue_list_uses_bearer_auth_and_filters_pull_requests() {
        let body = r#"[
            {"number": 7, "title": "Fix clock", "body": "details", "html_url": "https://github.com/a/b/issues/7",
             "labels": [{"name": "bug"}], "created_at": "2026-01-02T03:04:05Z"},
            {"number": 8, "title": "A pull request", "html_url": "https://github.com/a/b/pull/8",
             "pull_request": {"url": "x"}, "labels": [], "created_at": "2026-01-03T00:00:00Z"}
        ]"#;
        let (base, rx) = mock_server(200, body.to_string());
        let ep = Endpoints::new(base, Provider::GitHub, "github.com", "a/b");

        let issues = list_issues(&ep, "test-token").await.unwrap();

        let request = rx.recv().unwrap();
        assert!(request.starts_with("GET /repos/a/b/issues?"), "{request}");
        // Header names arrive lowercased over HTTP/1.1.
        assert!(
            request.to_lowercase().contains("authorization: bearer test-token"),
            "{request}"
        );
        assert_eq!(issues.len(), 1, "pull requests must be filtered out");
        assert_eq!(issues[0].number, "7");
        assert_eq!(issues[0].labels, vec!["bug".to_string()]);
        assert_eq!(issues[0].created_at, "2026-01-02T03:04:05Z");
    }

    #[tokio::test]
    async fn gitlab_notes_use_private_token_and_skip_system_notes() {
        let body = r#"[
            {"id": 1, "body": "real comment", "created_at": "2026-02-01T10:00:00Z",
             "author": {"username": "lute"}, "system": false},
            {"id": 2, "body": "changed label", "created_at": "2026-02-01T10:01:00Z",
             "author": {"username": "lute"}, "system": true}
        ]"#;
        let (base, rx) = mock_server(200, body.to_string());
        let ep = Endpoints::new(base, Provider::GitLab, "gitlab.com", "lute/station");

        let comments = list_issue_comments(&ep, "glpat-test", "12").await.unwrap();

        let request = rx.recv().unwrap();
        assert!(request.starts_with("GET /projects/lute%2Fstation/issues/12/notes?"), "{request}");
        assert!(
            request.to_lowercase().contains("private-token: glpat-test"),
            "{request}"
        );
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].author, "lute");
        assert_eq!(comments[0].rest_id, "1");
    }

    #[tokio::test]
    async fn creates_a_github_pull_request_and_returns_its_url() {
        let body = r#"{"html_url": "https://github.com/a/b/pull/42", "number": 42}"#;
        let (base, rx) = mock_server(201, body.to_string());
        let ep = Endpoints::new(base, Provider::GitHub, "github.com", "a/b");

        let url = create_pull_request(&ep, "t", "feature/x", "dev", "Title", "Body")
            .await
            .unwrap();

        let request = rx.recv().unwrap();
        assert!(request.starts_with("POST /repos/a/b/pulls"), "{request}");
        assert!(request.contains("\"head\":\"feature/x\""), "{request}");
        assert!(request.contains("\"base\":\"dev\""), "{request}");
        assert_eq!(url, "https://github.com/a/b/pull/42");
    }

    #[tokio::test]
    async fn maps_host_failures_onto_typed_errors() {
        let (base, _rx) = mock_server(401, r#"{"message":"Bad credentials"}"#.to_string());
        let ep = Endpoints::new(base, Provider::GitHub, "github.com", "a/b");
        let err = list_issues(&ep, "bad").await.unwrap_err();
        assert_eq!(err.kind, ApiErrorKind::Rejected);

        let (base, _rx) = mock_server(404, r#"{"message":"Not Found"}"#.to_string());
        let ep = Endpoints::new(base, Provider::GitHub, "github.com", "a/b");
        let err = current_user(&ep, "t").await.unwrap_err();
        assert_eq!(err.kind, ApiErrorKind::NoAccess);

        let err = ApiError::no_token(Provider::GitLab, "gitlab.com");
        assert!(matches!(
            err.to_cli_error(),
            crate::modes::workspace::cli_errors::CliError::TokenMissing { .. }
        ));
    }

    /// Real network check against the public GitHub API. No token needed for
    /// a public repository, so this validates URL building, headers and
    /// parsing end-to-end. Run with `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn live_github_public_repository_listing() {
        let repo = repo("https://github.com/rust-lang/rust");
        let ep = Endpoints::for_repo(&repo);
        assert_eq!(ep.base, "https://api.github.com");
        // Unauthenticated request: the endpoint is public, the token is only
        // used for the header format.
        let issues = list_issues(&ep, "").await.expect("live call failed");
        assert!(!issues.is_empty(), "expected open issues on rust-lang/rust");
        assert!(issues.iter().all(|i| !i.title.is_empty()));
        assert!(issues.iter().all(|i| i.url.contains("/issues/")));
    }
}