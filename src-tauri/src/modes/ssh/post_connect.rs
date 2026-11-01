//! Post-connect input replay.
//!
//! A bastion like JumpServer does not give you a host shell on login — it
//! shows an asset menu and waits for a number, a name or an IP. Until that
//! is answered, every byte typed (a terminal keystroke or an agent's
//! command) feeds the menu instead of the host.
//!
//! A profile can store the answers; they are typed into the shell with a
//! delay so the menu has a chance to appear. Same data is used by the
//! terminal session and by MCP's shell channel.

use sqlx::SqlitePool;

/// Default wait before the first line: long enough for the menu to render
/// on a normal link, short enough not to feel laggy.
pub const DEFAULT_FIRST_DELAY_MS: u64 = 800;
/// Gap between subsequent lines.
pub const GAP_MS: u64 = 400;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PostConnect {
    pub lines: Vec<String>,
    pub first_delay_ms: u64,
}

impl PostConnect {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

/// Split the stored blob (one answer per line), dropping blank lines.
pub fn parse(raw: Option<&str>, delay_ms: Option<i64>) -> PostConnect {
    let lines = raw
        .unwrap_or("")
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>();
    let first = delay_ms
        .map(|d| d.clamp(0, 30_000) as u64)
        .unwrap_or(DEFAULT_FIRST_DELAY_MS);
    PostConnect {
        lines,
        first_delay_ms: first,
    }
}

/// Load a profile's script. Any DB error degrades to "nothing to replay" —
/// a broken lookup must not stop the user from connecting by hand.
pub async fn load(pool: &SqlitePool, profile_id: &str) -> PostConnect {
    let row: Option<(Option<String>, Option<i64>)> = sqlx::query_as(
        "SELECT post_connect_inputs, post_connect_delay_ms FROM ssh_profiles WHERE id = ?",
    )
    .bind(profile_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    match row {
        Some((raw, delay)) => parse(raw.as_deref(), delay),
        None => PostConnect::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_lines_and_spaces_are_dropped() {
        let p = parse(Some(" 2 \n\n\n10.0.0.5\n"), None);
        assert_eq!(p.lines, vec!["2".to_string(), "10.0.0.5".to_string()]);
        assert_eq!(p.first_delay_ms, DEFAULT_FIRST_DELAY_MS);
    }

    #[test]
    fn empty_stored_value_means_interact_directly() {
        assert!(parse(None, None).is_empty());
        assert!(parse(Some(""), Some(900)).is_empty());
    }

    #[test]
    fn delay_is_clamped_to_something_sane() {
        assert_eq!(parse(Some("1"), Some(-5)).first_delay_ms, 0);
        assert_eq!(parse(Some("1"), Some(999_999)).first_delay_ms, 30_000);
        assert_eq!(parse(Some("1"), Some(1500)).first_delay_ms, 1500);
    }
}