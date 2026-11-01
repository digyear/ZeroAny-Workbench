//! Post-connect steps: wait for the host to say something, then answer.
//!
//! A bastion like JumpServer does not give you a host shell on login — it
//! prints an asset menu and waits for a number, a name or an IP. Until that
//! is answered, every byte typed (a terminal keystroke or an agent's command)
//! feeds the menu instead of the host.
//!
//! A blind "sleep 800ms then type" is fragile: a slow link types into the
//! login banner, a fast one types before the menu renders. So a profile
//! stores *steps*: wait until this text shows up in the output, pause a
//! little, type the answer. A step without `expect` is a blind send (for
//! prompts that print nothing, like a bare `Press Enter`).
//!
//! The same sequence runs for the terminal session and for MCP's shell
//! channel — one configuration, both paths.

use std::time::Duration;

use russh::client::Msg;
use russh::{Channel, ChannelMsg};
use sqlx::SqlitePool;

/// Default pause after the expected text appears: enough for the prompt to
/// finish rendering, short enough that it doesn't feel like a hang.
pub const DEFAULT_DELAY_MS: u64 = 300;
/// How long to wait for one `expect` before giving up on that step.
pub const DEFAULT_TIMEOUT_MS: u64 = 15_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostStep {
    /// Text to wait for. `None` = send without waiting.
    pub expect: Option<String>,
    /// What to type (Enter is appended).
    pub send: String,
    /// Pause between seeing the text and typing.
    pub delay_ms: u64,
    /// Give up waiting for `expect` after this long (then send anyway).
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PostConnect {
    pub steps: Vec<PostStep>,
}

impl PostConnect {
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

/// Accept two shapes:
///   `expect => answer`   one step per line (the UI's format)
///   JSON `[{"expect": "...", "send": "104"}]`  for programmatic callers
/// A bare line with no arrow is a blind send, which keeps the older
/// "just type these lines" configuration working.
pub fn parse(raw: Option<&str>, delay_ms: Option<i64>) -> PostConnect {
    let raw = raw.unwrap_or("").trim();
    if raw.is_empty() {
        return PostConnect::default();
    }
    let delay = delay_ms
        .map(|d| d.clamp(0, 30_000) as u64)
        .unwrap_or(DEFAULT_DELAY_MS);

    if raw.starts_with('[') {
        if let Ok(steps) = serde_json::from_str::<Vec<serde_json::Value>>(raw) {
            let parsed = steps
                .into_iter()
                .filter_map(|value| {
                    let send = value.get("send")?.as_str()?.trim().to_string();
                    if send.is_empty() {
                        return None;
                    }
                    Some(PostStep {
                        expect: value
                            .get("expect")
                            .and_then(|v| v.as_str())
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty()),
                        send,
                        delay_ms: value
                            .get("delayMs")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(delay),
                        timeout_ms: value
                            .get("timeoutMs")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(DEFAULT_TIMEOUT_MS),
                    })
                })
                .collect::<Vec<_>>();
            return PostConnect { steps: parsed };
        }
    }

    let steps = raw
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .filter_map(|line| match line.split_once("=>") {
            Some((expect, send)) => {
                let send = send.trim();
                if send.is_empty() {
                    return None;
                }
                let expect = expect.trim();
                Some(PostStep {
                    expect: if expect.is_empty() {
                        None
                    } else {
                        Some(expect.to_string())
                    },
                    send: send.to_string(),
                    delay_ms: delay,
                    timeout_ms: DEFAULT_TIMEOUT_MS,
                })
            }
            None => Some(PostStep {
                expect: None,
                send: line.to_string(),
                delay_ms: delay,
                timeout_ms: DEFAULT_TIMEOUT_MS,
            }),
        })
        .collect();
    PostConnect { steps }
}

/// Load a profile's script. Any DB error degrades to "nothing to replay" — a
/// broken lookup must not stop the user from connecting by hand.
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


/// Wait until the channel goes quiet.
///
/// Answering an asset menu starts a *new* connection underneath — the bastion
/// prints progress dots, then the host's banner, then the prompt. Anything
/// sent during that window is swallowed. Rather than guess a sleep length,
/// wait for `quiet_ms` without output (and give up after `max_ms`).
pub async fn wait_for_quiet<F>(
    chan: &mut Channel<Msg>,
    quiet_ms: u64,
    max_ms: u64,
    mut on_data: F,
) -> Vec<u8>
where
    F: FnMut(&[u8]),
{
    let deadline = tokio::time::Instant::now() + Duration::from_millis(max_ms);
    let mut seen: Vec<u8> = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            break;
        }
        let window = left.min(Duration::from_millis(quiet_ms));
        match tokio::time::timeout(window, chan.wait()).await {
            Ok(Some(ChannelMsg::Data { data })) => {
                on_data(data.as_ref());
                seen.extend_from_slice(&data);
            }
            Ok(Some(ChannelMsg::ExtendedData { data, .. })) => {
                on_data(data.as_ref());
                seen.extend_from_slice(&data);
            }
            Ok(Some(_)) => {}
            // Channel closed, or no output for `quiet_ms`: settled.
            Ok(None) | Err(_) => break,
        }
    }
    seen
}

/// Run the steps against an open shell channel.
///
/// `on_data` receives every chunk read while waiting, so the terminal can
/// keep showing the banner/menu streaming in. Returns everything read (so a
/// caller can log why a step timed out).
pub async fn run_steps<F>(
    chan: &mut Channel<Msg>,
    steps: &[PostStep],
    mut on_data: F,
) -> Vec<u8>
where
    F: FnMut(&[u8]),
{
    let mut seen: Vec<u8> = Vec::new();
    for step in steps {
        if let Some(expect) = &step.expect {
            let needle = expect.as_bytes();
            let deadline =
                tokio::time::Instant::now() + Duration::from_millis(step.timeout_ms);
            let start = seen.len();
            loop {
                if seen[start..]
                    .windows(needle.len())
                    .any(|w| w == needle)
                {
                    break;
                }
                let left = deadline.saturating_duration_since(tokio::time::Instant::now());
                if left.is_zero() {
                    log::info!(
                        "[ssh] post-connect: `{}` never appeared; sending anyway",
                        expect
                    );
                    break;
                }
                match tokio::time::timeout(left, chan.wait()).await {
                    Ok(Some(ChannelMsg::Data { data })) => {
                        on_data(data.as_ref());
                        seen.extend_from_slice(&data);
                    }
                    Ok(Some(ChannelMsg::ExtendedData { data, .. })) => {
                        on_data(data.as_ref());
                        seen.extend_from_slice(&data);
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(step.delay_ms)).await;
        if let Err(e) = chan.data(format!("{}\r", step.send).as_bytes()).await {
            log::warn!("[ssh] post-connect input failed: {}", e);
            break;
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expect_arrow_format_is_parsed() {
        let p = parse(Some("Opt> => 104\n\n确认 => y\n"), Some(500));
        assert_eq!(p.steps.len(), 2);
        assert_eq!(p.steps[0].expect.as_deref(), Some("Opt>"));
        assert_eq!(p.steps[0].send, "104");
        assert_eq!(p.steps[0].delay_ms, 500);
        assert_eq!(p.steps[1].send, "y");
    }

    #[test]
    fn bare_lines_stay_blind_sends() {
        let p = parse(Some("2\n10.0.0.5"), None);
        assert_eq!(p.steps.len(), 2);
        assert!(p.steps.iter().all(|s| s.expect.is_none()));
        assert_eq!(p.steps[0].delay_ms, DEFAULT_DELAY_MS);
    }

    #[test]
    fn json_steps_carry_their_own_timing() {
        let raw = r#"[{"expect":"Opt>","send":"104","delayMs":700},{"send":"y"}]"#;
        let p = parse(Some(raw), None);
        assert_eq!(p.steps.len(), 2);
        assert_eq!(p.steps[0].delay_ms, 700);
        assert_eq!(p.steps[1].send, "y");
        assert_eq!(p.steps[1].timeout_ms, DEFAULT_TIMEOUT_MS);
    }

    #[test]
    fn empty_and_arrowless_answers_are_dropped() {
        assert!(parse(None, None).is_empty());
        assert!(parse(Some("   "), Some(900)).is_empty());
        assert!(parse(Some("Opt> =>"), None).is_empty());
    }
}