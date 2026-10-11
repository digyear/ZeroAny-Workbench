// Typed CLI failure modes shared by every shell-out in workspace mode
// (`gh`, `glab`, `git`). Each variant maps to a clear toast in the UI;
// the frontend matches on `kind` to render the right next-step hint
// (install URL, login command, etc.) without parsing freeform stderr.
//
// Detection is best-effort substring matching against stderr/stdout —
// the CLIs don't return structured errors, so we look for the same
// signal words their human-readable output uses. False positives are
// rare in practice because each variant's matchers are specific to
// that failure mode.

use serde::Serialize;
use std::process::Output;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CliError {
    /// Binary isn't on PATH. Surface install URL — works on every OS
    /// since we don't try to suggest a per-platform package manager
    /// command (no assumption that the user has brew / apt / choco).
    #[serde(rename_all = "camelCase")]
    NotInstalled { tool: String, install_url: String },
    /// CLI is installed but not authenticated, OR auth'd to a different
    /// account / host than the repo needs. We collapse those into one
    /// variant — the next step is the same: re-login.
    #[serde(rename_all = "camelCase")]
    NotAuthenticated { tool: String, login_cmd: String },
    /// CLI authenticated, but the active account can't access this
    /// repo (404 / 403). User likely needs to switch accounts or get
    /// access granted.
    #[serde(rename_all = "camelCase")]
    NoAccess { tool: String, repo: String },
    /// Network / DNS / connectivity failure. Transient — retry.
    #[serde(rename_all = "camelCase")]
    NetworkError { msg: String },
    /// `git commit` with nothing staged. Not really an error — the
    /// caller usually surfaces this as "no changes to ship".
    NoChanges,
    /// `git push` failed because the branch has no upstream. The
    /// caller normally sets `-u origin <branch>` to recover, so this
    /// usually only fires when a manual push is attempted.
    BranchNotPushed,
    /// No API token stored for this git host. With a token the workspace
    /// talks to GitHub/GitLab directly; without one it falls back to the
    /// CLI, so this fires when neither is available.
    #[serde(rename_all = "camelCase")]
    TokenMissing { provider: String, host: String },
    /// Stored token was rejected by the host (401 / revoked / expired).
    #[serde(rename_all = "camelCase")]
    TokenRejected { provider: String, host: String },
    /// Catch-all — raw stderr lifted into the toast. Last resort;
    /// every classifiable failure should map to one of the variants
    /// above so we can give a clean next-step hint.
    Other { stderr: String },
}

impl CliError {
    /// Human-readable single-line message for the toast. Frontend may
    /// also render its own copy by matching on the `kind` field.
    pub fn message(&self) -> String {
        match self {
            CliError::NotInstalled { tool, install_url } => format!(
                "{tool} is not installed or not on PATH. Install it from {install_url} and retry."
            ),
            CliError::NotAuthenticated { tool, login_cmd } => format!(
                "{tool} is not authenticated (or signed in to a different account). Run `{login_cmd}` and retry."
            ),
            CliError::NoAccess { tool, repo } => format!(
                "{tool} can't access {repo}. Check that the active account has permissions on this repo, or switch accounts."
            ),
            CliError::NetworkError { msg } => format!(
                "Network error: {msg}. Retry in a moment."
            ),
            CliError::NoChanges => {
                "Nothing to commit — the worktree has no pending changes.".into()
            }
            CliError::BranchNotPushed => {
                "Branch has no upstream remote. Push it first with `git push -u origin <branch>`.".into()
            }
            CliError::TokenMissing { provider, host } => format!(
                "No {provider} access token stored for {host}. Save one in the board's Git account settings to connect without the CLI, then retry."
            ),
            CliError::TokenRejected { provider, host } => format!(
                "{provider} rejected the stored token for {host}. Update it in the board's Git account settings and retry."
            ),
            CliError::Other { stderr } => {
                if stderr.is_empty() {
                    "Command failed with no output. Check the workspace logs.".into()
                } else {
                    stderr.clone()
                }
            }
        }
    }
}

/// Cross-platform PATH check. Delegates to the shared resolver so
/// bundled GUI builds see the same PATH a real terminal would —
/// launchd / desktop launchers strip brew/nvm/asdf entries otherwise.
pub fn is_on_path(bin: &str) -> bool {
    crate::shared::platform::path::is_on_path(bin)
}

/// Canonical install URL for a known CLI. Empty string for unknown
/// tools — the toast then omits the link instead of showing a bogus
/// one. We intentionally don't pick a per-platform package-manager
/// command since the install pages cover every supported OS.
pub fn install_url_for(tool: &str) -> &'static str {
    match tool {
        "gh" => "https://cli.github.com/",
        "glab" => "https://gitlab.com/gitlab-org/cli",
        "git" => "https://git-scm.com/downloads",
        _ => "",
    }
}

/// Canonical login invocation for a known CLI. Used in
/// NotAuthenticated toast copy so the user can copy-paste the exact
/// command instead of guessing.
pub fn login_cmd_for(tool: &str) -> String {
    match tool {
        "gh" => "gh auth login".into(),
        "glab" => "glab auth login".into(),
        _ => format!("{tool} auth login"),
    }
}

/// Classify a failed `Command::output()` into a typed CliError.
/// Returns `None` when the command succeeded — caller short-circuits
/// happy paths. `repo_hint` shows up in NoAccess errors when known
/// (e.g. "owner/repo"); pass `None` for git operations that aren't
/// repo-scoped.
pub fn classify_output(tool: &str, output: &Output, repo_hint: Option<&str>) -> Option<CliError> {
    if output.status.success() {
        return None;
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let combined = format!("{stderr}\n{stdout}").to_lowercase();

    // Order matters — auth errors often include "404"-ish wording
    // ("repository not found" can be GitHub's response to an
    // unauthenticated request for a private repo), so check auth first.
    if combined.contains("could not find auth")
        || combined.contains("not logged in")
        || combined.contains("not authenticated")
        || combined.contains("authentication required")
        || combined.contains("authentication failed")
        || combined.contains("invalid token")
        || combined.contains("bad credentials")
        || combined.contains("unauthorized")
        || combined.contains("auth token")
        || combined.contains("gh auth login")
        || combined.contains("glab auth login")
    {
        return Some(CliError::NotAuthenticated {
            tool: tool.to_string(),
            login_cmd: login_cmd_for(tool),
        });
    }

    if combined.contains("could not resolve host")
        || combined.contains("network is unreachable")
        || combined.contains("connection refused")
        || combined.contains("connection timed out")
        || combined.contains("dns lookup failed")
        || (combined.contains("timeout") && !combined.contains("commit"))
    {
        return Some(CliError::NetworkError {
            msg: stderr.lines().next().unwrap_or("connection failed").to_string(),
        });
    }

    if combined.contains("403")
        || combined.contains("forbidden")
        || combined.contains("permission denied")
        || combined.contains("access denied")
        || combined.contains("not allowed")
        || combined.contains("404")
        || combined.contains("repository not found")
        || combined.contains("does not exist or you do not have")
        // GitHub's GraphQL response when the active account can't see
        // a repo (private + no access OR doesn't exist). Looks like:
        //   GraphQL: Could not resolve to a Repository with the name
        //   'org/repo'. (repository)
        // Treated as NoAccess because in practice the repo IS there
        // and the user just needs to switch accounts.
        || combined.contains("could not resolve to a repository")
        || combined.contains("could not resolve to a node")
    {
        return Some(CliError::NoAccess {
            tool: tool.to_string(),
            repo: repo_hint.unwrap_or("this repo").to_string(),
        });
    }

    if combined.contains("nothing to commit")
        || combined.contains("no changes added")
        || combined.contains("nothing added to commit")
    {
        return Some(CliError::NoChanges);
    }

    if combined.contains("has no upstream branch")
        || combined.contains("no remote tracking")
        || combined.contains("the current branch") && combined.contains("has no upstream")
    {
        return Some(CliError::BranchNotPushed);
    }

    Some(CliError::Other { stderr })
}
