//! Per-project session defaults ("记忆") for the Quick Bar.
//!
//! Two granularities coexist (decision D2): `project_root` (default) and
//! `worktree`. Which one is used is a global setting, so switching it never
//! clobbers the other key's row.

use crate::modes::agent::worktree::resolve_project_identity;
use crate::shared::repos::settings as settings_repo;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tauri::State;

/// Setting key holding the active granularity: `project_root` | `worktree`.
pub const SCOPE_SETTING_KEY: &str = "agent_preferences_scope";
pub const SCOPE_PROJECT_ROOT: &str = "project_root";
pub const SCOPE_WORKTREE: &str = "worktree";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPreferences {
    pub provider: Option<String>,
    pub profile: Option<String>,
    pub purpose: Option<String>,
    pub base_branch: Option<String>,
    pub worktree_enabled: Option<bool>,
    /// Context set names (stable across machines; ids are local).
    pub context_names: Option<Vec<String>>,
    pub git_name: Option<String>,
    pub git_email: Option<String>,
    pub skip_permissions: Option<bool>,
}

impl ProjectPreferences {
    /// True when nothing has been remembered yet — callers then fall back to
    /// global defaults instead of persisting an all-None row.
    pub fn is_empty(&self) -> bool {
        self.provider.is_none()
            && self.profile.is_none()
            && self.purpose.is_none()
            && self.base_branch.is_none()
            && self.worktree_enabled.is_none()
            && self.context_names.is_none()
            && self.git_name.is_none()
            && self.git_email.is_none()
            && self.skip_permissions.is_none()
    }
}

pub async fn scope_mode(pool: &SqlitePool) -> String {
    match settings_repo::get_by_key(pool, SCOPE_SETTING_KEY).await {
        Ok(Some(setting)) if setting.value == SCOPE_WORKTREE => SCOPE_WORKTREE.to_string(),
        _ => SCOPE_PROJECT_ROOT.to_string(),
    }
}

/// Resolve `(scope_kind, scope_key)` for a session path under the active mode.
pub async fn resolve_scope(pool: &SqlitePool, project_path: &str) -> (String, String) {
    let mode = scope_mode(pool).await;
    let path = project_path.trim().to_string();
    if mode == SCOPE_WORKTREE {
        return (SCOPE_WORKTREE.to_string(), path);
    }
    let identity_path = path.clone();
    match tokio::task::spawn_blocking(move || resolve_project_identity(&identity_path)).await {
        Ok(identity) => (SCOPE_PROJECT_ROOT.to_string(), identity.project_root),
        Err(_) => (SCOPE_PROJECT_ROOT.to_string(), path),
    }
}

pub async fn load(
    pool: &SqlitePool,
    project_path: &str,
) -> Result<Option<ProjectPreferences>, String> {
    let (kind, key) = resolve_scope(pool, project_path).await;
    let row: Option<(
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i64>,
    )> = sqlx::query_as(
        "SELECT provider, profile, purpose, base_branch, worktree_enabled, \
                context_names, git_name, git_email, skip_permissions \
         FROM agent_project_preferences WHERE scope_kind = ? AND scope_key = ?",
    )
    .bind(&kind)
    .bind(&key)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;

    Ok(row.map(|r| ProjectPreferences {
        provider: r.0.filter(|s| !s.trim().is_empty()),
        profile: r.1.filter(|s| !s.trim().is_empty()),
        purpose: r.2.filter(|s| !s.trim().is_empty()),
        base_branch: r.3.filter(|s| !s.trim().is_empty()),
        worktree_enabled: r.4.map(|v| v != 0),
        context_names: r
            .5
            .and_then(|json| serde_json::from_str::<Vec<String>>(&json).ok()),
        git_name: r.6.filter(|s| !s.trim().is_empty()),
        git_email: r.7.filter(|s| !s.trim().is_empty()),
        skip_permissions: r.8.map(|v| v != 0),
    }))
}

pub async fn save(
    pool: &SqlitePool,
    project_path: &str,
    prefs: &ProjectPreferences,
) -> Result<(), String> {
    let (kind, key) = resolve_scope(pool, project_path).await;
    let now = chrono::Utc::now().to_rfc3339();
    let context_names = prefs
        .context_names
        .as_ref()
        .map(|names| serde_json::to_string(names).unwrap_or_else(|_| "[]".to_string()));
    sqlx::query(
        "INSERT INTO agent_project_preferences (
            scope_kind, scope_key, provider, profile, purpose, base_branch,
            worktree_enabled, context_names, git_name, git_email,
            skip_permissions, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(scope_kind, scope_key) DO UPDATE SET
            provider = excluded.provider,
            profile = excluded.profile,
            purpose = excluded.purpose,
            base_branch = excluded.base_branch,
            worktree_enabled = excluded.worktree_enabled,
            context_names = excluded.context_names,
            git_name = excluded.git_name,
            git_email = excluded.git_email,
            skip_permissions = excluded.skip_permissions,
            updated_at = excluded.updated_at",
    )
    .bind(&kind)
    .bind(&key)
    .bind(prefs.provider.as_deref())
    .bind(prefs.profile.as_deref())
    .bind(prefs.purpose.as_deref())
    .bind(prefs.base_branch.as_deref())
    .bind(prefs.worktree_enabled.map(|v| if v { 1_i64 } else { 0_i64 }))
    .bind(context_names)
    .bind(prefs.git_name.as_deref())
    .bind(prefs.git_email.as_deref())
    .bind(prefs.skip_permissions.map(|v| if v { 1_i64 } else { 0_i64 }))
    .bind(&now)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn agent_get_project_preferences(
    pool: State<'_, SqlitePool>,
    project_path: String,
) -> Result<Option<ProjectPreferences>, String> {
    load(pool.inner(), &project_path).await
}

#[tauri::command]
pub async fn agent_set_project_preferences(
    pool: State<'_, SqlitePool>,
    project_path: String,
    preferences: ProjectPreferences,
) -> Result<(), String> {
    if preferences.is_empty() {
        return Ok(());
    }
    save(pool.inner(), &project_path, &preferences).await
}

#[tauri::command]
pub async fn agent_get_preferences_scope(pool: State<'_, SqlitePool>) -> Result<String, String> {
    Ok(scope_mode(pool.inner()).await)
}

#[tauri::command]
pub async fn agent_set_preferences_scope(
    pool: State<'_, SqlitePool>,
    scope: String,
) -> Result<(), String> {
    let value = if scope == SCOPE_WORKTREE {
        SCOPE_WORKTREE
    } else {
        SCOPE_PROJECT_ROOT
    };
    settings_repo::upsert(pool.inner(), SCOPE_SETTING_KEY, value)
        .await
        .map_err(|e| e.to_string())
}