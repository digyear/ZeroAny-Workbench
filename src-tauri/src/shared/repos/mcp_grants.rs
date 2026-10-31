//! MCP exposure grants — which saved connections an external agent may
//! use through the built-in MCP server, and with what scope.
//!
//! One row per (kind, id). No row = not exposed, so nothing an agent can
//! reach was implicitly enabled: the user has to turn each connection on.
//!
//! Kinds: `ssh` today; `sql` will reuse the same table and scopes.

use sqlx::{Row, SqlitePool};

pub const KIND_SSH: &str = "ssh";
pub const KIND_SQL: &str = "sql";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpGrant {
    pub resource_kind: String,
    pub resource_id: String,
    pub enabled: bool,
    /// `read_only` | `service_control` | `full`. The meaning is per kind;
    /// SSH interprets it as a command allowlist (see `modes::ssh::policy`).
    pub scope: String,
    pub created_at: String,
    pub updated_at: String,
}

pub async fn list_for_kind(pool: &SqlitePool, kind: &str) -> Result<Vec<McpGrant>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT resource_kind, resource_id, enabled, scope, created_at, updated_at \
         FROM mcp_grants WHERE resource_kind = ? ORDER BY resource_id",
    )
    .bind(kind)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(row_to_grant).collect())
}

pub async fn get(pool: &SqlitePool, kind: &str, id: &str) -> Result<Option<McpGrant>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT resource_kind, resource_id, enabled, scope, created_at, updated_at \
         FROM mcp_grants WHERE resource_kind = ? AND resource_id = ?",
    )
    .bind(kind)
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(row_to_grant))
}

/// Enable/disable a connection and set its scope in one shot. Upsert so
/// the UI can flip a toggle without caring whether a row exists yet.
pub async fn set(
    pool: &SqlitePool,
    kind: &str,
    id: &str,
    enabled: bool,
    scope: &str,
    now: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO mcp_grants (resource_kind, resource_id, enabled, scope, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT(resource_kind, resource_id) DO UPDATE SET \
             enabled = excluded.enabled, \
             scope = excluded.scope, \
             updated_at = excluded.updated_at",
    )
    .bind(kind)
    .bind(id)
    .bind(enabled as i64)
    .bind(scope)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Drop the row entirely (used when the connection itself is deleted).
pub async fn delete(pool: &SqlitePool, kind: &str, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM mcp_grants WHERE resource_kind = ? AND resource_id = ?")
        .bind(kind)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

fn row_to_grant(row: sqlx::sqlite::SqliteRow) -> McpGrant {
    McpGrant {
        resource_kind: row.get("resource_kind"),
        resource_id: row.get("resource_id"),
        enabled: row.get::<i64, _>("enabled") != 0,
        scope: row.get("scope"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}
