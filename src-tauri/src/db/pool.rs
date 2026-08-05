//! SQLite connection pool initialization for the ZeroAny Workbench app database.
//!
//! Lives in `app_data_dir/zeroany-workbench.db`, with `foreign_keys = ON` enforced
//! per-connection (SQLite default is OFF). The pool is created once during
//! `setup()` and managed via Tauri state for `#[tauri::command]` access.

use std::path::Path;
use std::str::FromStr;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};

/// Maximum concurrent connections to the SQLite database.
///
/// SQLite serialises writes regardless of pool size, but multiple read
/// connections can run concurrently in WAL mode. 5 covers the typical
/// frontend-driven query bursts (multiple list_* commands firing at once
/// from `loadX()` calls in the Svelte stores).
const MAX_CONNECTIONS: u32 = 5;

/// Open the ZeroAny Workbench SQLite pool, creating the file if missing.
///
/// `app_data_dir` is `~/Library/Application Support/com.zeroany.workbench/`
/// on macOS and the per-platform equivalent elsewhere — Tauri provides it
/// via `app.path().app_data_dir()`.
pub async fn init(app_data_dir: &Path) -> Result<SqlitePool, String> {
    std::fs::create_dir_all(app_data_dir)
        .map_err(|e| format!("create app data dir: {}", e))?;

    let db_path = app_data_dir.join("zeroany-workbench.db");
    let url = format!("sqlite:{}?mode=rwc", db_path.display());

    let opts = SqliteConnectOptions::from_str(&url)
        .map_err(|e| format!("invalid db url {}: {}", url, e))?
        .pragma("journal_mode", "WAL")
        .pragma("synchronous", "NORMAL")
        .pragma("busy_timeout", "5000")
        .pragma("foreign_keys", "ON")
        .create_if_missing(true);

    SqlitePoolOptions::new()
        .max_connections(MAX_CONNECTIONS)
        .connect_with(opts)
        .await
        .map_err(|e| format!("connect to {}: {}", db_path.display(), e))
}
