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
        return result.map(|mut value| {
            value["profileId"] = json!(profile_id);
            value["via"] = json!("live-session");
            value
        });
    }

    // `false`: an MCP call has no human to answer an OTP prompt, so the
    // connect fails fast with the prompt names instead of stalling.
    let handle = open_authenticated_ssh_session(pool, profile_id, false).await?;
    let result = exec_on_handle(&handle, command).await;
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
) -> Result<Value, String> {
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
        if let Some(result) = exec_via_shell(handle, command).await {
            return Ok(result);
        }
    }

    Ok(json!({
        "command": command,
        "stdout": String::from_utf8_lossy(&stdout),
        "stderr": String::from_utf8_lossy(&stderr),
        "exitCode": exit_code,
        "truncated": truncated,
        "mode": "exec",
    }))
}

/// Run the command inside a dedicated shell channel instead of an exec
/// channel: request a PTY, get a shell, feed a sentinel-wrapped line, read
/// until the closing sentinel. Works on hosts whose bastion implements
/// `shell` but not `exec`, and keeps MCP output out of the user's terminal.
async fn exec_via_shell(
    handle: &russh::client::Handle<crate::modes::ssh::ssh_session::ClientHandler>,
    command: &str,
) -> Option<Value> {
    let mut channel = handle.channel_open_session().await.ok()?;
    channel
        .request_pty(false, "xterm-256color", 200, 50, 0, 0, &[])
        .await
        .ok()?;
    channel.request_shell(false).await.ok()?;

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
    while let Some(msg) = channel.wait().await {
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
    let (body, exit_code) = split_sentinel_output(&text, &begin, &end)?;
    Some(json!({
        "command": command,
        "stdout": body,
        "stderr": "",
        "exitCode": exit_code,
        "truncated": truncated,
        "mode": "shell",
    }))
}

/// Pull the payload out of a sentinel-wrapped shell transcript. Returns the
/// text between the markers and the exit code printed after the end marker.
/// Everything before the first begin marker (banner, prompt, the echoed
/// command line) is discarded.
fn split_sentinel_output(text: &str, begin: &str, end: &str) -> Option<(String, u32)> {
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
    let body = rest[..stop].to_string();
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
fn push_capped(buf: &mut Vec<u8>, data: &[u8], truncated: &mut bool) {
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
