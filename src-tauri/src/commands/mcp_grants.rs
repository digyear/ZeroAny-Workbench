//! MCP exposure grants — the UI-facing half of `shared::repos::mcp_grants`.
//!
//! Deliberately resource-kind agnostic: the SSH panel uses `ssh` today and
//! the SQL panel will call the same two commands with `sql` when that
//! exposure lands.

use sqlx::SqlitePool;
use tauri::State;

use crate::shared::repos::mcp_grants::{self, KIND_SQL, KIND_SSH, McpGrant};

const VALID_SCOPES: [&str; 3] = ["read_only", "service_control", "full"];

fn validate_kind(kind: &str) -> Result<(), String> {
    if matches!(kind, KIND_SSH | KIND_SQL) {
        Ok(())
    } else {
        Err(format!("unknown resource_kind: {}", kind))
    }
}

/// List the grants for one kind. Rows are only created once the user has
/// touched the toggle, so a connection missing here is simply "not exposed".
#[tauri::command]
pub async fn mcp_grants_list(
    pool: State<'_, SqlitePool>,
    resource_kind: String,
) -> Result<Vec<McpGrant>, String> {
    validate_kind(&resource_kind)?;
    mcp_grants::list_for_kind(pool.inner(), &resource_kind)
        .await
        .map_err(|e| e.to_string())
}

/// Expose / hide one connection and pick its scope in one call.
#[tauri::command]
pub async fn mcp_grant_set(
    pool: State<'_, SqlitePool>,
    resource_kind: String,
    resource_id: String,
    enabled: bool,
    scope: String,
) -> Result<(), String> {
    validate_kind(&resource_kind)?;
    if !VALID_SCOPES.contains(&scope.as_str()) {
        return Err(format!(
            "invalid scope `{}` — expected one of {:?}",
            scope, VALID_SCOPES
        ));
    }
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    mcp_grants::set(
        pool.inner(),
        &resource_kind,
        &resource_id,
        enabled,
        &scope,
        &now,
    )
    .await
    .map_err(|e| e.to_string())?;
    crate::cloud::scheduler::bump("mcp");
    Ok(())
}
