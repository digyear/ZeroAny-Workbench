//! Schema migrations for the ZeroAny Workbench SQLite database.
//!
//! Migrations live as numbered SQL files under `src-tauri/migrations/`
//! (`V<n>__<description>.sql`). The `sqlx::migrate!` macro embeds them at
//! compile time, computes per-migration checksums, and runs each one
//! exactly once per database — tracked in the `_sqlx_migrations` table.
//!
//! Adding a new migration: drop a numbered `.sql` file in `migrations/`,
//! rebuild. No code changes required here.
//!
//! For databases that pre-date the introduction of this migrator (alpha
//! users on the old hand-rolled runner), [`run`] first calls
//! [`super::bootstrap::seed_existing_install`] to detect what's already
//! applied and seed `_sqlx_migrations` with the matching checksums.
//! Without that step, sqlx-migrate would attempt to re-run V1–Vn against
//! schemas that already exist, hit duplicate-table / duplicate-column
//! errors, roll back the transaction, and fail.

use sqlx::sqlite::SqlitePool;

use super::bootstrap;

/// Compile-time-embedded migration set. The path is relative to the
/// crate root (`src-tauri/`).
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Bring the database to the latest schema version, preserving existing data.
///
/// Steps:
///   1. Bootstrap `_sqlx_migrations` for legacy databases (recover state-B
///      from the old broken v7, then schema-probe each version's signature
///      and seed the tracking table with matching checksums).
///   2. Run any unapplied migrations transactionally via sqlx-migrate.
pub async fn run(pool: &SqlitePool) -> Result<(), String> {
    bootstrap::seed_existing_install(pool, &MIGRATOR)
        .await
        .map_err(|e| format!("migration bootstrap: {}", e))?;

    MIGRATOR
        .run(pool)
        .await
        .map_err(|e| format!("migration apply: {}", e))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    #[tokio::test]
    async fn fresh_database_has_latest_agent_session_columns() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        run(&pool).await.unwrap();

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('agent_discovered_sessions') \
             WHERE name = 'project_root'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1);

        let profile_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('agent_sessions') \
             WHERE name = 'profile'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(profile_count, 1);

        let worktree_enabled_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('agent_sessions') \
             WHERE name = 'worktree_enabled'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(worktree_enabled_count, 1);
        let worktree_enabled_default: String = sqlx::query_scalar(
            "SELECT dflt_value FROM pragma_table_info('agent_sessions') \
             WHERE name = 'worktree_enabled'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(worktree_enabled_default, "1");

        let managed_project_root_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('agent_sessions') \
             WHERE name = 'project_root'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(managed_project_root_count, 1);

        let discovered_profile_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('agent_discovered_sessions') \
             WHERE name = 'profile'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(discovered_profile_count, 1);

        // Provider-native ids are only unique inside a Hermes profile.
        // The catalog must preserve both rows so each can be resumed from
        // its owning state.db.
        for (id, profile) in [
            ("hermes:default:same-id", "default"),
            ("hermes:cozy-engineer:same-id", "cozy-engineer"),
        ] {
            sqlx::query(
                "INSERT INTO agent_discovered_sessions (
                    id, provider, profile, external_session_id,
                    created_at, updated_at, last_seen_at
                 ) VALUES (?, 'hermes', ?, 'same-id', 'now', 'now', 'now')",
            )
            .bind(id)
            .bind(profile)
            .execute(&pool)
            .await
            .unwrap();
        }
        let same_id_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM agent_discovered_sessions \
             WHERE provider = 'hermes' AND external_session_id = 'same-id'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(same_id_count, 2);
    }

    #[tokio::test]
    async fn discovered_profile_migration_preserves_existing_catalog_state() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE agent_sessions (id TEXT PRIMARY KEY);
             INSERT INTO agent_sessions (id) VALUES ('managed-1');
             CREATE TABLE agent_discovered_sessions (
                id TEXT PRIMARY KEY,
                provider TEXT NOT NULL,
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
                UNIQUE(provider, external_session_id)
             );
             INSERT INTO agent_discovered_sessions (
                id, provider, external_session_id, project_path, project_root,
                project_name, title, created_at, updated_at, last_seen_at,
                hidden, hidden_at, adopted_agent_session_id
             ) VALUES (
                'hermes:legacy-id', 'hermes', 'legacy-id', '/repo', '/repo',
                'repo', 'Legacy', 'created', 'updated', 'seen', 1, 'hidden-at',
                'managed-1'
             );",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::raw_sql(include_str!(
            "../../migrations/29_agent_discovered_session_profile.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();

        let row: (String, String, i64, Option<String>) = sqlx::query_as(
            "SELECT id, profile, hidden, adopted_agent_session_id
             FROM agent_discovered_sessions",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "hermes:default:legacy-id");
        assert_eq!(row.1, "default");
        assert_eq!(row.2, 1);
        assert_eq!(row.3.as_deref(), Some("managed-1"));
    }
}
