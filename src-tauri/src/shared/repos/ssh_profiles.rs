use sqlx::SqlitePool;

use crate::modes::ssh::models::SshProfile;

pub async fn list_all(pool: &SqlitePool) -> Result<Vec<SshProfile>, sqlx::Error> {
    // `SELECT *` on purpose: an enumerated column list silently breaks every
    // list call the moment a migration adds a column the struct expects.
    sqlx::query_as::<_, SshProfile>(
        "SELECT * FROM ssh_profiles \
         ORDER BY (last_used_at IS NULL), last_used_at DESC, created_at DESC",
    )
    .fetch_all(pool)
    .await
}

pub async fn get_by_id(pool: &SqlitePool, id: &str) -> Result<SshProfile, sqlx::Error> {
    sqlx::query_as::<_, SshProfile>("SELECT * FROM ssh_profiles WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
}

#[allow(clippy::too_many_arguments)]
pub async fn insert(
    pool: &SqlitePool,
    id: &str,
    name: &str,
    host: &str,
    port: i64,
    username: &str,
    auth_type: &str,
    key_path: Option<&str>,
    accent_color: Option<&str>,
    created_at: &str,
    jump_profile_id: Option<&str>,
    proxy_command: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO ssh_profiles \
            (id, name, host, port, username, auth_type, key_path, accent_color, \
             last_used_at, created_at, jump_profile_id, proxy_command) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL, ?, ?, ?)",
    )
    .bind(id)
    .bind(name)
    .bind(host)
    .bind(port)
    .bind(username)
    .bind(auth_type)
    .bind(key_path)
    .bind(accent_color)
    .bind(created_at)
    .bind(jump_profile_id)
    .bind(proxy_command)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn update_name(pool: &SqlitePool, id: &str, name: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE ssh_profiles SET name = ? WHERE id = ?")
        .bind(name)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_host(pool: &SqlitePool, id: &str, host: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE ssh_profiles SET host = ? WHERE id = ?")
        .bind(host)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_port(pool: &SqlitePool, id: &str, port: i64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE ssh_profiles SET port = ? WHERE id = ?")
        .bind(port)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_username(
    pool: &SqlitePool,
    id: &str,
    username: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE ssh_profiles SET username = ? WHERE id = ?")
        .bind(username)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_auth_type(
    pool: &SqlitePool,
    id: &str,
    auth_type: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE ssh_profiles SET auth_type = ? WHERE id = ?")
        .bind(auth_type)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_key_path(
    pool: &SqlitePool,
    id: &str,
    key_path: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE ssh_profiles SET key_path = ? WHERE id = ?")
        .bind(key_path)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_accent_color(
    pool: &SqlitePool,
    id: &str,
    accent_color: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE ssh_profiles SET accent_color = ? WHERE id = ?")
        .bind(accent_color)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_by_id(pool: &SqlitePool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM ssh_profiles WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn touch_last_used(
    pool: &SqlitePool,
    id: &str,
    last_used_at: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE ssh_profiles SET last_used_at = ? WHERE id = ?")
        .bind(last_used_at)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Set or clear the jump-host pointer. Pass `None` to clear (direct connect).
pub async fn update_jump_profile_id(
    pool: &SqlitePool,
    id: &str,
    jump_profile_id: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE ssh_profiles SET jump_profile_id = ? WHERE id = ?")
        .bind(jump_profile_id)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Set or clear the proxy-command template. Pass `None` to clear.
pub async fn update_proxy_command(
    pool: &SqlitePool,
    id: &str,
    proxy_command: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE ssh_profiles SET proxy_command = ? WHERE id = ?")
        .bind(proxy_command)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A migration that adds a column must not break the profile list — the
    /// bug that hid every SSH profile from the UI (#4.4.4).
    #[tokio::test]
    async fn list_all_survives_columns_added_by_migrations() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        crate::db::migrator::MIGRATOR.run(&pool).await.unwrap();
        insert(
            &pool,
            "p1",
            "box",
            "host.example",
            22,
            "root",
            "password",
            None,
            None,
            "2026-01-01T00:00:00Z",
            None,
            None,
        )
        .await
        .unwrap();
        let rows = list_all(&pool).await.expect("list must not fail");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "box");
        assert!(rows[0].post_connect_inputs.is_none());
    }
}
