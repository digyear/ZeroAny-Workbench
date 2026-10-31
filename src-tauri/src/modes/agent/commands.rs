use crate::modes::agent::models::{AgentContext, AgentSession};
use crate::modes::agent::worktree::resolve_project_identity;
use crate::shared::cli::{registry::runner_for, runner::CliRunner};
use crate::shared::repos::sessions as sessions_repo;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::path::PathBuf;
use tauri::State;

#[tauri::command]
pub async fn agent_list_sessions(pool: State<'_, SqlitePool>) -> Result<Vec<AgentSession>, String> {
    let mut sessions = sessions_repo::list_sessions(pool.inner())
        .await
        .map_err(|e| e.to_string())?;
    let paths = sessions
        .iter()
        .map(|session| session.project_path.clone())
        .collect::<Vec<_>>();
    let identities = tokio::task::spawn_blocking(move || {
        paths
            .into_iter()
            .map(|path| {
                let identity = resolve_project_identity(&path);
                (path, identity)
            })
            .collect::<HashMap<_, _>>()
    })
    .await
    .map_err(|error| format!("Project identity resolution failed: {error}"))?;

    for session in &mut sessions {
        let Some(identity) = identities.get(&session.project_path) else {
            continue;
        };
        if session.project_root.as_deref() != Some(&identity.project_root)
            || session.project_name != identity.project_name
        {
            sessions_repo::update_session_project_identity(
                pool.inner(),
                &session.id,
                &identity.project_root,
                &identity.project_name,
            )
            .await
            .map_err(|e| e.to_string())?;
            session.project_root = Some(identity.project_root.clone());
            session.project_name = identity.project_name.clone();
        }
    }
    Ok(sessions)
}

/// Optional overrides for `agent_create_session`. Every field is optional:
/// `None` means "use the project's remembered default, then the global
/// default". This is the shape the Quick Bar sends (usually just a prompt)
/// and the shape the Advanced modal sends (everything explicit).
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionOptions {
    pub title: Option<String>,
    pub purpose: Option<String>,
    pub skip_permissions: Option<bool>,
    pub custom_prompt: Option<String>,
    pub git_name: Option<String>,
    pub git_email: Option<String>,
    pub provider: Option<String>,
    pub binary_path: Option<String>,
    pub profile: Option<String>,
    pub base_branch: Option<String>,
    pub branch_name: Option<String>,
    pub worktree_enabled: Option<bool>,
    /// Context set names attached at creation time.
    pub context_names: Option<Vec<String>>,
}

#[tauri::command]
pub async fn agent_create_session(
    pool: State<'_, SqlitePool>,
    project_path: String,
    prompt: Option<String>,
    options: Option<SessionOptions>,
) -> Result<AgentSession, String> {
    let project_path = project_path.trim().to_string();
    let identity_path = project_path.clone();
    let identity = tokio::task::spawn_blocking(move || resolve_project_identity(&identity_path))
        .await
        .map_err(|error| format!("Project identity resolution failed: {error}"))?;

    let requested = options.unwrap_or_default();
    // Remembered defaults: project root (or worktree) → global defaults.
    // A failure here must never block session creation.
    let remembered = crate::modes::agent::preferences::load(pool.inner(), &project_path)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();

    let initial_prompt = prompt
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string);

    // Title: explicit → first line of the prompt → generic.
    let title = requested
        .title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .or_else(|| {
            initial_prompt
                .as_deref()
                .map(|p| crate::modes::agent::commands::title_from_prompt(p))
        })
        .unwrap_or_else(|| "New session".to_string());

    // Purpose: explicit → remembered → Development.
    let purpose = requested
        .purpose
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .or_else(|| remembered.purpose.clone())
        .unwrap_or_else(|| "Development".to_string());

    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let context_prompt = requested
        .custom_prompt
        .clone()
        .unwrap_or_else(|| SessionOptions::purpose_prompt_fallback(&purpose));
    // Default to Claude when nothing was chosen; unknown ids fall back via
    // `runner_for`. The string is persisted as-is so future providers slot in
    // without a column change.
    let provider = requested
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .or_else(|| remembered.provider.clone())
        .unwrap_or_else(|| "claude".to_string());
    let skip = if requested
        .skip_permissions
        .or(remembered.skip_permissions)
        .unwrap_or(false)
    {
        1
    } else {
        0
    };
    let bin = requested
        .binary_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let profile = requested
        .profile
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or(remembered.profile.as_deref());
    if profile.is_some() && provider != "hermes" {
        return Err(format!("Provider '{provider}' does not support profiles"));
    }
    if let Some(selected) = profile {
        let available = runner_for(&provider).profiles()?;
        if !available.iter().any(|candidate| candidate == selected) {
            return Err(format!("Hermes profile '{selected}' does not exist"));
        }
    }
    // Decision D1: worktree isolation is OFF unless the caller asked for it.
    // Remembered preferences are intentionally NOT allowed to flip the global
    // default off→on, otherwise one manual toggle would stick forever.
    let worktree_enabled = requested.worktree_enabled.unwrap_or(false);
    let base_branch = requested
        .base_branch
        .as_deref()
        .or(remembered.base_branch.as_deref());
    sessions_repo::insert_session(
        pool.inner(),
        &id,
        &title,
        &purpose,
        &project_path,
        Some(&identity.project_root),
        &identity.project_name,
        &context_prompt,
        skip,
        requested.git_name.as_deref().or(remembered.git_name.as_deref()),
        requested.git_email.as_deref().or(remembered.git_email.as_deref()),
        &now,
        &now,
        &provider,
        bin,
        profile,
        if identity.is_linked_worktree { None } else { base_branch },
        if identity.is_linked_worktree { None } else { requested.branch_name.as_deref() },
        if worktree_enabled && !identity.is_linked_worktree { 1 } else { 0 },
    )
    .await
    .map_err(|e| e.to_string())?;

    if let Some(text) = initial_prompt.as_deref() {
        sessions_repo::update_session_initial_prompt(pool.inner(), &id, Some(text))
            .await
            .map_err(|e| e.to_string())?;
    }

    // Attach remembered/explicit context sets by name. Unknown names are
    // skipped — a context deleted on another machine must not block creation.
    if let Some(names) = requested
        .context_names
        .clone()
        .or_else(|| remembered.context_names.clone())
    {
        if !names.is_empty() {
            if let Ok(contexts) = sessions_repo::list_contexts(pool.inner()).await {
                for name in names {
                    if let Some(ctx) = contexts.iter().find(|c| c.name == name) {
                        let _ =
                            sessions_repo::attach_context_to_session(pool.inner(), &id, &ctx.id)
                                .await;
                    }
                }
            }
        }
    }

    // Persist what the caller explicitly chose (never derived defaults).
    let explicit = crate::modes::agent::preferences::ProjectPreferences {
        provider: requested.provider.clone(),
        profile: requested.profile.clone(),
        purpose: requested.purpose.clone(),
        base_branch: requested.base_branch.clone(),
        worktree_enabled: requested.worktree_enabled,
        context_names: requested.context_names.clone(),
        git_name: requested.git_name.clone(),
        git_email: requested.git_email.clone(),
        skip_permissions: requested.skip_permissions,
    };
    if !explicit.is_empty() {
        let _ = crate::modes::agent::preferences::save(pool.inner(), &project_path, &explicit).await;
    }

    // Sessions are machine-local — only agent_contexts travel through
    // cloud sync, so no `bump("agent")` here.

    // Lazy MCP registration for non-Claude providers. Boot does NOT
    // auto-register these (would touch ~/.codex/config.toml or
    // ~/.config/opencode/opencode.json for every alpha tester who has
    // those CLIs but uses Clauge as Claude-only). Triggered here so the
    // user has explicitly opted in by creating a session in that
    // provider. Best-effort; silent on failure.
    crate::modes::workspace::commands::ensure_provider_mcp_registered(
        pool.inner(),
        &provider,
    ).await;

    sessions_repo::get_session_by_id(pool.inner(), &id).await.map_err(|e| e.to_string())
}

/// ⋯ → Branch: copy this conversation into a NEW session using the provider's
/// own fork support (Claude: `--resume <parent id> --fork-session`). Files stay
/// in the same working tree; the two conversations diverge from here.
///
/// The new row deliberately keeps `claude_session_id` NULL: the spawn path
/// resumes the PARENT id with the fork flag, and the capture poll then persists
/// the forked conversation's own id. Storing the parent id here would make
/// every later reopen resume the parent's history instead of the fork's.
#[tauri::command]
pub async fn agent_fork_session(
    pool: State<'_, SqlitePool>,
    session_id: String,
) -> Result<AgentSession, String> {
    let parent = sessions_repo::get_session_by_id(pool.inner(), &session_id)
        .await
        .map_err(|e| e.to_string())?;

    if parent.provider != "claude" {
        return Err(format!(
            "Branch is only available for Claude sessions right now (native --fork-session); '{}' has no fork support.",
            parent.provider
        ));
    }
    if parent
        .claude_session_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .is_none()
    {
        return Err(
            "This session has no saved conversation yet — send it a message first, then branch."
                .to_string(),
        );
    }

    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let title = format!("{} ⑂", parent.title.trim());

    sessions_repo::insert_session(
        pool.inner(),
        &id,
        &title,
        &parent.purpose,
        &parent.project_path,
        parent.project_root.as_deref(),
        &parent.project_name,
        &parent.context_prompt,
        parent.skip_permissions,
        parent.git_name.as_deref(),
        parent.git_email.as_deref(),
        &now,
        &now,
        &parent.provider,
        parent.binary_path.as_deref(),
        parent.profile.as_deref(),
        None,
        None,
        0,
    )
    .await
    .map_err(|e| e.to_string())?;

    // Context sets ride along: a branch is the same task with the same
    // background, just a different continuation point.
    if let Ok(contexts) = sessions_repo::list_contexts_for_session(pool.inner(), &parent.id).await {
        for context in contexts {
            let _ = sessions_repo::attach_context_to_session(pool.inner(), &id, &context.id).await;
        }
    }

    sessions_repo::mark_session_forked(pool.inner(), &id, &parent.id)
        .await
        .map_err(|e| e.to_string())?;

    sessions_repo::get_session_by_id(pool.inner(), &id)
        .await
        .map_err(|e| e.to_string())
}

/// First 40 chars of the first non-empty line — the Quick Bar title default.
pub fn title_from_prompt(prompt: &str) -> String {
    let line = prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("New session");
    let mut title: String = line.chars().take(40).collect();
    if line.chars().count() > 40 {
        title.push('…');
    }
    title
}

impl SessionOptions {
    /// Purpose text is normally supplied by the frontend from
    /// `SESSION_PURPOSES`; this keeps CLI/Quick-Bar callers (which only send a
    /// purpose id) from landing with an empty context prompt.
    fn purpose_prompt_fallback(purpose: &str) -> String {
        match purpose {
            "Custom" => String::new(),
            other => format!("You are working in a ZeroAny Workbench {other} session."),
        }
    }
}

#[tauri::command]
pub fn agent_list_profiles(provider: String) -> Result<Vec<String>, String> {
    runner_for(provider.trim()).profiles()
}

#[tauri::command]
pub async fn agent_update_session(
    pool: State<'_, SqlitePool>, id: String,
    title: Option<String>, skip_permissions: Option<bool>,
    git_name: Option<String>, git_email: Option<String>, context_prompt: Option<String>,
    // Pass `Some("")` (empty string) to CLEAR the per-session override
    // and restore the default $PATH lookup. `None` means "don't touch
    // the existing value" — keeps the partial-update semantics the
    // other Option params already use.
    binary_path: Option<String>,
) -> Result<(), String> {
    if let Some(t) = title {
        sessions_repo::update_session_title(pool.inner(), &id, &t).await.map_err(|e| e.to_string())?;
    }
    if let Some(sp) = skip_permissions {
        let val = if sp { 1 } else { 0 };
        sessions_repo::update_session_skip_permissions(pool.inner(), &id, val).await.map_err(|e| e.to_string())?;
    }
    if let Some(ref name) = git_name {
        sessions_repo::update_session_git_name(pool.inner(), &id, name).await.map_err(|e| e.to_string())?;
    }
    if let Some(ref email) = git_email {
        sessions_repo::update_session_git_email(pool.inner(), &id, email).await.map_err(|e| e.to_string())?;
    }
    if let Some(ref prompt) = context_prompt {
        sessions_repo::update_session_context_prompt(pool.inner(), &id, prompt).await.map_err(|e| e.to_string())?;
    }
    if let Some(bp) = binary_path {
        let trimmed = bp.trim();
        let arg = if trimmed.is_empty() { None } else { Some(trimmed) };
        sessions_repo::update_session_binary_path(pool.inner(), &id, arg).await.map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn agent_delete_session(pool: State<'_, SqlitePool>, id: String) -> Result<(), String> {
    // Guard: a session with children nested under it (manual drag&drop or
    // automatic fork lineage) cannot be deleted — the children would lose
    // their visible parent. Move them out or delete them first.
    let children = sessions_repo::count_child_sessions(pool.inner(), &id)
        .await
        .map_err(|e| e.to_string())?;
    if children > 0 {
        return Err(format!(
            "This session has {children} child session(s) nested under it. \
             Move them to top level (drag onto the project header) or delete them first."
        ));
    }
    sessions_repo::delete_session(pool.inner(), &id).await.map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn agent_update_session_id(pool: State<'_, SqlitePool>, id: String, claude_session_id: String) -> Result<(), String> {
    let val = if claude_session_id.is_empty() { None } else { Some(claude_session_id) };
    sessions_repo::update_session_claude_id(pool.inner(), &id, val.as_deref()).await.map_err(|e| e.to_string())
}

/// Manual nesting (drag & drop in the left nav): move `id` under `parent_id`,
/// or detach to top-level when `parent_id` is None/empty. Validation (cycles,
/// depth, existence) lives in the repo so it holds for every caller.
#[tauri::command]
pub async fn agent_set_session_parent(
    pool: State<'_, SqlitePool>,
    id: String,
    parent_id: Option<String>,
) -> Result<(), String> {
    sessions_repo::set_session_parent(pool.inner(), &id, parent_id.as_deref()).await
}

/// Push the Workbench session title into the provider's own session store.
///
/// Hermes >= 0.20 auto-titles a session from the first user message, so
/// without this sync the provider-side session list shows the first prompt
/// instead of the name the user picked in Workbench. `sessions rename`
/// records `user` provenance — the highest tier — so Hermes' auto-titling
/// never overwrites it again. Claude receives its title natively at spawn
/// (`--name`), so this command is Hermes-only. Best-effort callers wrap it
/// and ignore errors: a failed rename must never block the terminal.
pub async fn sync_session_title_to_provider(session: &crate::modes::agent::models::AgentSession) -> Result<(), String> {
    if session.provider != "hermes" {
        return Ok(());
    }
    let Some(sid) = session.claude_session_id.as_deref().filter(|s| !s.trim().is_empty()) else {
        return Ok(());
    };
    if session.title.trim().is_empty() {
        return Ok(());
    }
    crate::shared::cli::hermes::rename_session_title(
        session.binary_path.as_deref(),
        session.profile.as_deref(),
        sid,
        &session.title,
    )
    .await
}

#[tauri::command]
pub async fn agent_sync_session_title(pool: State<'_, SqlitePool>, id: String) -> Result<(), String> {
    let session = sessions_repo::get_session_by_id(pool.inner(), &id).await.map_err(|e| e.to_string())?;
    sync_session_title_to_provider(&session).await
}

#[tauri::command]
pub async fn agent_update_last_used(pool: State<'_, SqlitePool>, id: String) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    sessions_repo::update_session_last_used(pool.inner(), &id, &now).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn agent_update_worktree(pool: State<'_, SqlitePool>, id: String, worktree_path: Option<String>, worktree_branch: Option<String>) -> Result<(), String> {
    sessions_repo::update_session_worktree(pool.inner(), &id, worktree_path.as_deref(), worktree_branch.as_deref()).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn agent_list_contexts(pool: State<'_, SqlitePool>) -> Result<Vec<AgentContext>, String> {
    sessions_repo::list_contexts(pool.inner()).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn agent_save_context(pool: State<'_, SqlitePool>, id: Option<String>, name: String, content: String) -> Result<AgentContext, String> {
    let now = chrono::Utc::now().to_rfc3339();
    let ctx_id = id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    sessions_repo::upsert_context(pool.inner(), &ctx_id, &name, &content, &now, &now)
        .await
        .map_err(|e| e.to_string())?;
    crate::cloud::scheduler::bump("agent");
    sessions_repo::get_context_by_id(pool.inner(), &ctx_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn agent_delete_context(pool: State<'_, SqlitePool>, id: String) -> Result<(), String> {
    sessions_repo::delete_context(pool.inner(), &id).await.map_err(|e| e.to_string())?;
    crate::cloud::scheduler::bump("agent");
    Ok(())
}

#[tauri::command]
pub async fn agent_get_session_contexts(pool: State<'_, SqlitePool>, session_id: String) -> Result<Vec<AgentContext>, String> {
    sessions_repo::list_contexts_for_session(pool.inner(), &session_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn agent_attach_context(pool: State<'_, SqlitePool>, session_id: String, context_id: String) -> Result<(), String> {
    sessions_repo::attach_context_to_session(pool.inner(), &session_id, &context_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn agent_detach_context(pool: State<'_, SqlitePool>, session_id: String, context_id: String) -> Result<(), String> {
    sessions_repo::detach_context_from_session(pool.inner(), &session_id, &context_id).await.map_err(|e| e.to_string())
}

// Each CLI reads its own project-level context file. Inject writes to
// exactly one file based on the session's provider so we don't pollute
// the user's repo with files no agent will read. Remove cleans the
// marker block from every known file defensively — handles the case
// where the user attached the same context to sessions under
// different providers in the same project.
const CTX_MARKER_START: &str = "<!-- ZEROANY-CONTEXT-START -->";
const CTX_MARKER_END: &str = "<!-- ZEROANY-CONTEXT-END -->";
const LEGACY_CTX_MARKER_START: &str = "<!-- CLAUGE-CONTEXT-START -->";
const LEGACY_CTX_MARKER_END: &str = "<!-- CLAUGE-CONTEXT-END -->";
// Purpose prompt is its own separate marker pair so it coexists with
// user-attached contexts (ZEROANY-CONTEXT-*) without either feature
// stomping the other's block when one updates and the other doesn't.
const PURPOSE_MARKER_START: &str = "<!-- ZEROANY-PURPOSE-START -->";
const PURPOSE_MARKER_END: &str = "<!-- ZEROANY-PURPOSE-END -->";
const LEGACY_PURPOSE_MARKER_START: &str = "<!-- CLAUGE-PURPOSE-START -->";
const LEGACY_PURPOSE_MARKER_END: &str = "<!-- CLAUGE-PURPOSE-END -->";
const ALL_CONTEXT_FILES: &[&str] = &["CLAUDE.md", "AGENTS.md", "GEMINI.md"];

fn strip_marker_block(content: &str, start_marker: &str, end_marker: &str) -> String {
    if let (Some(start), Some(end)) = (content.find(start_marker), content.find(end_marker)) {
        let before = content[..start].trim_end();
        let after = &content[end + end_marker.len()..];
        format!("{}{}", before, after)
    } else {
        content.to_string()
    }
}

fn context_file_for(provider: &str) -> &'static str {
    match provider {
        "codex" | "opencode" | "hermes" => "AGENTS.md",
        "gemini" => "GEMINI.md",
        _ => "CLAUDE.md",
    }
}

fn write_injected_context(path: &PathBuf, contexts: &[(String, String)]) -> Result<(), String> {
    let existing_content = if path.exists() {
        let raw = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let without_current = strip_marker_block(&raw, CTX_MARKER_START, CTX_MARKER_END);
        strip_marker_block(
            &without_current,
            LEGACY_CTX_MARKER_START,
            LEGACY_CTX_MARKER_END,
        )
    } else {
        String::new()
    };

    // Filter out snippets whose content already exists in the file
    let mut filtered = String::new();
    for (name, content) in contexts {
        if !existing_content.contains(content.trim()) {
            if !filtered.is_empty() { filtered.push_str("\n\n---\n\n"); }
            filtered.push_str(&format!("## {}\n\n{}", name, content));
        }
    }
    if filtered.is_empty() { return Ok(()); }

    let injected = format!("\n\n{}\n{}\n{}\n", CTX_MARKER_START, filtered, CTX_MARKER_END);
    if !existing_content.is_empty() {
        std::fs::write(path, format!("{}{}", existing_content.trim_end(), injected)).map_err(|e| e.to_string())?;
    } else {
        std::fs::write(path, filtered).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn agent_inject_contexts(
    pool: State<'_, SqlitePool>,
    project_path: String,
    context_ids: Vec<String>,
    provider: Option<String>,
) -> Result<(), String> {
    if context_ids.is_empty() { return Ok(()); }

    // Fetch context content from DB by ID
    let mut contexts: Vec<(String, String)> = Vec::new();
    for id in &context_ids {
        let row = sessions_repo::get_context_name_and_content(pool.inner(), id)
            .await
            .map_err(|e| e.to_string())?;
        if let Some(ctx) = row {
            contexts.push(ctx);
        }
    }
    if contexts.is_empty() { return Ok(()); }

    // Write to the single file the session's CLI actually reads.
    // Claude → CLAUDE.md, Codex / OpenCode → AGENTS.md.
    let filename = context_file_for(provider.as_deref().unwrap_or("claude"));
    let path = PathBuf::from(&project_path).join(filename);
    write_injected_context(&path, &contexts)
}

#[tauri::command]
pub fn agent_remove_injected_contexts(project_path: String) -> Result<(), String> {
    // Defensive sweep: if the user attached contexts under one provider
    // then later switched the session's provider, the previous file
    // would otherwise be orphaned. Clean every known context file's
    // marker block — no-op on files that don't exist.
    for filename in ALL_CONTEXT_FILES {
        let path = PathBuf::from(&project_path).join(filename);
        if !path.exists() { continue; }

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let without_current = strip_marker_block(&content, CTX_MARKER_START, CTX_MARKER_END);
        let cleaned = strip_marker_block(
            &without_current,
            LEGACY_CTX_MARKER_START,
            LEGACY_CTX_MARKER_END,
        );
        if cleaned != content {
            if cleaned.trim().is_empty() {
                let _ = std::fs::remove_file(&path);
            } else {
                std::fs::write(&path, cleaned.trim_end().to_string() + "\n").map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

/// Write the session's purpose prompt into the provider's project-level
/// context file inside `<!-- ZEROANY-PURPOSE-START --> … <!-- ZEROANY-PURPOSE-END -->`.
///
/// Background: every other supported CLI exposes a real system-prompt
/// flag (Claude `--append-system-prompt`, Codex `-c instructions=…`).
/// Gemini does NOT — the closest analogue, `--prompt-interactive
/// <text>`, smuggles the prompt into the user-message channel and
/// makes Gemini execute it immediately at spawn / on resume instead
/// of waiting for the user's first turn. That's the "Gemini starts
/// running the moment I open the session" bug the alpha tester reported.
/// Writing the prompt into `GEMINI.md` before spawn moves the prompt
/// into the context channel where it belongs — Gemini reads the file at
/// startup and treats it as ambient instructions. The TUI opens and
/// waits for user input, matching Claude/Codex behaviour.
///
/// Preserves any pre-existing content in the file (user's own
/// GEMINI.md / CLAUDE.md / AGENTS.md) by only touching the marked
/// block. Coexists with the attach-contexts feature because the two
/// use different marker pairs.
fn write_injected_purpose(path: &PathBuf, purpose_text: &str) -> Result<(), String> {
    let trimmed = purpose_text.trim();
    // Read existing file, stripping both current and legacy purpose blocks.
    let existing = if path.exists() {
        let raw = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let without_current = strip_marker_block(&raw, PURPOSE_MARKER_START, PURPOSE_MARKER_END);
        strip_marker_block(
            &without_current,
            LEGACY_PURPOSE_MARKER_START,
            LEGACY_PURPOSE_MARKER_END,
        )
    } else {
        String::new()
    };

    // Empty prompt → just clean. Removes the marker block; deletes the
    // file if nothing else remains.
    if trimmed.is_empty() {
        let cleaned = existing.trim();
        if cleaned.is_empty() {
            if path.exists() {
                let _ = std::fs::remove_file(path);
            }
        } else {
            std::fs::write(path, cleaned.to_string() + "\n")
                .map_err(|e| e.to_string())?;
        }
        return Ok(());
    }

    // Build the new block. Single trailing newline at end so the file
    // ends cleanly whether or not the original had content.
    let block = format!(
        "{}\n\n{}\n\n{}",
        PURPOSE_MARKER_START, trimmed, PURPOSE_MARKER_END
    );
    let final_content = if existing.trim().is_empty() {
        format!("{}\n", block)
    } else {
        format!("{}\n\n{}\n", existing.trim_end(), block)
    };
    std::fs::write(path, final_content).map_err(|e| e.to_string())?;
    Ok(())
}

/// Inject (or clear) the session's purpose prompt into the provider's
/// project-level context file. Called BEFORE spawn from the frontend.
///
/// Providers without a reliable system-prompt flag use a marker block in
/// their project context file. Other providers are a no-op so the frontend
/// can invoke this command unconditionally.
#[tauri::command]
pub fn agent_inject_purpose(
    project_path: String,
    provider: String,
    purpose_prompt: String,
) -> Result<(), String> {
    // Providers without a real system-prompt CLI flag get their persona
    // written into the project's agent file pre-spawn. Claude / Codex
    // both have first-class flags (`--append-system-prompt` and `-c
    // instructions=`), so their persona travels via the spawn command
    // instead — no file write needed. Gemini, OpenCode, and Hermes write
    // to a marker block in their respective context file (GEMINI.md /
    // AGENTS.md), which doesn't collide with the contexts feature's own
    // marker block.
    if provider != "gemini" && provider != "opencode" && provider != "hermes" {
        return Ok(());
    }
    let filename = context_file_for(&provider);
    let path = PathBuf::from(&project_path).join(filename);
    write_injected_purpose(&path, &purpose_prompt)
}

#[tauri::command]
pub fn agent_update_tray_title(app_handle: tauri::AppHandle, title: String) -> Result<(), String> {
    if let Some(tray) = app_handle.tray_by_id(&tauri::tray::TrayIconId::new("main-tray")) {
        tray.set_title(Some(&title)).map_err(|e| format!("Tray error: {}", e))?;
    }
    Ok(())
}

/// Quick "does this binary exist and run?" probe used by the
/// per-session custom-binary picker in NewSessionModal / EditSessionModal.
/// Calls `<path> --version` with a 3-second timeout. Returns the trimmed
/// stdout on success (e.g. "claude-code 1.4.2"), or an `Err` describing
/// what went wrong. Non-blocking: the modal uses the result as a hint
/// only — devs can still save a path that doesn't pass the probe (a CLI
/// might not implement --version, or might gate it behind login).
#[tauri::command]
pub fn agent_validate_binary(path: String) -> Result<String, String> {
    use std::time::Duration;
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("Path is empty".to_string());
    }
    if !std::path::Path::new(trimmed).is_file() {
        return Err(format!("File does not exist: {}", trimmed));
    }
    let child = std::process::Command::new(trimmed)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not execute: {}", e))?;

    // Wait with a 3s timeout. We don't pull in tokio for this single call
    // — a thread-and-sleep loop is fine for one short probe.
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(3);
    let mut child = child;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = String::new();
                let mut stderr = String::new();
                use std::io::Read;
                if let Some(mut s) = child.stdout.take() {
                    let _ = s.read_to_string(&mut stdout);
                }
                if let Some(mut s) = child.stderr.take() {
                    let _ = s.read_to_string(&mut stderr);
                }
                if status.success() {
                    let trimmed_out = stdout.trim();
                    return Ok(if trimmed_out.is_empty() { stderr.trim().to_string() } else { trimmed_out.to_string() });
                }
                let msg = if !stderr.trim().is_empty() { stderr.trim().to_string() } else { format!("Exited with status {}", status) };
                return Err(msg);
            }
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    return Err("Binary did not respond to --version within 3s".to_string());
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => return Err(format!("Wait failed: {}", e)),
        }
    }
}

#[tauri::command]
pub fn agent_check_claude_installed() -> bool {
    // Preserved as a Claude-specific name for the original callers; the
    // provider-aware variant below is the new shape going forward.
    let cli: &dyn CliRunner = runner_for("claude");
    cli.resolve_binary_path() != cli.binary_name()
}

#[tauri::command]
pub fn agent_check_cli_installed(provider: String) -> bool {
    // Used by the provider picker (NewSessionModal / coworker modal) to
    // grey out CLIs that aren't on PATH. `resolve_binary_path` returns
    // the bare binary name when `which` / `where.exe` fails, so a
    // distinct absolute path means the binary is installed.
    let cli: &dyn CliRunner = runner_for(&provider);
    cli.resolve_binary_path() != cli.binary_name()
}

/// Batched probe used by the app-boot pre-flight: one Tauri round-trip
/// instead of four. Returns the installed map keyed by provider id.
#[tauri::command]
pub fn agent_check_clis_installed() -> std::collections::HashMap<String, bool> {
    let mut out = std::collections::HashMap::new();
    for p in ["claude", "codex", "gemini", "opencode", "hermes"] {
        let cli: &dyn CliRunner = runner_for(p);
        out.insert(p.to_string(), cli.resolve_binary_path() != cli.binary_name());
    }
    out
}

#[tauri::command]
pub fn agent_get_claude_plan() -> Result<String, String> {
    let output = std::process::Command::new("security")
        .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
        .output()
        .map_err(|e| format!("Keychain error: {}", e))?;
    if !output.status.success() {
        return Ok(String::new());
    }
    let json_str = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;
    let parsed: serde_json::Value =
        serde_json::from_str(json_str.trim()).map_err(|e| e.to_string())?;
    Ok(parsed
        .get("claudeAiOauth")
        .and_then(|o| o.get("subscriptionType").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string())
}

#[cfg(test)]
mod purpose_tests {
    use super::*;

    #[test]
    fn hermes_purpose_is_written_to_agents_md() {
        let dir = std::env::temp_dir().join(format!(
            "zeroany-hermes-purpose-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("AGENTS.md"), "# Existing project guidance\n").unwrap();

        agent_inject_purpose(
            dir.to_string_lossy().into_owned(),
            "hermes".into(),
            "Review before editing".into(),
        )
        .unwrap();

        let content = std::fs::read_to_string(dir.join("AGENTS.md")).unwrap();
        assert!(content.contains("# Existing project guidance"));
        assert!(content.contains("<!-- ZEROANY-PURPOSE-START -->"));
        assert!(content.contains("Review before editing"));
        assert!(content.contains("<!-- ZEROANY-PURPOSE-END -->"));
        assert!(!content.contains("CLAUGE"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn legacy_purpose_marker_is_migrated_without_duplication() {
        let dir = std::env::temp_dir().join(format!(
            "zeroany-legacy-purpose-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("AGENTS.md"),
            "# Existing project guidance\n\n<!-- CLAUGE-PURPOSE-START -->\n\nOld prompt\n\n<!-- CLAUGE-PURPOSE-END -->\n",
        )
        .unwrap();

        agent_inject_purpose(
            dir.to_string_lossy().into_owned(),
            "hermes".into(),
            "New prompt".into(),
        )
        .unwrap();

        let content = std::fs::read_to_string(dir.join("AGENTS.md")).unwrap();
        assert!(content.contains("# Existing project guidance"));
        assert!(content.contains("<!-- ZEROANY-PURPOSE-START -->"));
        assert!(content.contains("New prompt"));
        assert!(!content.contains("Old prompt"));
        assert!(!content.contains("CLAUGE"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
