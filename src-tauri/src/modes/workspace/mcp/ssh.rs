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
use serde_json::{json, Value};
use sqlx::SqlitePool;

use crate::modes::ssh::ssh_session::open_authenticated_ssh_session;

/// Cap on how much of a command's output we hand back. Agents read these
/// bytes into their context, so a careless `cat /var/log/syslog` must not
/// be able to flood it.
const MAX_OUTPUT_BYTES: usize = 128 * 1024;

/// `ssh_profiles_list` — saved SSH hosts, minus anything secret.
pub(super) async fn profiles_list(pool: &SqlitePool) -> Result<Value, (i32, String)> {
    let profiles = crate::shared::repos::ssh_profiles::list_all(pool)
        .await
        .map_err(|e| (-32603, format!("DB error: {}", e)))?;

    let count = profiles.len();
    let rows: Vec<Value> = profiles
        .into_iter()
        .map(|p| {
            json!({
                "profileId": p.id,
                // Camel-cased for the same reason `name` is: the agent
                // prompt / docs speak in camelCase everywhere else.
                "name": p.name,
                "host": p.host,
                "port": p.port,
                "username": p.username,
                "authType": p.auth_type,
                "lastUsedAt": p.last_used_at,
                "jumpProfileId": p.jump_profile_id,
                "proxyCommand": p.proxy_command,
            })
        })
        .collect();

    Ok(json!({ "profiles": rows, "count": count }))
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

    crate::telemetry::bump("mcp.ssh_exec");

    match tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        run_command(pool, &profile_id, &command),
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

/// Dial, exec, drop the connection.
async fn run_command(
    pool: &SqlitePool,
    profile_id: &str,
    command: &str,
) -> Result<Value, String> {
    let handle = open_authenticated_ssh_session(pool, profile_id).await?;
    let result = exec_on_handle(&handle, command).await;
    // Best effort: the server closes on its own if this fails.
    let _ = handle
        .disconnect(russh::Disconnect::ByApplication, "", "en")
        .await;
    result.map(|mut value| {
        value["profileId"] = json!(profile_id);
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

    Ok(json!({
        "command": command,
        "stdout": String::from_utf8_lossy(&stdout),
        "stderr": String::from_utf8_lossy(&stderr),
        "exitCode": exit_code,
        "truncated": truncated,
    }))
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