// SSH tools for the workspace MCP server.
//
// Two capabilities, deliberately small:
//   * `ssh_profiles_list` — read-only discovery of the saved SSH hosts, so
//     an external agent can resolve "the staging box" to a profileId.
//   * `ssh_exec` — one-shot command execution against one profile.
//
// There is no persistent session between calls: each `ssh_exec` dials,
// authenticates, runs, drains the channel and disconnects. That keeps the
// MCP surface stateless (an agent may fan out calls in parallel) and means
// a stuck command can never leak a half-open terminal into the UI.
//
// Auth is whatever the profile already stores (key / agent / password),
// including jump hosts and ProxyCommand. Profiles that need an interactive
// second factor are the one rough edge: the connect path emits the usual
// `ssh:auth-prompts` event, so the app pops its modal and the human types
// the code — the tool call just waits (bounded by `timeoutSecs`).

use std::time::Duration;

use russh::ChannelMsg;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::modes::ssh::policy::{SshScope, check_command};
use crate::modes::ssh::ssh_session::open_authenticated_ssh_session;
use crate::shared::repos::mcp_grants::{self, KIND_SSH};

/// Cap on how much of a command's output we hand back. Agents read these
/// bytes into their context, so a careless `cat /var/log/syslog` must not
/// be able to flood it.
const MAX_OUTPUT_BYTES: usize = 128 * 1024;

/// `ssh_profiles_list` — saved SSH hosts, minus anything secret.
pub(super) async fn profiles_list(pool: &SqlitePool) -> Result<Value, (i32, String)> {
    let profiles = crate::shared::repos::ssh_profiles::list_all(pool)
        .await
        .map_err(|e| (-32603, format!("DB error: {}", e)))?;
    // Only hosts the user explicitly exposed to MCP, each carrying the scope
    // it was exposed at. A profile with no grant row simply isn't listed.
    let grants = mcp_grants::list_for_kind(pool, KIND_SSH)
        .await
        .map_err(|e| (-32603, format!("DB error: {}", e)))?;

    let mut rows: Vec<Value> = Vec::new();
    for p in profiles {
        let Some(grant) = grants.iter().find(|g| g.enabled && g.resource_id == p.id) else {
            continue;
        };
        let scope = SshScope::parse(&grant.scope);
        rows.push(json!({
            "profileId": p.id,
            "name": p.name,
            "host": p.host,
            "port": p.port,
            "username": p.username,
            "authType": p.auth_type,
            "lastUsedAt": p.last_used_at,
            "jumpProfileId": p.jump_profile_id,
            "proxyCommand": p.proxy_command,
            "scope": scope.as_str(),
            "scopeSummary": scope.summary(),
        }));
    }

    let count = rows.len();
    Ok(json!({
        "profiles": rows,
        "count": count,
        "note": if count == 0 {
            "No SSH connection is exposed to MCP yet — enable one under Settings → MCP."
        } else {
            "Commands are checked against each host's MCP scope before they run."
        },
    }))
}

/// `ssh_exec` — run one command on one saved profile.
pub(super) async fn exec(pool: &SqlitePool, args: &Value) -> Result<Value, (i32, String)> {
    let profile_id = args
        .get("profileId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or((-32602, "profileId required".to_string()))?
        .to_string();
    let command = args
        .get("command")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .ok_or((-32602, "command required".to_string()))?
        .to_string();
    // `isolated` (default) opens its own channel; `shared` types into the
    // user's open terminal so they can watch the command run.
    let shared = args
        .get("mode")
        .and_then(|v| v.as_str())
        .map(|m| m.eq_ignore_ascii_case("shared"))
        .unwrap_or(false);
    // Connect + auth + run share one budget: an MFA prompt can legitimately
    // eat most of it, and a long build should be able to ask for more.
    let timeout_secs = args
        .get("timeoutSecs")
        .and_then(|v| v.as_u64())
        .unwrap_or(60)
        .clamp(5, 600);

    // Exposure + scope are checked before anything touches the network: no
    // grant row means the profile was never shared with MCP.
    let grant = mcp_grants::get(pool, KIND_SSH, &profile_id)
        .await
        .map_err(|e| (-32603, format!("DB error: {}", e)))?
        .filter(|g| g.enabled)
        .ok_or((
            -32603,
            format!(
                "profile `{}` is not exposed to MCP — enable it under Settings → MCP",
                profile_id
            ),
        ))?;
    let scope = SshScope::parse(&grant.scope);
    check_command(scope, &command).map_err(|e| (-32603, e))?;

    crate::telemetry::bump("mcp.ssh_exec");

    if shared {
        return match tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            crate::modes::ssh::live::run_shared(&profile_id, &command, Duration::from_secs(timeout_secs)),
        )
        .await
        {
            Ok(Some(Ok(mut value))) => {
                value["profileId"] = json!(profile_id);
                Ok(value)
            }
            Ok(Some(Err(e))) => Err((-32603, e)),
            Ok(None) => Err((
                -32603,
                format!(
                    "shared mode needs an open SSH terminal for `{}` — open the connection in the app first, or use mode `isolated`",
                    profile_id
                ),
            )),
            Err(_) => Err((
                -32603,
                format!("shared command timed out after {}s", timeout_secs),
            )),
        };
    }

    match tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        run_command(pool, &profile_id, &command, Duration::from_secs(timeout_secs)),
    )
    .await
    {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(e)) => Err((-32603, e)),
        // Dropping the future drops the handle, which closes the connection —
        // we never leave a stray session behind on timeout.
        Err(_) => Err((
            -32603,
            format!(
                "ssh exec timed out after {}s — host unreachable, command still running, \
                 or the profile needs an interactive prompt (MFA); answer it in the app \
                 and retry with a larger timeoutSecs",
                timeout_secs
            ),
        )),
    }
}

/// Run `command` for MCP: on the connection the user already authenticated
/// when one is live (a host that wants a one-time code is only reachable
/// that way), otherwise dial a fresh one, exec, and drop it.
async fn run_command(
    pool: &SqlitePool,
    profile_id: &str,
    command: &str,
    budget: Duration,
) -> Result<Value, String> {
    if let Some(result) = crate::modes::ssh::live::run(profile_id, command, budget).await {
        // A host that only allows one channel (bastion) answers the reused
        // connection with nothing at all. Fall through to a fresh dial in
        // that case — with an enrolled TOTP seed it can authenticate on its
        // own, which is exactly why the seed is worth storing.
        let empty = match &result {
            Ok(value) => {
                value["stdout"].as_str().unwrap_or("").is_empty()
                    && value["stderr"].as_str().unwrap_or("").is_empty()
            }
            Err(_) => false,
        };
        if !empty {
            return result.map(|mut value| {
                value["profileId"] = json!(profile_id);
                value["via"] = json!("live-session");
                value
            });
        }
        log::info!(
            "[mcp] live session returned nothing for {}; retrying on a fresh connection",
            profile_id
        );
    }

    // `false`: an MCP call has no human to answer an OTP prompt, so the
    // connect fails fast with the prompt names instead of stalling.
    let handle = open_authenticated_ssh_session(pool, profile_id, false).await?;
    let post = crate::modes::ssh::post_connect::load(pool, profile_id).await;
    let result = exec_on_handle(&handle, command, Some(&post)).await;
    // Best effort: the server closes on its own if this fails.
    let _ = handle
        .disconnect(russh::Disconnect::ByApplication, "", "en")
        .await;
    result.map(|mut value| {
        value["profileId"] = json!(profile_id);
        value["via"] = json!("new-connection");
        value
    })
}

/// Exec one command on an already-authenticated session and collect its
/// output. Split out from `run_command` so tests can drive it against a
/// fixture without a profile row.
pub(crate) async fn exec_on_handle(
    handle: &russh::client::Handle<crate::modes::ssh::ssh_session::ClientHandler>,
    command: &str,
    post: Option<&crate::modes::ssh::post_connect::PostConnect>,
) -> Result<Value, String> {
    // A profile with a bastion menu script is a host that only ever gives a
    // shell: an `exec` channel there is either a silent no-op or consumes
    // the single channel the bastion allows. Go straight to the shell.
    if let Some(post) = post.filter(|p| !p.is_empty()) {
        return match exec_via_shell(handle, command, Some(post)).await {
            Some(result) => result,
            None => Err(
                "could not open a shell channel — check the After-connect lines".to_string(),
            ),
        };
    }

    let mut channel = handle
        .channel_open_session()
        .await
        .map_err(|e| format!("open channel: {}", e))?;
    channel
        .exec(true, command)
        .await
        .map_err(|e| format!("exec request: {}", e))?;

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let mut exit_code: Option<u32> = None;
    let mut truncated = false;

    while let Some(msg) = channel.wait().await {
        match msg {
            ChannelMsg::Data { data } => {
                push_capped(&mut stdout, &data, &mut truncated);
            }
            // ext == 1 is SSH_EXTENDED_DATA_STDERR (RFC 4254 §5.2).
            ChannelMsg::ExtendedData { data, ext } if ext == 1 => {
                push_capped(&mut stderr, &data, &mut truncated);
            }
            ChannelMsg::ExitStatus { exit_status } => exit_code = Some(exit_status),
            ChannelMsg::Eof | ChannelMsg::Close => break,
            _ => {}
        }
    }

    // A bastion can acknowledge an `exec` request locally and close the
    // channel without ever running the command — JumpServer's koko does
    // exactly that: exit 0, no output, ~0ms. Fall back to a real shell on a
    // second channel (the user's terminal channel is untouched).
    if exit_code.unwrap_or(0) == 0 && stdout.is_empty() && stderr.is_empty() {
        if let Some(result) = exec_via_shell(handle, command, post).await {
            return result;
        }
    }

    let empty = stdout.is_empty() && stderr.is_empty();
    let mut value = json!({
        "command": command,
        "stdout": String::from_utf8_lossy(&stdout),
        "stderr": String::from_utf8_lossy(&stderr),
        "exitCode": exit_code,
        "truncated": truncated,
        "mode": "exec",
    });
    if empty {
        // No output from either transport usually means the host only lets
        // one channel live (a bastion). Point the caller at shared mode.
        value["note"] = json!(
            "host produced no output for this command — if it sits behind a bastion, retry with mode \"shared\" so the command runs in your open SSH terminal"
        );
    }
    Ok(value)
}

/// Run the command inside a dedicated shell channel instead of an exec
/// channel: request a PTY, get a shell, feed a sentinel-wrapped line, read
/// until the closing sentinel. Works on hosts whose bastion implements
/// `shell` but not `exec`, and keeps MCP output out of the user's terminal.
async fn exec_via_shell(
    handle: &russh::client::Handle<crate::modes::ssh::ssh_session::ClientHandler>,
    command: &str,
    post: Option<&crate::modes::ssh::post_connect::PostConnect>,
) -> Option<Result<Value, String>> {
    let mut channel = handle.channel_open_session().await.ok()?;
    channel
        .request_pty(false, "xterm-256color", 200, 50, 0, 0, &[])
        .await
        .ok()?;
    channel.request_shell(false).await.ok()?;

    // Answer a bastion's asset menu first; otherwise the sentinel lands in
    // the menu and nothing runs. Steps wait for their prompt text.
    if let Some(post) = post.filter(|p| !p.is_empty()) {
        crate::modes::ssh::post_connect::run_steps(&mut channel, &post.steps, |_| {}).await;
        // Selecting an asset opens a second connection underneath: wait for
        // the host prompt to stop moving before typing the command.
        crate::modes::ssh::post_connect::wait_for_quiet(&mut channel, 600, 25_000, |_| {}).await;
    }

    let tag = uuid::Uuid::new_v4().simple().to_string();
    let begin = format!("__ZA_B_{}__", tag);
    let end = format!("__ZA_E_{}__", tag);
    // `$?` is captured as the LAST thing before the end marker; the leading
    // newline keeps a prompt from being glued onto the begin marker.
    let script = format!(
        "printf '\n{}\n'; {}\nprintf '\n{} %s\n' \"$?\"\n",
        begin, command, end
    );
    channel.data(script.as_bytes()).await.ok()?;

    let mut raw: Vec<u8> = Vec::new();
    let mut truncated = false;
    // Bounded: a host that never reaches the sentinel (bad menu answer, extra
    // prompt) must come back with its transcript instead of pinning the
    // caller until the outer MCP timeout erases every clue.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(25);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            break;
        }
        let Some(msg) = tokio::time::timeout(left, channel.wait())
            .await
            .ok()
            .flatten()
        else {
            break;
        };
        match msg {
            ChannelMsg::Data { data } => {
                push_capped(&mut raw, &data, &mut truncated);
                if raw
                    .windows(end.len())
                    .any(|w| w == end.as_bytes())
                {
                    // Give the trailing exit code a moment to arrive.
                    let mut tail_deadline = tokio::time::Instant::now()
                        + std::time::Duration::from_millis(250);
                    while tokio::time::Instant::now() < tail_deadline {
                        match tokio::time::timeout(
                            std::time::Duration::from_millis(50),
                            channel.wait(),
                        )
                        .await
                        {
                            Ok(Some(ChannelMsg::Data { data })) => {
                                push_capped(&mut raw, &data, &mut truncated)
                            }
                            Ok(Some(_)) => {}
                            Ok(None) => break,
                            Err(_) => {}
                        }
                    }
                    break;
                }
            }
            ChannelMsg::ExtendedData { data, ext } if ext == 1 => {
                push_capped(&mut raw, &data, &mut truncated)
            }
            ChannelMsg::Eof | ChannelMsg::Close => break,
            _ => {}
        }
    }
    let _ = channel.close().await;

    let text = String::from_utf8_lossy(&raw).replace("\r\n", "\n");
    let Some((body, exit_code)) = split_sentinel_output(&text, &begin, &end) else {
        // The command never reached a shell — most often the host is sitting
        // in a bastion menu. Hand back what it *did* say so the caller can
        // configure an `expect` for it instead of guessing.
        let tail: String = text
            .chars()
            .rev()
            .take(600)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        return Some(Err(format!(
            "the command never reached a shell (no output markers). Host said:\n{}",
            tail.trim()
        )));
    };
    Some(Ok(json!({
        "command": command,
        "stdout": body,
        "stderr": "",
        "exitCode": exit_code,
        "truncated": truncated,
        "mode": "shell",
    })))
}


/// Strip what a PTY adds around a captured payload: the echoed
/// end-marker `printf` (which contains the marker text), ANSI/OSC title
/// sequences, and trailing prompt-only lines.
fn clean_shell_body(body: &str, end: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in body.trim_end_matches('\n').split('\n') {
        if line.contains(end) {
            break;
        }
        lines.push(strip_ansi(line));
    }
    while let Some(last) = lines.last() {
        let t = last.trim_end();
        // Drop a trailing prompt, and a trailing prompt *plus the start of our
        // own echoed command* — on a narrow PTY that echo can wrap, leaving
        // `[root@host ~]# printf '` as its own line.
        let prompt_with_echo = ["]# ", "]$ ", "] $ ", "] # "]
            .iter()
            .any(|p| t.contains(p));
        if t.is_empty()
            || t.ends_with('#')
            || t.ends_with('$')
            || t.ends_with('%')
            || prompt_with_echo
        {
            lines.pop();
        } else {
            break;
        }
    }
    lines.join("\n")
}

/// Drop CSI (`ESC [ … final`) and OSC (`ESC ] … BEL|ST`) sequences.
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                // CSI: parameters then a final byte in @..~
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Pull the payload out of a sentinel-wrapped shell transcript. Returns the
/// text between the markers and the exit code printed after the end marker.
/// Everything before the first begin marker (banner, prompt, the echoed
/// command line) is discarded.
pub(crate) fn split_sentinel_output(text: &str, begin: &str, end: &str) -> Option<(String, u32)> {
    // The PTY echoes the command we sent, and that echo *contains* both
    // marker literals (inside `printf '\n…\n'`). Require a real newline on
    // both sides of each marker so only the emitted lines match, never the
    // echoed script.
    let text = text.replace("\r\n", "\n");
    let open = format!("\n{}\n", begin);
    let start = text.find(&open)? + open.len();
    let rest = &text[start..];
    let close = format!("\n{} ", end);
    let stop = rest.find(&close)?;
    // The newline that separates the payload from the end marker is ours.
    let body = clean_shell_body(&rest[..stop], end);
    let tail = &rest[stop + close.len()..];
    let code = tail
        .split_whitespace()
        .next()
        .and_then(|c| c.parse::<u32>().ok())
        .unwrap_or(0);
    Some((body, code))
}

/// Append up to the cap, remembering whether anything was dropped. We keep
/// draining the channel after hitting the cap so the remote process is not
/// left blocked on a full window.
pub(crate) fn push_capped(buf: &mut Vec<u8>, data: &[u8], truncated: &mut bool) {
    let room = MAX_OUTPUT_BYTES.saturating_sub(buf.len());
    if room == 0 {
        *truncated = true;
        return;
    }
    if data.len() > room {
        buf.extend_from_slice(&data[..room]);
        *truncated = true;
    } else {
        buf.extend_from_slice(data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentinel_output_is_extracted_and_exit_code_read() {
        // A real transcript: prompt + echoed command before the begin marker,
        // CRLF line endings, and the exit code after the end marker.
        let text = "\r\n$ printf '\\n__B__\\n'; uname -a\r\n__B__\r\nLinux box 6.1\r\nalpine\r\n__E__ 0\r\n$ ";
        let (body, code) = split_sentinel_output(text, "__B__", "__E__").expect("markers present");
        assert_eq!(body, "Linux box 6.1\nalpine");
        assert_eq!(code, 0);
    }

    #[test]
    fn sentinel_output_reports_nonzero_and_missing_markers() {
        let text = "\r\n__B__\r\ncat: nope: No such file\r\n__E__ 1\r\n$ ";
        let (body, code) = split_sentinel_output(text, "__B__", "__E__").expect("markers present");
        assert!(body.contains("No such file"), "{}", body);
        assert_eq!(code, 1);
        assert!(split_sentinel_output("command not found", "__B__", "__E__").is_none());
    }

    #[test]
    fn output_is_capped_and_flagged() {
        let mut buf = Vec::new();
        let mut truncated = false;
        push_capped(&mut buf, &vec![b'a'; MAX_OUTPUT_BYTES + 10], &mut truncated);
        assert_eq!(buf.len(), MAX_OUTPUT_BYTES);
        assert!(truncated);
    }

    #[test]
    fn small_output_passes_through_untouched() {
        let mut buf = Vec::new();
        let mut truncated = false;
        push_capped(&mut buf, b"ok\n", &mut truncated);
        assert_eq!(buf, b"ok\n");
        assert!(!truncated);
    }
}

#[cfg(test)]
mod cleanup_tests {
    use super::{clean_shell_body, strip_ansi};

    #[test]
    fn pty_noise_around_the_payload_is_removed() {
        // Shape of a real capture: output, then the echoed end-marker
        // printf, then the prompt's OSC title sequence.
        let body = "Linux box 6.1\n\x1b]0;root@box:~\\\x07[root@box ~]# printf '\\\\n__E__ %s\\\\n' \"$?\"\n[root@box ~]# ";
        assert_eq!(clean_shell_body(body, "__E__"), "Linux box 6.1");
    }

    #[test]
    fn wrapped_echo_of_our_own_command_is_dropped() {
        // Real shape from a JumpServer shell on a narrow PTY: the echoed
        // end-marker printf wraps, so its first half is its own line.
        let body = "lute-dev-04\nLinux box 6.1\n[root@lute-dev-04 ~]# printf '";
        assert_eq!(clean_shell_body(body, "__E__"), "lute-dev-04\nLinux box 6.1");
    }

    #[test]
    fn ansi_colours_and_multiline_output_survive() {
        let body = "\x1b[32mok\x1b[0m\nline2\nline3\n";
        assert_eq!(clean_shell_body(body, "__E__"), "ok\nline2\nline3");
    }

    #[test]
    fn strip_ansi_handles_csi_and_osc() {
        assert_eq!(strip_ansi("a\x1b[1;31mb\x1b[0mc"), "abc");
        assert_eq!(strip_ansi("x\x1b]0;title\x07y"), "xy");
    }
}
