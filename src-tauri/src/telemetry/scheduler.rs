// 24-hour scheduler.
//
// Lifecycle:
//   1. Spawned once from lib.rs setup.
//   2. Loop: read `settings.telemetry_next_at`; if in the past, flush;
//      otherwise sleep until then.
//   3. Flush = drain counters, assemble payload, POST. Counters reset
//      regardless of POST outcome (we'd rather lose a day's data than
//      pile up).
//   4. After a successful POST, write `next_at = now + 24h`. After a
//      failed POST, write `next_at = now + 1h` so we retry sooner
//      without spamming.
//
// Fire-and-forget guarantees:
//   • Runs in its own tokio task; nothing on the user's interactive
//     path ever awaits this.
//   • HTTP call has a 10s timeout — even if the worker is down, the
//     scheduler is unblocked within 10s and goes back to sleeping.
//   • All errors are logged at WARN and swallowed. No panics.
//
// Opt-out:
//   • `settings.telemetry_optout = "true"` → flush is a no-op (counters
//     are still drained so they don't grow unbounded). The fingerprint
//     read still runs because it's the only place that ensures the
//     device_id row exists; that's a single-row INSERT once per install.

use std::time::Duration;

use chrono::Utc;
use sqlx::SqlitePool;
use tauri::AppHandle;
use tauri::Manager;

use crate::cloud::auth::AuthState;
use crate::cloud::config::{api_available, API_BASE_URL};
use crate::shared::repos::settings as settings_repo;
use crate::telemetry::counters::{drain, restore};
use crate::telemetry::device::fingerprint;
use crate::telemetry::payload::{assemble, collect_db_buckets};

const SETTING_NEXT_AT: &str = "telemetry_next_at";
const SETTING_OPTOUT: &str = "telemetry_optout";

const FLUSH_INTERVAL_SECS: i64 = 24 * 60 * 60;          // 24h on success
const FLUSH_RETRY_SECS: i64 = 60 * 60;                  // 1h on failure
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL_MAX: Duration = Duration::from_secs(15 * 60); // 15m max sleep

pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // Tiny grace period at boot so we don't compete with the rest
        // of setup (DB migrations, cloud status hydrate, etc.).
        tokio::time::sleep(Duration::from_secs(30)).await;
        run_loop(app).await;
    });
}

async fn run_loop(app: AppHandle) {
    loop {
        let pool = match app.try_state::<SqlitePool>() {
            Some(p) => p.inner().clone(),
            None => {
                // Pool not registered yet (very early boot). Try again later.
                tokio::time::sleep(Duration::from_secs(30)).await;
                continue;
            }
        };

        let now = Utc::now().timestamp();
        let next_at = settings_repo::get_i64_or(&pool, SETTING_NEXT_AT, 0).await;

        if next_at <= now {
            flush_once(&app, &pool).await;
        }

        // Recompute remaining time AFTER the flush — `flush_once`
        // updates `next_at` on its way out.
        let now = Utc::now().timestamp();
        let next_at = settings_repo::get_i64_or(&pool, SETTING_NEXT_AT, now + FLUSH_INTERVAL_SECS).await;
        let mut wait = next_at.saturating_sub(now).max(60); // never sleep <60s
        // Cap the single sleep at 15 minutes so settings changes
        // (e.g. opt-out toggled) take effect within a quarter hour
        // without needing to wake us explicitly.
        if wait > POLL_INTERVAL_MAX.as_secs() as i64 {
            wait = POLL_INTERVAL_MAX.as_secs() as i64;
        }
        tokio::time::sleep(Duration::from_secs(wait as u64)).await;
    }
}

async fn flush_once(_app: &AppHandle, pool: &SqlitePool) {
    // No API configured at build time — drain counters and skip.
    if !api_available() {
        let _ = drain();
        schedule_next(pool, FLUSH_INTERVAL_SECS).await;
        return;
    }

    let opted_out = settings_repo::get_bool_or(pool, SETTING_OPTOUT, false).await;

    // Drain counters even when opted out — otherwise they'd grow
    // unbounded for as long as the user has telemetry off.
    let drained = drain();

    if opted_out {
        schedule_next(pool, FLUSH_INTERVAL_SECS).await;
        return;
    }

    let device = fingerprint(pool).await;
    let db_counts = collect_db_buckets(pool).await;
    let payload = assemble(device, drained, db_counts);

    // Use the canonical token+provider pair so attribution works the
    // same way as every other Cloud API call. The worker's
    // `authenticate()` requires BOTH headers — Authorization alone
    // routes through as anonymous (which is why every telemetry row
    // had user_id = NULL until this fix).
    let auth_pair = _app
        .try_state::<AuthState>()
        .and_then(|s| s.active_token_and_provider());

    let endpoint = format!("{}/api/telemetry/heartbeat", API_BASE_URL);
    let client = match reqwest::Client::builder().timeout(HTTP_TIMEOUT).build() {
        Ok(c) => c,
        Err(e) => {
            log::warn!("[telemetry] client build failed: {}", e);
            schedule_next(pool, FLUSH_RETRY_SECS).await;
            return;
        }
    };

    let mut req = client.post(&endpoint).json(&payload);
    if let Some((token, provider)) = auth_pair {
        req = req
            .header("Authorization", format!("Bearer {}", token))
            .header("X-Provider", provider);
    }

    match req.send().await {
        Ok(resp) if resp.status().is_success() => {
            schedule_next(pool, FLUSH_INTERVAL_SECS).await;
        }
        Ok(resp) => {
            // 4xx → bad payload (don't restore — we'd just send the
            // same bad payload next time). 5xx → server problem,
            // restore counters and retry sooner.
            let status = resp.status();
            if status.is_server_error() {
                restore(&payload_to_drain(&payload));
                schedule_next(pool, FLUSH_RETRY_SECS).await;
            } else {
                log::warn!("[telemetry] heartbeat 4xx: {}", status);
                schedule_next(pool, FLUSH_INTERVAL_SECS).await;
            }
        }
        Err(e) => {
            // Network failure — most common case (offline, captive
            // portal, worker outage). Restore + back off.
            log::warn!("[telemetry] heartbeat send failed: {}", e);
            restore(&payload_to_drain(&payload));
            schedule_next(pool, FLUSH_RETRY_SECS).await;
        }
    }
}

async fn schedule_next(pool: &SqlitePool, delay_secs: i64) {
    let next_at = Utc::now().timestamp() + delay_secs;
    let _ = settings_repo::upsert(pool, SETTING_NEXT_AT, &next_at.to_string()).await;
}

// Rebuild a DrainResult from the bucketed payload — used when we need
// to restore counters after a failed send. Since the payload only
// carries bucket labels, the restore is approximate: we re-add one
// representative count per bucket. This is intentionally lossy; the
// alternative (storing the raw drained values alongside the payload)
// would risk the raw numbers being logged. Approximate restore is
// strictly better than losing the entire window.
fn payload_to_drain(p: &crate::telemetry::payload::HeartbeatPayload) -> crate::telemetry::counters::DrainResult {
    use crate::telemetry::counters::{
        DrainResult, MODE_AGENT, MODE_EXPLORER, MODE_NOSQL, MODE_REST, MODE_SQL, MODE_SSH,
        MODE_WORKSPACE,
    };

    let bucket_min = |label: &str| -> u64 {
        match label {
            "1-10" => 1,
            "11-100" => 11,
            "101-1k" => 101,
            "1k+" => 1001,
            _ => 0,
        }
    };

    let mut features = Vec::with_capacity(p.features.len());
    for (k, v) in &p.features {
        features.push((*k, bucket_min(v)));
    }
    let mut errors = Vec::with_capacity(p.errors.len());
    for (k, v) in &p.errors {
        errors.push((*k, bucket_min(v)));
    }

    let mut modes_bits = 0u32;
    for piece in p.modes_active.split(',') {
        modes_bits |= match piece {
            "rest" => MODE_REST,
            "sql" => MODE_SQL,
            "nosql" => MODE_NOSQL,
            "ssh" => MODE_SSH,
            "agent" => MODE_AGENT,
            "explorer" => MODE_EXPLORER,
            "workspace" => MODE_WORKSPACE,
            _ => 0,
        };
    }

    DrainResult {
        features,
        errors,
        modes_bits,
    }
}
