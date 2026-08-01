use sqlx::SqlitePool;

use crate::modes::agent::models::{
    AgentDiscoveredSession, DiscoveredSessionListOptions, DiscoveredSessionUpsert,
};

pub async fn upsert_discovered_session(
    pool: &SqlitePool,
    item: &DiscoveredSessionUpsert,
) -> Result<AgentDiscoveredSession, sqlx::Error> {
    let profile = if item.provider == "hermes" {
        item.profile
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("default")
    } else {
        ""
    };
    let id = if profile.is_empty() {
        format!("{}:{}", item.provider, item.external_session_id)
    } else {
        format!("{}:{}:{}", item.provider, profile, item.external_session_id)
    };
    let created_at = item
        .created_at
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(&item.last_seen_at);
    let updated_at = item
        .updated_at
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(&item.last_seen_at);

    sqlx::query(
        "INSERT INTO agent_discovered_sessions (
            id, provider, profile, external_session_id, project_path, project_root, project_name,
            title, preview, created_at, updated_at, last_seen_at,
            parent_external_session_id, session_kind, source_path
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(provider, profile, external_session_id) DO UPDATE SET
            project_path = COALESCE(excluded.project_path, agent_discovered_sessions.project_path),
            project_root = CASE
                WHEN agent_discovered_sessions.project_root IS NOT NULL
                 AND agent_discovered_sessions.project_root != agent_discovered_sessions.project_path
                 AND excluded.project_root = excluded.project_path
                THEN agent_discovered_sessions.project_root
                ELSE COALESCE(excluded.project_root, agent_discovered_sessions.project_root)
            END,
            project_name = COALESCE(excluded.project_name, agent_discovered_sessions.project_name),
            title = COALESCE(excluded.title, agent_discovered_sessions.title),
            preview = COALESCE(excluded.preview, agent_discovered_sessions.preview),
            updated_at = CASE
                WHEN excluded.updated_at > agent_discovered_sessions.updated_at THEN excluded.updated_at
                ELSE agent_discovered_sessions.updated_at
            END,
            last_seen_at = excluded.last_seen_at,
            parent_external_session_id = COALESCE(excluded.parent_external_session_id, agent_discovered_sessions.parent_external_session_id),
            session_kind = COALESCE(excluded.session_kind, agent_discovered_sessions.session_kind),
            source_path = COALESCE(excluded.source_path, agent_discovered_sessions.source_path)",
    )
    .bind(&id)
    .bind(&item.provider)
    .bind(profile)
    .bind(&item.external_session_id)
    .bind(&item.project_path)
    .bind(&item.project_root)
    .bind(&item.project_name)
    .bind(&item.title)
    .bind(&item.preview)
    .bind(created_at)
    .bind(updated_at)
    .bind(&item.last_seen_at)
    .bind(&item.parent_external_session_id)
    .bind(&item.session_kind)
    .bind(&item.source_path)
    .execute(pool)
    .await?;

    get_by_provider_profile_external(pool, &item.provider, profile, &item.external_session_id).await
}

pub async fn get_by_provider_profile_external(
    pool: &SqlitePool,
    provider: &str,
    profile: &str,
    external_session_id: &str,
) -> Result<AgentDiscoveredSession, sqlx::Error> {
    sqlx::query_as::<_, AgentDiscoveredSession>(
        "SELECT id, provider, NULLIF(profile, '') AS profile, external_session_id,
                project_path, project_root, project_name, title, preview,
                created_at, updated_at, last_seen_at, parent_external_session_id,
                session_kind, source_path, hidden, hidden_at, adopted_agent_session_id
         FROM agent_discovered_sessions
         WHERE provider = ? AND profile = ? AND external_session_id = ?",
    )
    .bind(provider)
    .bind(profile)
    .bind(external_session_id)
    .fetch_one(pool)
    .await
}

pub async fn list_discovered_sessions(
    pool: &SqlitePool,
    opts: &DiscoveredSessionListOptions,
) -> Result<Vec<AgentDiscoveredSession>, sqlx::Error> {
    let include_hidden = opts.include_hidden.unwrap_or(false);
    let provider = opts
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let project_path = opts
        .project_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let search = opts
        .search
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let search_like = search.map(|s| format!("%{}%", s));

    sqlx::query_as::<_, AgentDiscoveredSession>(
        "SELECT
             discovered.id,
             discovered.provider,
             NULLIF(discovered.profile, '') AS profile,
             discovered.external_session_id,
             discovered.project_path,
             discovered.project_root,
             discovered.project_name,
             discovered.title,
             discovered.preview,
             discovered.created_at,
             discovered.updated_at,
             discovered.last_seen_at,
             discovered.parent_external_session_id,
             discovered.session_kind,
             discovered.source_path,
             discovered.hidden,
             discovered.hidden_at,
             COALESCE(
                 discovered.adopted_agent_session_id,
                 (
                     SELECT managed.id
                     FROM agent_sessions AS managed
                     WHERE managed.provider = discovered.provider
                       AND managed.claude_session_id = discovered.external_session_id
                       AND managed.origin = 'manual'
                       AND (
                            discovered.provider != 'hermes'
                            OR COALESCE(managed.profile, 'default') = discovered.profile
                       )
                     ORDER BY managed.last_used_at DESC
                     LIMIT 1
                 )
             ) AS adopted_agent_session_id
         FROM agent_discovered_sessions AS discovered
         WHERE (? = 1 OR discovered.hidden = 0)
           AND (? IS NULL OR discovered.provider = ?)
           AND (? IS NULL OR discovered.project_path = ?)
           AND (
                ? IS NULL
                OR discovered.external_session_id LIKE ?
                OR COALESCE(discovered.project_name, '') LIKE ?
                OR COALESCE(discovered.project_path, '') LIKE ?
                OR COALESCE(discovered.project_root, '') LIKE ?
                OR COALESCE(discovered.title, '') LIKE ?
                OR COALESCE(discovered.preview, '') LIKE ?
                OR COALESCE(discovered.profile, '') LIKE ?
           )
         ORDER BY discovered.provider ASC,
                  COALESCE(discovered.project_root, discovered.project_name, discovered.project_path, '') ASC,
                  discovered.updated_at DESC",
    )
    .bind(if include_hidden { 1 } else { 0 })
    .bind(provider)
    .bind(provider)
    .bind(project_path)
    .bind(project_path)
    .bind(search_like.as_deref())
    .bind(search_like.as_deref())
    .bind(search_like.as_deref())
    .bind(search_like.as_deref())
    .bind(search_like.as_deref())
    .bind(search_like.as_deref())
    .bind(search_like.as_deref())
    .bind(search_like.as_deref())
    .fetch_all(pool)
    .await
}

pub async fn set_hidden(
    pool: &SqlitePool,
    id: &str,
    hidden: bool,
    hidden_at: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE agent_discovered_sessions
         SET hidden = ?, hidden_at = ?
         WHERE id = ?",
    )
    .bind(if hidden { 1 } else { 0 })
    .bind(if hidden { hidden_at } else { None })
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn memory_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql(
            "CREATE TABLE agent_sessions (
                id TEXT PRIMARY KEY,
                provider TEXT NOT NULL,
                profile TEXT,
                claude_session_id TEXT,
                origin TEXT NOT NULL,
                last_used_at TEXT NOT NULL
            );
            CREATE TABLE agent_discovered_sessions (
                id TEXT PRIMARY KEY,
                provider TEXT NOT NULL,
                profile TEXT NOT NULL DEFAULT '',
                external_session_id TEXT NOT NULL,
                project_path TEXT,
                project_root TEXT,
                project_name TEXT,
                title TEXT,
                preview TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                last_seen_at TEXT NOT NULL,
                parent_external_session_id TEXT,
                session_kind TEXT,
                source_path TEXT,
                hidden INTEGER NOT NULL DEFAULT 0,
                hidden_at TEXT,
                adopted_agent_session_id TEXT,
                UNIQUE(provider, profile, external_session_id)
            );",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn upsert_deduplicates_by_provider_and_external_id() {
        let pool = memory_pool().await;
        let base = DiscoveredSessionUpsert {
            provider: "claude".into(),
            profile: None,
            external_session_id: "sid-1".into(),
            project_path: Some("/repo/.zeroany-worktrees/task".into()),
            project_root: Some("/repo".into()),
            project_name: Some("repo".into()),
            title: Some("First".into()),
            preview: Some("hello".into()),
            created_at: Some("2026-01-01T00:00:00Z".into()),
            updated_at: Some("2026-01-01T00:00:00Z".into()),
            last_seen_at: "2026-01-01T00:00:00Z".into(),
            parent_external_session_id: None,
            session_kind: Some("conversation".into()),
            source_path: Some("/tmp/a.jsonl".into()),
        };
        let first = upsert_discovered_session(&pool, &base).await.unwrap();

        let second = DiscoveredSessionUpsert {
            preview: Some("updated".into()),
            updated_at: Some("2026-01-02T00:00:00Z".into()),
            last_seen_at: "2026-01-03T00:00:00Z".into(),
            // Simulate a later scan after an arbitrary linked worktree was
            // deleted and Git could only fall back to the original cwd.
            project_root: Some("/repo/.zeroany-worktrees/task".into()),
            ..base
        };
        let row = upsert_discovered_session(&pool, &second).await.unwrap();
        let rows = list_discovered_sessions(
            &pool,
            &DiscoveredSessionListOptions {
                include_hidden: Some(true),
                provider: None,
                project_path: None,
                search: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(first.id, row.id);
        assert_eq!(rows.len(), 1);
        assert_eq!(row.preview.as_deref(), Some("updated"));
        assert_eq!(
            row.project_path.as_deref(),
            Some("/repo/.zeroany-worktrees/task")
        );
        assert_eq!(row.project_root.as_deref(), Some("/repo"));
        assert_eq!(row.updated_at, "2026-01-02T00:00:00Z");
        assert_eq!(row.last_seen_at, "2026-01-03T00:00:00Z");
    }

    #[tokio::test]
    async fn list_marks_matching_manual_session_as_adopted() {
        let pool = memory_pool().await;
        let discovered = DiscoveredSessionUpsert {
            provider: "hermes".into(),
            profile: Some("default".into()),
            external_session_id: "resume-1".into(),
            project_path: Some("/repo".into()),
            project_root: Some("/repo".into()),
            project_name: Some("repo".into()),
            title: Some("Existing session".into()),
            preview: None,
            created_at: Some("2026-01-01T00:00:00Z".into()),
            updated_at: Some("2026-01-01T00:00:00Z".into()),
            last_seen_at: "2026-01-01T00:00:00Z".into(),
            parent_external_session_id: None,
            session_kind: Some("conversation".into()),
            source_path: None,
        };
        upsert_discovered_session(&pool, &discovered).await.unwrap();
        sqlx::query(
            "INSERT INTO agent_sessions (
                id, provider, claude_session_id, origin, last_used_at
             ) VALUES ('managed-1', 'hermes', 'resume-1', 'manual', '2026-01-02T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let rows = list_discovered_sessions(
            &pool,
            &DiscoveredSessionListOptions {
                include_hidden: Some(false),
                provider: None,
                project_path: None,
                search: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].adopted_agent_session_id.as_deref(),
            Some("managed-1")
        );
    }

    #[tokio::test]
    async fn same_hermes_session_id_is_distinct_across_profiles() {
        let pool = memory_pool().await;
        let base = DiscoveredSessionUpsert {
            provider: "hermes".into(),
            profile: Some("default".into()),
            external_session_id: "same-id".into(),
            project_path: Some("/repo".into()),
            project_root: Some("/repo".into()),
            project_name: Some("repo".into()),
            title: Some("Default".into()),
            preview: None,
            created_at: Some("2026-01-01T00:00:00Z".into()),
            updated_at: Some("2026-01-01T00:00:00Z".into()),
            last_seen_at: "2026-01-01T00:00:00Z".into(),
            parent_external_session_id: None,
            session_kind: Some("conversation".into()),
            source_path: None,
        };
        let default = upsert_discovered_session(&pool, &base).await.unwrap();
        let named = upsert_discovered_session(
            &pool,
            &DiscoveredSessionUpsert {
                profile: Some("cozy-engineer".into()),
                title: Some("Cozy".into()),
                ..base
            },
        )
        .await
        .unwrap();

        assert_ne!(default.id, named.id);
        let rows = list_discovered_sessions(
            &pool,
            &DiscoveredSessionListOptions {
                include_hidden: Some(true),
                provider: Some("hermes".into()),
                project_path: None,
                search: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].profile.as_deref(), Some("cozy-engineer"));
        assert_eq!(rows[1].profile.as_deref(), Some("default"));
    }
}
