use crate::modes::ssh::models::SshProfile;
use crate::shared::platform::credential_store::{credential_store, CredentialStore};
use crate::shared::repos::ssh_profiles as ssh_profiles_repo;
use sqlx::SqlitePool;
use tauri::State;

#[tauri::command]
pub async fn ssh_list_profiles(pool: State<'_, SqlitePool>) -> Result<Vec<SshProfile>, String> {
    ssh_profiles_repo::list_all(pool.inner())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn ssh_create_profile(
    pool: State<'_, SqlitePool>,
    name: String,
    host: String,
    port: i64,
    username: String,
    auth_type: String,
    key_path: Option<String>,
    accent_color: Option<String>,
    secret: Option<String>,
    jump_profile_id: Option<String>,
    proxy_command: Option<String>,
) -> Result<SshProfile, String> {
    if !matches!(auth_type.as_str(), "key" | "password" | "agent" | "interactive") {
        return Err(format!("invalid auth_type: {}", auth_type));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

    ssh_profiles_repo::insert(
        pool.inner(),
        &id,
        &name,
        &host,
        port,
        &username,
        &auth_type,
        key_path.as_deref(),
        accent_color.as_deref(),
        &now,
        jump_profile_id.as_deref(),
        proxy_command.as_deref(),
    )
    .await
    .map_err(|e| e.to_string())?;

    if let Some(s) = secret {
        if !s.is_empty() {
            credential_store()
                .store(&id, &s)
                .await
                .map_err(|e| format!("credential store: {}", e))?;
        }
    }

    crate::cloud::scheduler::bump("ssh");

    ssh_profiles_repo::get_by_id(pool.inner(), &id)
        .await
        .map_err(|e| e.to_string())
}

/// Update profile fields. Each `Option<T>` follows the existing
/// "send-or-leave-alone" convention: `Some(value)` updates, `None`
/// leaves the column untouched.
///
/// For `jump_profile_id` and `proxy_command`, sending an empty string
/// clears the field (the connect path treats empty as "no proxy" — see
/// ssh_session::open_authenticated_ssh_session). Sending the field as
/// `null`/omitted leaves the existing value untouched.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn ssh_update_profile(
    pool: State<'_, SqlitePool>,
    id: String,
    name: Option<String>,
    host: Option<String>,
    port: Option<i64>,
    username: Option<String>,
    auth_type: Option<String>,
    key_path: Option<String>,
    accent_color: Option<String>,
    secret: Option<String>,
    jump_profile_id: Option<String>,
    proxy_command: Option<String>,
) -> Result<SshProfile, String> {
    if let Some(ref n) = name {
        ssh_profiles_repo::update_name(pool.inner(), &id, n)
            .await
            .map_err(|e| e.to_string())?;
    }
    if let Some(ref h) = host {
        ssh_profiles_repo::update_host(pool.inner(), &id, h)
            .await
            .map_err(|e| e.to_string())?;
    }
    if let Some(p) = port {
        ssh_profiles_repo::update_port(pool.inner(), &id, p)
            .await
            .map_err(|e| e.to_string())?;
    }
    if let Some(ref u) = username {
        ssh_profiles_repo::update_username(pool.inner(), &id, u)
            .await
            .map_err(|e| e.to_string())?;
    }
    if let Some(ref a) = auth_type {
        if !matches!(a.as_str(), "key" | "password" | "agent" | "interactive") {
            return Err(format!("invalid auth_type: {}", a));
        }
        ssh_profiles_repo::update_auth_type(pool.inner(), &id, a)
            .await
            .map_err(|e| e.to_string())?;
    }
    if let Some(ref kp) = key_path {
        ssh_profiles_repo::update_key_path(pool.inner(), &id, kp)
            .await
            .map_err(|e| e.to_string())?;
    }
    if let Some(ref ac) = accent_color {
        ssh_profiles_repo::update_accent_color(pool.inner(), &id, ac)
            .await
            .map_err(|e| e.to_string())?;
    }
    if let Some(s) = secret {
        let store = credential_store();
        // Replace: delete then store. delete is best-effort idempotent.
        let _ = store.delete(&id).await;
        if !s.is_empty() {
            store
                .store(&id, &s)
                .await
                .map_err(|e| format!("credential store: {}", e))?;
        }
    }
    // Some(value) updates the column; None leaves it alone. Empty string is
    // stored as-is — the connect path filters empty strings as "no proxy"
    // (since OpenSSH itself can't have a meaningful empty proxy config).
    if let Some(ref jump) = jump_profile_id {
        // Treat empty string as explicit clear (NULL the column) so the
        // foreign-key SET NULL semantics work cleanly.
        let val = if jump.is_empty() { None } else { Some(jump.as_str()) };
        ssh_profiles_repo::update_jump_profile_id(pool.inner(), &id, val)
            .await
            .map_err(|e| e.to_string())?;
    }
    if let Some(ref cmd) = proxy_command {
        let val = if cmd.is_empty() { None } else { Some(cmd.as_str()) };
        ssh_profiles_repo::update_proxy_command(pool.inner(), &id, val)
            .await
            .map_err(|e| e.to_string())?;
    }

    crate::cloud::scheduler::bump("ssh");

    ssh_profiles_repo::get_by_id(pool.inner(), &id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn ssh_delete_profile(
    pool: State<'_, SqlitePool>,
    id: String,
) -> Result<(), String> {
    // Best-effort: clear credential first. Failures here shouldn't block row deletion.
    let _ = credential_store().delete(&id).await;
    let _ = credential_store().delete(&totp_key(&id)).await;
    // Drop the MCP grant too — otherwise a deleted connection keeps a row
    // that would re-expose a *new* profile if the id ever got reused.
    let _ = crate::shared::repos::mcp_grants::delete(
        pool.inner(),
        crate::shared::repos::mcp_grants::KIND_SSH,
        &id,
    )
    .await;
    ssh_profiles_repo::delete_by_id(pool.inner(), &id)
        .await
        .map_err(|e| e.to_string())?;
    crate::cloud::scheduler::bump("ssh");
    Ok(())
}

#[tauri::command]
pub async fn ssh_touch_profile(
    pool: State<'_, SqlitePool>,
    id: String,
) -> Result<(), String> {
    // Use ISO-8601 (RFC 3339) so the value parses reliably in WKWebView's Date.
    // SQLite's `datetime('now')` returns "YYYY-MM-DD HH:MM:SS" which can yield
    // Invalid Date in Safari/WKWebView. Match the format used by created_at.
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    ssh_profiles_repo::touch_last_used(pool.inner(), &id, &now)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn ssh_get_credential(id: String) -> Result<Option<String>, String> {
    credential_store().get(&id).await
}

// ── TOTP enrolment ─────────────────────────────────────────────────────────
// A one-time code is the one thing an agent cannot supply. Enrolling the seed
// here lets the app compute it (see `modes::ssh::totp`), so an exposed profile
// works headless — at the cost of the "second factor" no longer being a second
// factor. Opt-in per profile, stored in the credential store next to the
// password, never in the profile row.

fn totp_key(id: &str) -> String {
    format!("totp:{}", id)
}

/// Enrol (or clear, with an empty string) the TOTP seed for a profile.
/// Returns the live code and its remaining lifetime so the UI can show "this
/// is the code the host will expect right now" — the only sane way to prove
/// the seed was pasted correctly before relying on it.
#[tauri::command]
pub async fn ssh_set_totp(
    id: String,
    secret: String,
) -> Result<serde_json::Value, String> {
    let store = credential_store();
    if secret.trim().is_empty() {
        let _ = store.delete(&totp_key(&id)).await;
        return Ok(serde_json::json!({ "enrolled": false }));
    }
    let normalized = crate::modes::ssh::totp::normalize_secret(&secret)?;
    let (code, seconds_left) = crate::modes::ssh::totp::current_code(&normalized)?;
    store
        .store(&totp_key(&id), &normalized)
        .await
        .map_err(|e| format!("credential store: {}", e))?;
    Ok(serde_json::json!({
        "enrolled": true,
        "code": code,
        "secondsLeft": seconds_left,
    }))
}

#[tauri::command]
pub async fn ssh_has_totp(id: String) -> Result<bool, String> {
    Ok(credential_store()
        .get(&totp_key(&id))
        .await
        .map_err(|e| format!("credential store: {}", e))?
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false))
}

/// Prove the enrolled seed actually satisfies this host, right after saving
/// it. Runs the real connect path non-interactively: with a correct seed it
/// authenticates without any prompt, which is exactly the property MCP calls
/// depend on. A wrong seed comes back as the "needs an interactive challenge"
/// refusal.
///
/// Note: this is a real login attempt — a bastion may count it against its
/// lockout budget, so it is a button the user presses rather than something
/// the UI does on every keystroke.
#[tauri::command]
pub async fn ssh_test_totp(
    pool: State<'_, SqlitePool>,
    id: String,
) -> Result<serde_json::Value, String> {
    if !ssh_has_totp(id.clone()).await? {
        return Err("no TOTP seed enrolled for this profile".to_string());
    }
    match crate::modes::ssh::ssh_session::open_authenticated_ssh_session(
        pool.inner(),
        &id,
        false,
    )
    .await
    {
        Ok(handle) => {
            let _ = handle
                .disconnect(russh::Disconnect::ByApplication, "", "en")
                .await;
            Ok(serde_json::json!({ "ok": true, "detail": "authenticated with the enrolled seed" }))
        }
        Err(e) => Ok(serde_json::json!({ "ok": false, "detail": e })),
    }
}

/// What to type into the shell right after it opens — the answer to a
/// bastion's asset menu. Newline-separated; blank clears it.
#[tauri::command]
pub async fn ssh_set_post_connect(
    pool: State<'_, SqlitePool>,
    id: String,
    inputs: String,
    delay_ms: Option<i64>,
) -> Result<(), String> {
    let normalised = inputs
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let value = if normalised.is_empty() {
        None
    } else {
        Some(normalised)
    };
    sqlx::query("UPDATE ssh_profiles SET post_connect_inputs = ?, post_connect_delay_ms = COALESCE(?, post_connect_delay_ms) WHERE id = ?")
        .bind(value)
        .bind(delay_ms.map(|d| d.clamp(0, 30_000)))
        .bind(&id)
        .execute(pool.inner())
        .await
        .map_err(|e| e.to_string())?;
    crate::cloud::scheduler::bump("ssh");
    Ok(())
}
