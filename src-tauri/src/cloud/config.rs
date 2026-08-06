// Compile-time API base. Set at build with CLAUGE_API_URL.
// Empty by default — all cloud endpoints (auth, sync, telemetry, push)
// become no-ops when no URL is configured.
pub const API_BASE_URL: &str = match option_env!("CLAUGE_API_URL") {
    Some(v) => v,
    None => "",
};

/// Returns true when a cloud API base URL has been configured at build time.
pub fn api_available() -> bool {
    !API_BASE_URL.is_empty()
}

// OS keyring service for cloud auth tokens. Separate service from SSH /
// Explorer secrets so wiping cloud auth doesn't touch user-saved credentials.
pub const KEYRING_SERVICE: &str = "ZeroAny Workbench Cloud Auth";

pub const KEY_GITHUB_ACCESS: &str = "github:access_token";
pub const KEY_GOOGLE_ACCESS: &str = "google:access_token";
pub const KEY_GOOGLE_REFRESH: &str = "google:refresh_token";
pub const KEY_GOOGLE_ID: &str = "google:id_token";

// settings table keys for non-secret bookkeeping.
pub const SETTINGS_KEY_USER_ID: &str = "cloud:user_id";
pub const SETTINGS_KEY_ACTIVE_PROVIDER: &str = "cloud:active_provider";
pub const SETTINGS_KEY_HAS_SYNCED: &str = "cloud:has_synced";
pub const SETTINGS_KEY_PLAN: &str = "cloud:plan";
// JSON snapshots of entitlements — read at boot so the AccountTab and
// pro-gates render the last-known values before the /api/auth/me round
// trip lands. Wiped by `auth::clear()` along with all other `cloud:%` rows.
pub const SETTINGS_KEY_CREDITS_SNAPSHOT: &str = "cloud:credits_snapshot";
pub const SETTINGS_KEY_SUB_SNAPSHOT: &str = "cloud:sub_snapshot";

// Per-kind last-pushed hash; key format `cloud:hash:<kind>`.
pub fn settings_key_hash(kind: &str) -> String {
    format!("cloud:hash:{}", kind)
}
// Per-kind last-synced ISO timestamp; key format `cloud:synced_at:<kind>`.
pub fn settings_key_synced_at(kind: &str) -> String {
    format!("cloud:synced_at:{}", kind)
}
// Per-kind conflict flag — set when a push returned 412 and the user
// hasn't resolved yet. Value stores the *remote* hash at conflict time,
// so the resolver can show summary stats against the right remote blob.
// Key format `cloud:conflict:<kind>`.
pub fn settings_key_conflict(kind: &str) -> String {
    format!("cloud:conflict:{}", kind)
}
