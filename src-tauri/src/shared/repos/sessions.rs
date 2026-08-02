use sqlx::SqlitePool;

use crate::modes::agent::models::{AgentContext, AgentSession};

// ---------------------------------------------------------------------------
// agent_sessions
// ---------------------------------------------------------------------------

/// Manual sessions only (the user-spawned terminal sessions surfaced
/// in the Agent panel). Hidden card-driven sessions are filtered out
/// so they don't pollute the panel's list — they're addressed via the
/// card drawer instead, see `find_hidden_session_for_card`.
pub async fn list_sessions(pool: &SqlitePool) -> Result<Vec<AgentSession>, sqlx::Error> {
    sqlx::query_as::<_, AgentSession>(
        "SELECT * FROM agent_sessions \
         WHERE origin = 'manual' \
         ORDER BY last_used_at DESC",
    )
    .fetch_all(pool)
    .await
}

/// Hidden session for a (card, coworker) pair, if any. Drawer chat
/// looks this up before creating a new one — same persona on the same
/// card always reuses its session (which preserves Claude's resume
/// thread for that conversation).
pub async fn find_hidden_session_for_card_and_coworker(
    pool: &SqlitePool,
    card_id: &str,
    coworker_id: &str,
) -> Result<Option<AgentSession>, sqlx::Error> {
    sqlx::query_as::<_, AgentSession>(
        "SELECT * FROM agent_sessions \
         WHERE card_id = ? AND coworker_id = ? AND origin = 'card' \
         ORDER BY last_used_at DESC LIMIT 1",
    )
    .bind(card_id)
    .bind(coworker_id)
    .fetch_optional(pool)
    .await
}

/// Insert a hidden ('card'-origin) session attached to a coworker.
/// Caller fills in title + project context; we stamp origin='card',
/// card_id, and coworker_id.
#[allow(clippy::too_many_arguments)]
pub async fn insert_hidden_session(
    pool: &SqlitePool,
    id: &str,
    title: &str,
    project_path: &str,
    project_name: &str,
    card_id: &str,
    coworker_id: &str,
    created_at: &str,
    last_used_at: &str,
    provider: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO agent_sessions \
         (id, title, purpose, project_path, project_name, context_prompt, \
          skip_permissions, git_name, git_email, created_at, last_used_at, \
          origin, card_id, coworker_id, provider) \
         VALUES (?, ?, '', ?, ?, '', 0, NULL, NULL, ?, ?, 'card', ?, ?, ?)",
    )
    .bind(id)
    .bind(title)
    .bind(project_path)
    .bind(project_name)
    .bind(created_at)
    .bind(last_used_at)
    .bind(card_id)
    .bind(coworker_id)
    .bind(provider)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_session_by_id(
    pool: &SqlitePool,
    id: &str,
) -> Result<AgentSession, sqlx::Error> {
    sqlx::query_as::<_, AgentSession>("SELECT * FROM agent_sessions WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
}

/// Most recently-used MANUAL session bound to `project_path`. Drives
/// the workspace auto-link + cards_claim heuristic: when an agent
/// mutates a card via MCP, we don't know its session id directly, so
/// we stamp the card with the session most likely to be the one
/// calling — i.e. the one the user has been actively interacting
/// with. Hidden ('card'-origin) sessions are excluded so a drawer
/// chat doesn't look like the calling terminal session.
pub async fn find_recent_session_for_project(
    pool: &SqlitePool,
    project_path: &str,
) -> Result<Option<AgentSession>, sqlx::Error> {
    sqlx::query_as::<_, AgentSession>(
        "SELECT * FROM agent_sessions \
         WHERE project_path = ? AND origin = 'manual' \
         ORDER BY last_used_at DESC LIMIT 1",
    )
    .bind(project_path)
    .fetch_optional(pool)
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_session(
    pool: &SqlitePool,
    id: &str,
    title: &str,
    purpose: &str,
    project_path: &str,
    project_name: &str,
    context_prompt: &str,
    skip_permissions: i32,
    git_name: Option<&str>,
    git_email: Option<&str>,
    created_at: &str,
    last_used_at: &str,
    provider: &str,
    binary_path: Option<&str>,
    profile: Option<&str>,
    base_branch: Option<&str>,
    worktree_branch: Option<&str>,
    worktree_enabled: i32,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO agent_sessions (id, title, purpose, project_path, project_name, context_prompt, skip_permissions, git_name, git_email, created_at, last_used_at, provider, binary_path, profile, base_branch, worktree_branch, worktree_enabled) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(title)
    .bind(purpose)
    .bind(project_path)
    .bind(project_name)
    .bind(context_prompt)
    .bind(skip_permissions)
    .bind(git_name)
    .bind(git_email)
    .bind(created_at)
    .bind(last_used_at)
    .bind(provider)
    .bind(binary_path)
    .bind(profile)
    .bind(base_branch)
    .bind(worktree_branch)
    .bind(worktree_enabled)
    .execute(pool)
    .await?;
    Ok(())
}

/// Set or clear the per-session binary override. Pass `None`/empty to
/// restore the default $PATH lookup behaviour.
pub async fn update_session_binary_path(
    pool: &SqlitePool,
    id: &str,
    binary_path: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE agent_sessions SET binary_path = ? WHERE id = ?")
        .bind(binary_path)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_session_title(
    pool: &SqlitePool,
    id: &str,
    title: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE agent_sessions SET title = ? WHERE id = ?")
        .bind(title)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_session_skip_permissions(
    pool: &SqlitePool,
    id: &str,
    skip: i32,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE agent_sessions SET skip_permissions = ? WHERE id = ?")
        .bind(skip)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_session_git_name(
    pool: &SqlitePool,
    id: &str,
    git_name: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE agent_sessions SET git_name = ? WHERE id = ?")
        .bind(git_name)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_session_git_email(
    pool: &SqlitePool,
    id: &str,
    git_email: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE agent_sessions SET git_email = ? WHERE id = ?")
        .bind(git_email)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_session_context_prompt(
    pool: &SqlitePool,
    id: &str,
    context_prompt: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE agent_sessions SET context_prompt = ? WHERE id = ?")
        .bind(context_prompt)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_session(pool: &SqlitePool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM agent_sessions WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_session_claude_id(
    pool: &SqlitePool,
    id: &str,
    claude_session_id: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE agent_sessions SET claude_session_id = ? WHERE id = ?")
        .bind(claude_session_id)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_session_last_used(
    pool: &SqlitePool,
    id: &str,
    last_used_at: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE agent_sessions SET last_used_at = ? WHERE id = ?")
        .bind(last_used_at)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_session_worktree(
    pool: &SqlitePool,
    id: &str,
    worktree_path: Option<&str>,
    worktree_branch: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE agent_sessions SET worktree_path = ?, worktree_branch = ? WHERE id = ?",
    )
    .bind(worktree_path)
    .bind(worktree_branch)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// agent_contexts
// ---------------------------------------------------------------------------

pub async fn list_contexts(pool: &SqlitePool) -> Result<Vec<AgentContext>, sqlx::Error> {
    sqlx::query_as::<_, AgentContext>("SELECT * FROM agent_contexts ORDER BY name")
        .fetch_all(pool)
        .await
}

pub async fn get_context_by_id(
    pool: &SqlitePool,
    id: &str,
) -> Result<AgentContext, sqlx::Error> {
    sqlx::query_as::<_, AgentContext>("SELECT * FROM agent_contexts WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
}

pub async fn upsert_context(
    pool: &SqlitePool,
    id: &str,
    name: &str,
    content: &str,
    created_at: &str,
    updated_at: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO agent_contexts (id, name, content, created_at, updated_at) VALUES (?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET name = excluded.name, content = excluded.content, updated_at = excluded.updated_at",
    )
    .bind(id)
    .bind(name)
    .bind(content)
    .bind(created_at)
    .bind(updated_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_context(pool: &SqlitePool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM agent_contexts WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn get_context_name_and_content(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<(String, String)>, sqlx::Error> {
    sqlx::query_as("SELECT name, content FROM agent_contexts WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
}

// ---------------------------------------------------------------------------
// agent_session_contexts (join table)
// ---------------------------------------------------------------------------

pub async fn list_contexts_for_session(
    pool: &SqlitePool,
    session_id: &str,
) -> Result<Vec<AgentContext>, sqlx::Error> {
    sqlx::query_as::<_, AgentContext>(
        "SELECT c.* FROM agent_contexts c INNER JOIN agent_session_contexts sc ON c.id = sc.context_id WHERE sc.session_id = ? ORDER BY c.name",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await
}

pub async fn attach_context_to_session(
    pool: &SqlitePool,
    session_id: &str,
    context_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT OR IGNORE INTO agent_session_contexts (session_id, context_id) VALUES (?, ?)",
    )
    .bind(session_id)
    .bind(context_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn detach_context_from_session(
    pool: &SqlitePool,
    session_id: &str,
    context_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM agent_session_contexts WHERE session_id = ? AND context_id = ?")
        .bind(session_id)
        .bind(context_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    #[tokio::test]
    async fn insert_and_reload_preserves_provider_profile() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::migrator::run(&pool).await.unwrap();

        insert_session(
            &pool,
            "session-1",
            "Profile session",
            "Custom",
            "/tmp/project",
            "project",
            "",
            0,
            None,
            None,
            "2026-08-01T00:00:00Z",
            "2026-08-01T00:00:00Z",
            "hermes",
            None,
            Some("work"),
            None,
            None,
            0,
        )
        .await
        .unwrap();

        let session = get_session_by_id(&pool, "session-1").await.unwrap();
        assert_eq!(session.provider, "hermes");
        assert_eq!(session.profile.as_deref(), Some("work"));
        assert_eq!(session.worktree_enabled, 0);
    }
}
