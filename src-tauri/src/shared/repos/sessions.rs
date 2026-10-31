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
    project_root: Option<&str>,
    project_name: &str,
    card_id: &str,
    coworker_id: &str,
    created_at: &str,
    last_used_at: &str,
    provider: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO agent_sessions \
         (id, title, purpose, project_path, project_root, project_name, context_prompt, \
          skip_permissions, git_name, git_email, created_at, last_used_at, \
          origin, card_id, coworker_id, provider) \
         VALUES (?, ?, '', ?, ?, ?, '', 0, NULL, NULL, ?, ?, 'card', ?, ?, ?)",
    )
    .bind(id)
    .bind(title)
    .bind(project_path)
    .bind(project_root)
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
    project_root: Option<&str>,
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
        "INSERT INTO agent_sessions (id, title, purpose, project_path, project_root, project_name, context_prompt, skip_permissions, git_name, git_email, created_at, last_used_at, provider, binary_path, profile, base_branch, worktree_branch, worktree_enabled) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(title)
    .bind(purpose)
    .bind(project_path)
    .bind(project_root)
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

pub async fn update_session_project_identity(
    pool: &SqlitePool,
    id: &str,
    project_root: &str,
    project_name: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE agent_sessions SET project_root = ?, project_name = ? WHERE id = ?")
        .bind(project_root)
        .bind(project_name)
        .bind(id)
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

/// One-shot first message captured by the Quick Bar. Written at creation,
/// consumed once by the spawn path, then cleared (`None`).
pub async fn update_session_initial_prompt(
    pool: &SqlitePool,
    id: &str,
    prompt: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE agent_sessions SET initial_prompt = ? WHERE id = ?")
        .bind(prompt)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Spawn lifecycle for the session row: idle | starting | running | exited | failed.
pub async fn update_session_spawn_state(
    pool: &SqlitePool,
    id: &str,
    state: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE agent_sessions SET spawn_state = ? WHERE id = ?")
        .bind(state)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Record that `id` is a native provider fork of `parent_id`. Written after
/// the insert so `insert_session` keeps its signature.
pub async fn mark_session_forked(
    pool: &SqlitePool,
    id: &str,
    parent_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE agent_sessions SET parent_session_id = ?, fork_kind = 'native' WHERE id = ?",
    )
    .bind(parent_id)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Sessions that would render nested under `id` in the left nav: manual
/// children (`parent_session_id`) plus auto fork-lineage children whose
/// `parent_external_session_id` matches this session's resume id and that
/// carry no manual override (a detached `''` or a manual parent elsewhere
/// means the row no longer nests here). Hidden card sessions never render
/// in the nav, so they don't count. Used to block deleting a parent.
pub async fn count_child_sessions(pool: &SqlitePool, id: &str) -> Result<i64, sqlx::Error> {
    let external: Option<String> =
        sqlx::query_scalar("SELECT claude_session_id FROM agent_sessions WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?
            .flatten();
    let manual: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agent_sessions \
         WHERE parent_session_id = ? AND origin = 'manual' AND id != ?",
    )
    .bind(id)
    .bind(id)
    .fetch_one(pool)
    .await?;
    let mut total = manual;
    if let Some(ext) = external.filter(|e| !e.trim().is_empty()) {
        let lineage: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM agent_sessions \
             WHERE parent_external_session_id = ? AND parent_session_id IS NULL \
               AND origin = 'manual' AND id != ?",
        )
        .bind(ext)
        .bind(id)
        .fetch_one(pool)
        .await?;
        total += lineage;
    }
    Ok(total)
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
    // Read the previous external id first: managed fork children store their
    // parent's external id in parent_external_session_id, and when a parent's
    // tip advances (Hermes compression recapture) those references must follow,
    // or the left-nav nesting silently breaks.
    let previous: Option<String> =
        sqlx::query_scalar("SELECT claude_session_id FROM agent_sessions WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?
            .flatten();
    sqlx::query("UPDATE agent_sessions SET claude_session_id = ? WHERE id = ?")
        .bind(claude_session_id)
        .bind(id)
        .execute(pool)
        .await?;
    if let (Some(old), Some(new)) = (previous, claude_session_id) {
        if !old.is_empty() && old != new {
            sqlx::query(
                "UPDATE agent_sessions SET parent_external_session_id = ? \
                 WHERE parent_external_session_id = ?",
            )
            .bind(new)
            .bind(old)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

/// Manual nesting (drag & drop): set or clear a session's manual parent.
/// `None`/empty parent detaches to explicit top-level (`''`, which also
/// suppresses automatic fork-lineage nesting in the UI).
///
/// Validation keeps the manual forest well-formed without the frontend
/// having to be trusted:
/// - both sessions must exist (and the child must be a visible manual row)
/// - no self-parenting
/// - no cycles: the proposed parent must not be a descendant of the child
/// - depth cap: the child's new ancestor chain must stay under 4 levels
pub async fn set_session_parent(
    pool: &SqlitePool,
    id: &str,
    parent_id: Option<&str>,
) -> Result<(), String> {
    const MAX_ANCESTORS: usize = 3; // child ends up at depth <= 3 (4 visible levels)
    let parent_id = parent_id.map(str::trim).unwrap_or("");

    let child: Option<String> = sqlx::query_scalar(
        "SELECT id FROM agent_sessions WHERE id = ? AND origin = 'manual'",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;
    if child.is_none() {
        return Err("Session not found".into());
    }

    if !parent_id.is_empty() {
        if parent_id == id {
            return Err("A session cannot be its own parent".into());
        }
        let parent: Option<String> =
            sqlx::query_scalar("SELECT id FROM agent_sessions WHERE id = ? AND origin = 'manual'")
                .bind(parent_id)
                .fetch_optional(pool)
                .await
                .map_err(|e| e.to_string())?;
        if parent.is_none() {
            return Err("Parent session not found".into());
        }
        // Walk the proposed parent's manual ancestor chain: reject cycles
        // (child appears in it) and enforce the depth cap. Bounded walk so
        // pre-existing corrupt data can't hang the command.
        let mut current = parent_id.to_string();
        let mut ancestors = 0usize;
        let mut seen = std::collections::HashSet::new();
        loop {
            if current == id {
                return Err("Cannot move a session under its own descendant".into());
            }
            if !seen.insert(current.clone()) {
                break; // existing cycle in stored data — stop walking
            }
            let next: Option<Option<String>> =
                sqlx::query_scalar("SELECT parent_session_id FROM agent_sessions WHERE id = ?")
                    .bind(&current)
                    .fetch_optional(pool)
                    .await
                    .map_err(|e| e.to_string())?;
            let Some(next_parent) = next.flatten().filter(|p| !p.is_empty()) else {
                break;
            };
            current = next_parent;
            ancestors += 1;
            if ancestors > MAX_ANCESTORS {
                return Err("Nesting is limited to 4 levels".into());
            }
        }
    }

    let result = sqlx::query("UPDATE agent_sessions SET parent_session_id = ? WHERE id = ?")
        .bind(parent_id)
        .bind(id)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
    if result.rows_affected() == 0 {
        return Err("Session not found".into());
    }
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
            Some("/tmp/project"),
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
        assert_eq!(session.project_root.as_deref(), Some("/tmp/project"));
    }

    #[tokio::test]
    async fn updating_session_external_id_repoints_fork_children() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::migrator::run(&pool).await.unwrap();

        for (id, title) in [("parent-1", "Parent"), ("child-1", "Child")] {
            insert_session(
                &pool,
                id,
                title,
                "External",
                "/tmp/project",
                Some("/tmp/project"),
                "project",
                "",
                0,
                None,
                None,
                "2026-08-01T00:00:00Z",
                "2026-08-01T00:00:00Z",
                "hermes",
                None,
                None,
                None,
                None,
                1,
            )
            .await
            .unwrap();
        }
        update_session_claude_id(&pool, "parent-1", Some("ext-old"))
            .await
            .unwrap();
        sqlx::query(
            "UPDATE agent_sessions SET parent_external_session_id = 'ext-old' WHERE id = 'child-1'",
        )
        .execute(&pool)
        .await
        .unwrap();

        // A tip advance (Hermes compression recapture) must drag fork children
        // along, or left-nav nesting silently breaks.
        update_session_claude_id(&pool, "parent-1", Some("ext-new"))
            .await
            .unwrap();
        let child = get_session_by_id(&pool, "child-1").await.unwrap();
        assert_eq!(child.parent_external_session_id.as_deref(), Some("ext-new"));

        // Clearing the parent's external id (reset) has no new id to point at;
        // the child keeps its last known parent reference.
        update_session_claude_id(&pool, "parent-1", None).await.unwrap();
        let child = get_session_by_id(&pool, "child-1").await.unwrap();
        assert_eq!(child.parent_external_session_id.as_deref(), Some("ext-new"));
    }

    async fn seed_manual_sessions(pool: &SqlitePool, ids: &[&str]) {
        for id in ids {
            insert_session(
                pool,
                id,
                id,
                "Custom",
                "/tmp/project",
                Some("/tmp/project"),
                "project",
                "",
                0,
                None,
                None,
                "2026-08-01T00:00:00Z",
                "2026-08-01T00:00:00Z",
                "hermes",
                None,
                None,
                None,
                None,
                1,
            )
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn manual_parent_nesting_validates_and_detaches() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::migrator::run(&pool).await.unwrap();
        seed_manual_sessions(&pool, &["a", "b", "c", "d", "e", "f"]).await;

        // Basic nesting: b under a.
        set_session_parent(&pool, "b", Some("a")).await.unwrap();
        assert_eq!(
            get_session_by_id(&pool, "b").await.unwrap().parent_session_id.as_deref(),
            Some("a")
        );

        // Self-parenting rejected.
        let err = set_session_parent(&pool, "a", Some("a")).await.unwrap_err();
        assert!(err.contains("own parent"), "{err}");

        // Cycle rejected: a under b (b is already under a).
        let err = set_session_parent(&pool, "a", Some("b")).await.unwrap_err();
        assert!(err.contains("descendant"), "{err}");

        // Unknown parent rejected.
        let err = set_session_parent(&pool, "c", Some("nope")).await.unwrap_err();
        assert!(err.contains("not found"), "{err}");

        // Depth cap: a←b←c←d←e is 4 levels (allowed), f under e is the 5th (rejected).
        set_session_parent(&pool, "c", Some("b")).await.unwrap();
        set_session_parent(&pool, "d", Some("c")).await.unwrap();
        set_session_parent(&pool, "e", Some("d")).await.unwrap();
        let err = set_session_parent(&pool, "f", Some("e")).await.unwrap_err();
        assert!(err.contains("4 levels"), "{err}");

        // Detach writes '' (explicit top-level), not NULL.
        set_session_parent(&pool, "b", None).await.unwrap();
        assert_eq!(
            get_session_by_id(&pool, "b").await.unwrap().parent_session_id.as_deref(),
            Some("")
        );

        // Hidden card sessions cannot be dragged (child must be a manual row).
        sqlx::query(
            "INSERT INTO agent_sessions (id, title, purpose, project_path, project_name, \
             created_at, last_used_at, origin, card_id) \
             VALUES ('card-1','Card','Custom','/tmp/project','project', \
             '2026-08-01T00:00:00Z','2026-08-01T00:00:00Z','card','card-x')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let err = set_session_parent(&pool, "card-1", Some("a")).await.unwrap_err();
        assert!(err.contains("not found"), "{err}");
    }

    #[tokio::test]
    async fn child_count_blocks_delete_for_manual_and_lineage_children() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        crate::db::migrator::run(&pool).await.unwrap();
        seed_manual_sessions(&pool, &["p", "m", "l", "d"]).await;
        // Parent resume id — lineage children reference this.
        update_session_claude_id(&pool, "p", Some("ext-p")).await.unwrap();

        // No children yet.
        assert_eq!(count_child_sessions(&pool, "p").await.unwrap(), 0);

        // Manual child counts.
        set_session_parent(&pool, "m", Some("p")).await.unwrap();
        assert_eq!(count_child_sessions(&pool, "p").await.unwrap(), 1);

        // Auto fork-lineage child counts (parent_external matches resume id).
        sqlx::query(
            "UPDATE agent_sessions SET parent_external_session_id = 'ext-p' WHERE id = 'l'",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(count_child_sessions(&pool, "p").await.unwrap(), 2);

        // A detached row ('' manual override) no longer counts as a child.
        sqlx::query(
            "UPDATE agent_sessions SET parent_external_session_id = 'ext-p', parent_session_id = '' WHERE id = 'd'",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(count_child_sessions(&pool, "p").await.unwrap(), 2);

        // Detaching the manual child drops the count; deleting stays blocked
        // while the lineage child remains.
        set_session_parent(&pool, "m", None).await.unwrap();
        assert_eq!(count_child_sessions(&pool, "p").await.unwrap(), 1);
    }
}
