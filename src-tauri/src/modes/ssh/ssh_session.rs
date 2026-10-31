//! Shared SSH connect-and-authenticate flow.
//!
//! Returns a post-auth `Handle` that callers can layer their channel type
//! on top of:
//!  - terminal mode: `request_pty` + `request_shell`
//!  - SFTP backend: `channel_open_session` + `request_subsystem("sftp")`
//!  - tunnel: `channel_open_direct_tcpip(...)`
//!
//! Architecture: the connect path branches on the profile's proxy config
//! when building the underlying transport stream, then funnels through
//! one shared SSH handshake + auth function. All three transport sources
//! produce a `Box<dyn AsyncRead+AsyncWrite+Send+Unpin>` that russh's
//! `client::connect_stream` can speak SSH over:
//!
//!   - direct TCP            → `TcpStream`
//!   - ProxyCommand          → subprocess argv spawn, stdin/stdout duplex
//!   - ProxyJump (Phase 3)   → `direct-tcpip` channel from a jump session
//!
//! The handshake-and-auth tail is identical across all three, so adding
//! a fourth transport source later is just a new arm in `build_transport`.

use russh::client::{self, Handle};
use russh::ChannelMsg;
use serde::Serialize;
use sqlx::SqlitePool;
use std::collections::HashSet;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::sync::oneshot;

use crate::modes::ssh::agent::try_agent_auth;
use crate::modes::ssh::models::{PendingAuthPrompts, SshProfile};
use crate::shared::platform::credential_store::{credential_store, CredentialStore};

/// Process-global AppHandle, set once at app startup. Only the
/// `keyboard-interactive` auth path needs this — to emit events to the
/// frontend asking the user for prompt answers, and to read the shared
/// `PendingAuthPrompts` state for response delivery. Other auth paths
/// don't touch it. Stored as `OnceLock` so callers (terminal.rs,
/// tunnel.rs, explorer session.rs) don't have to be updated to thread
/// AppHandle through every chain.
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

/// Called once from `lib.rs` setup() after the app is built.
pub fn set_app_handle(handle: AppHandle) {
    let _ = APP_HANDLE.set(handle);
}

fn app_handle() -> Option<&'static AppHandle> {
    APP_HANDLE.get()
}

/// russh client handler — accepts any host key (TOFU phase 1; see the
/// SSH-mode design doc for the planned known-hosts verification).
pub struct ClientHandler;

#[async_trait::async_trait]
impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh_keys::key::PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

/// Everything needed to dial + authenticate one SSH session. Built either
/// from a stored `ssh_profiles` row or from a "direct credentials" form
/// (Explorer SFTP).
pub struct SshAuthSpec {
    pub host: String,
    pub port: u16,
    pub username: String,
    /// "key" | "password" | "agent".
    pub auth_type: String,
    pub key_path: Option<String>,
    /// Password (auth_type=password) or passphrase (auth_type=key). None
    /// is fine for unencrypted keys / agent auth.
    pub secret: Option<String>,
}

/// Trait alias for "anything russh can speak SSH over". Concrete types:
/// `TcpStream` for direct connects, `ProxyCommandStream` for spawned
/// subprocesses, and (Phase 3) a russh channel wrapper for ProxyJump.
pub(crate) trait Transport: AsyncRead + AsyncWrite + Send + Unpin + 'static {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin + 'static> Transport for T {}

type BoxedTransport = Box<dyn Transport>;

/// Connect to the host described by the SSH profile, perform the chosen
/// auth method, and return the post-auth `Handle`. Touches the profile's
/// `last_used_at` (best-effort — won't fail the connect on bookkeeping
/// errors).
///
/// Walks the ProxyJump chain (each profile's `jump_profile_id` pointer)
/// from the destination back to the outermost host, then connects forward
/// through each link. ProxyCommand on any profile takes precedence over
/// ProxyJump on the same profile (matches OpenSSH).
pub async fn open_authenticated_ssh_session(
    pool: &SqlitePool,
    profile_id: &str,
) -> Result<Handle<ClientHandler>, String> {
    // 1. Walk the jump chain. Start at the destination, follow
    // jump_profile_id pointers until we hit a profile with no jump set.
    // Cycle protection via visited HashSet so a misconfigured A→B→A loop
    // fails fast instead of recursing forever.
    let mut chain: Vec<SshProfile> = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut current_id = profile_id.to_string();
    loop {
        if !visited.insert(current_id.clone()) {
            return Err(format!(
                "ssh proxy chain cycle detected (profile {} appears twice)",
                current_id
            ));
        }
        let profile: SshProfile =
            sqlx::query_as::<_, SshProfile>("SELECT * FROM ssh_profiles WHERE id = ?")
                .bind(&current_id)
                .fetch_one(pool)
                .await
                .map_err(|e| format!("ssh profile lookup ({}): {}", current_id, e))?;
        // Empty strings are stored alongside NULL by the UI clear path; treat
        // both as "no proxy" so the connect logic doesn't try to follow a
        // pointer to "" or run an empty ProxyCommand.
        let next_jump = profile
            .jump_profile_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let has_proxy_command = profile
            .proxy_command
            .as_deref()
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        chain.push(profile);
        match next_jump {
            // ProxyCommand on the current profile wins over its jump pointer
            // (per ssh_config(5) precedence). Stop walking; ProxyCommand will
            // build the transport for this profile when we connect forward.
            Some(j) if !has_proxy_command => {
                current_id = j;
            }
            _ => break,
        }
    }
    // chain is currently [destination, jump1, jump2, …, outermost].
    // We want to connect outermost → … → destination, so reverse.
    chain.reverse();

    // 2. Connect forward through the chain. Each iteration's `prev_handle`
    // (when present) is the SSH session through which we open the next
    // hop's transport. The first hop uses ProxyCommand or direct TCP.
    let mut prev_handle: Option<Handle<ClientHandler>> = None;
    for (i, profile) in chain.iter().enumerate() {
        // Bump last_used_at on every profile in the chain (best-effort).
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let _ = sqlx::query("UPDATE ssh_profiles SET last_used_at = ? WHERE id = ?")
            .bind(&now)
            .bind(&profile.id)
            .execute(pool)
            .await;

        // Pull credential for this hop.
        let secret: Option<String> = credential_store()
            .get(&profile.id)
            .await
            .map_err(|e| format!("credential lookup ({}): {}", profile.name, e))?;

        // Build transport for this hop. Filter empty proxy_command (UI's
        // clear sentinel) so we don't try to spawn an empty argv.
        let proxy_cmd = profile.proxy_command.as_deref().filter(|s| !s.is_empty());
        let transport: BoxedTransport = if i == 0 {
            // Outermost host: ProxyCommand if set, otherwise direct TCP.
            // (Outermost can't be ProxyJump-only by definition — chain walk
            // already followed any jump pointers to reach this profile.)
            if let Some(cmd) = proxy_cmd {
                spawn_proxy_command_stream(cmd, &profile.host, profile.port, &profile.username)
                    .await?
            } else {
                let tcp =
                    tokio::net::TcpStream::connect((profile.host.as_str(), profile.port as u16))
                        .await
                        .map_err(|e| format!("tcp connect ({}): {}", profile.name, e))?;
                if let Err(e) = tcp.set_nodelay(true) {
                    log::warn!("[ssh] warning: TCP_NODELAY not set ({})", e);
                }
                Box::new(tcp)
            }
        } else {
            // Inner hop: tunnel a `direct-tcpip` channel through the
            // previous handle to this hop's host:port. ProxyCommand on
            // an inner hop bypasses the jump (matches OpenSSH precedence).
            if let Some(cmd) = proxy_cmd {
                spawn_proxy_command_stream(cmd, &profile.host, profile.port, &profile.username)
                    .await?
            } else {
                let prev = prev_handle.take().expect("inner hop without prev_handle");
                open_jump_channel_stream(prev, &profile.host, profile.port as u16).await?
            }
        };

        let spec = SshAuthSpec {
            host: profile.host.clone(),
            port: profile.port as u16,
            username: profile.username.clone(),
            auth_type: profile.auth_type.clone(),
            key_path: profile.key_path.clone(),
            secret,
        };
        prev_handle = Some(perform_ssh_handshake_and_auth(transport, &spec).await?);
    }

    prev_handle.ok_or_else(|| "ssh proxy chain resolved to empty list".to_string())
}

/// Same as `open_authenticated_ssh_session`, but takes a pre-built spec
/// rather than loading from `ssh_profiles`. Used by Explorer SFTP when
/// the user picks "New connection details" (no SSH profile). Direct TCP
/// only — proxy support requires a profile (so the proxy config has a
/// place to live).
pub async fn open_authenticated_ssh_session_with_spec(
    spec: SshAuthSpec,
) -> Result<Handle<ClientHandler>, String> {
    // TCP socket. Manual creation so we can disable Nagle's algorithm —
    // critical for interactive SSH (40-200ms keystroke latency without it).
    let tcp = tokio::net::TcpStream::connect((spec.host.as_str(), spec.port))
        .await
        .map_err(|e| format!("tcp connect: {}", e))?;
    if let Err(e) = tcp.set_nodelay(true) {
        log::warn!("[ssh] warning: TCP_NODELAY not set ({})", e);
    }
    perform_ssh_handshake_and_auth(Box::new(tcp), &spec).await
}

/// Run the SSH handshake and authentication on the given transport
/// stream. Shared by the direct, ProxyCommand, and ProxyJump paths so
/// auth code stays in one place.
async fn perform_ssh_handshake_and_auth(
    transport: BoxedTransport,
    spec: &SshAuthSpec,
) -> Result<Handle<ClientHandler>, String> {
    // SSH handshake with a hard 15s timeout. ProxyCommand subprocesses can
    // hang if the underlying tunnel never opens — we don't want SSH connect
    // attempts to wait forever in that case.
    //
    // Server keepalive: russh's default Config has none, so a host that
    // dies silently (network partition, instance terminated, laptop sleep
    // on the other side) leaves the channel in a half-open state forever
    // and the frontend dot stays green indefinitely. With these settings
    // the client sends a keepalive every 30s and tears the channel down
    // after 3 missed replies — dead hosts surface within ~90s as an
    // ordinary exit, which markExited() on the frontend converts to a
    // 'disconnected' state (no more stale green pulse).
    let config = Arc::new(client::Config {
        keepalive_interval: Some(std::time::Duration::from_secs(30)),
        keepalive_max: 3,
        ..client::Config::default()
    });
    let connect_fut = client::connect_stream(config, transport, ClientHandler);
    let mut handle: Handle<ClientHandler> = match tokio::time::timeout(
        std::time::Duration::from_secs(15),
        connect_fut,
    )
    .await
    {
        Ok(Ok(h)) => h,
        Ok(Err(e)) => return Err(format!("ssh connect: {}", e)),
        Err(_) => {
            crate::telemetry::bump("err.ssh_timeout");
            return Err("ssh connect: timed out after 15s".to_string());
        }
    };

    // Auth phase has its own timeout — separate from the handshake timeout
    // above. If the server closes the connection mid-negotiation (some
    // hardened sshd configs do this after a single rejected method),
    // russh's underlying recv can wait forever. 30s covers slow PAM stacks
    // (e.g. LDAP-backed auth doing a network round-trip) without burning
    // the user's afternoon on a stuck UI.
    let authed = match tokio::time::timeout(
        std::time::Duration::from_secs(30),
        run_auth(&mut handle, spec),
    )
    .await
    {
        Ok(Ok(b)) => b,
        Ok(Err(e)) => return Err(e),
        Err(_) => {
            crate::telemetry::bump("err.ssh_timeout");
            return Err("ssh auth: timed out after 30s".to_string());
        }
    };
    if !authed {
        return Err("authentication failed".to_string());
    }

    crate::telemetry::bump("ssh.connect");
    Ok(handle)
}

/// Auth dispatch — split out of `perform_ssh_handshake_and_auth` so it can
/// be wrapped in a timeout cleanly.
async fn run_auth(
    handle: &mut russh::client::Handle<ClientHandler>,
    spec: &SshAuthSpec,
) -> Result<bool, String> {
    Ok(match spec.auth_type.as_str() {
        "key" => {
            let key_path = spec
                .key_path
                .as_ref()
                .ok_or_else(|| "key auth requires key_path".to_string())?;
            let passphrase = spec.secret.as_deref();
            let keypair = russh_keys::load_secret_key(key_path, passphrase)
                .map_err(|e| format!("load key: {}", e))?;
            let primary = handle
                .authenticate_publickey(&spec.username, Arc::new(keypair))
                .await
                .map_err(|e| format!("ssh auth publickey: {}", e))?;
            if primary {
                true
            } else {
                // Key + OTP hosts: the key satisfies the first factor and the
                // server answers by offering `keyboard-interactive` for the
                // second. Without this fallback such hosts just reported
                // "authentication failed" and the user never saw a code field.
                try_keyboard_interactive_fallback(handle, &spec.username, None).await
            }
        }
        "password" => {
            // Three cases:
            //   1. Stored password present → try plain `password` method
            //      first (fast path for traditional sshd), fall back to
            //      keyboard-interactive on rejection (the stored password
            //      pre-fills the first prompt).
            //   2. Stored password present + traditional server → done in
            //      step 1, no modal.
            //   3. No stored password → skip the plain-password attempt
            //      entirely (it can't possibly succeed) and go straight
            //      to keyboard-interactive. The modal asks the user for
            //      the password as the first prompt — same UX as ssh,
            //      VS Code, Zed, etc.
            match spec.secret.clone() {
                Some(password) => {
                    match handle
                        .authenticate_password(&spec.username, password.clone())
                        .await
                    {
                        Ok(true) => true,
                        // Rejected, or the server refuses the `password` method
                        // outright (PAM-only hosts answer with
                        // keyboard-interactive): drive the prompts so the user
                        // can enter password and OTP.
                        Ok(false) => {
                            try_keyboard_interactive_with_ui(
                                handle,
                                &spec.username,
                                Some(&password),
                            )
                            .await?
                        }
                        Err(error) => {
                            log::debug!(
                                target: "ssh::auth",
                                "password method rejected ({}), trying keyboard-interactive",
                                error
                            );
                            try_keyboard_interactive_fallback(
                                handle,
                                &spec.username,
                                Some(&password),
                            )
                            .await
                        }
                    }
                }
                None => {
                    try_keyboard_interactive_with_ui(handle, &spec.username, None).await?
                }
            }
        }
        "agent" => {
            if try_agent_auth(handle, &spec.username).await? {
                true
            } else {
                // Agent + OTP: same story as key auth — the agent proves the
                // first factor, keyboard-interactive carries the code.
                try_keyboard_interactive_fallback(handle, &spec.username, None).await
            }
        }
        "interactive" => {
            // Keyboard-interactive PAM auth driven by frontend prompts.
            // Used for servers like password+OTP setups where the SSH
            // protocol carries multiple sequential prompts and we can't
            // pre-store all the answers. The connect path emits a Tauri
            // event per prompt round; the frontend modal collects answers
            // and submits them via `ssh_submit_auth_prompts`.
            //
            // Pre-fill behavior: if the profile has a stored secret AND
            // the FIRST prompt round contains exactly one prompt that
            // looks like a password (no echo + "password" in the prompt
            // text), we pre-fill that one prompt with the stored secret
            // and only show the modal for subsequent rounds. Saves one
            // typing step for the most common 2-step setup.
            try_keyboard_interactive_with_ui(handle, &spec.username, spec.secret.as_deref()).await?
        }
        other => return Err(format!("unknown auth_type: {}", other)),
    })
}

// ────────────────────────────────────────────────────────────────────────
// Keyboard-interactive (PAM) auth with frontend-driven prompts
// ────────────────────────────────────────────────────────────────────────

/// Payload sent to the frontend when the SSH server requests a prompt
/// round. Each `request_id` is unique per round so the frontend can route
/// the response back via `ssh_submit_auth_prompts`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthPromptsPayload {
    request_id: String,
    name: String,
    instructions: String,
    /// Each prompt: (text, echo). `echo=false` → password-style input.
    prompts: Vec<AuthPrompt>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthPrompt {
    prompt: String,
    echo: bool,
}

/// Second-factor fallback for hosts that require MFA: a successful key /
/// agent / password step is answered by the server with a
/// `keyboard-interactive` round for the one-time code. Errors here mean the
/// server does not offer keyboard-interactive at all, which is not a new
/// failure — the caller keeps its original result (plain "auth failed").
async fn try_keyboard_interactive_fallback(
    handle: &mut russh::client::Handle<ClientHandler>,
    username: &str,
    pre_stored_secret: Option<&str>,
) -> bool {
    match try_keyboard_interactive_with_ui(handle, username, pre_stored_secret).await {
        Ok(ok) => ok,
        Err(error) => {
            log::debug!(
                target: "ssh::auth",
                "keyboard-interactive fallback unavailable: {}",
                error
            );
            false
        }
    }
}

/// Drive a keyboard-interactive auth flow. Each `InfoRequest` round emits
/// a `ssh:auth-prompts` Tauri event and parks on a oneshot for the user
/// to submit answers via the matching frontend modal. Returns true on
/// auth success, false on failure / user cancellation / timeout.
async fn try_keyboard_interactive_with_ui(
    handle: &mut russh::client::Handle<ClientHandler>,
    username: &str,
    pre_stored_secret: Option<&str>,
) -> Result<bool, String> {
    use russh::client::KeyboardInteractiveAuthResponse;

    let app = app_handle().ok_or_else(|| {
        "keyboard-interactive auth: AppHandle not available (Tauri startup incomplete)"
            .to_string()
    })?;
    let pending: tauri::State<'_, PendingAuthPrompts> = app.state();

    let mut response = handle
        .authenticate_keyboard_interactive_start(username.to_string(), None)
        .await
        .map_err(|e| format!("ssh auth keyboard-interactive: {}", e))?;

    let mut round_index: usize = 0;
    // Cap rounds defensively. Real PAM stacks rarely exceed 3 InfoRequests;
    // 8 lines up with OpenSSH defaults.
    for _ in 0..8 {
        match response {
            KeyboardInteractiveAuthResponse::Success => return Ok(true),
            KeyboardInteractiveAuthResponse::Failure => return Ok(false),
            KeyboardInteractiveAuthResponse::InfoRequest {
                name,
                instructions,
                prompts,
            } => {
                // First-round single-password-prompt heuristic: pre-fill from
                // stored secret without showing the modal. Detection: prompt
                // is non-echoing AND prompt text mentions "password" (case-
                // insensitive). Avoids surprising the user on a normal
                // password-only server while still supporting password+OTP.
                let is_first_password_round = round_index == 0
                    && prompts.len() == 1
                    && !prompts[0].echo
                    && prompts[0].prompt.to_lowercase().contains("password");

                let answers: Vec<String> = if prompts.is_empty() {
                    // Info-only round: PAM stacks sometimes send these
                    // between logical rounds (carrying `name` /
                    // `instructions` text but no prompts to answer).
                    // Acknowledge with empty answers — never show a modal
                    // for these, since the user has nothing to type.
                    Vec::new()
                } else if is_first_password_round && pre_stored_secret.is_some() {
                    vec![pre_stored_secret.unwrap().to_string()]
                } else {
                    request_prompts_from_user(app, &pending, &name, &instructions, &prompts)
                        .await?
                };

                response = handle
                    .authenticate_keyboard_interactive_respond(answers)
                    .await
                    .map_err(|e| {
                        format!("ssh auth keyboard-interactive respond: {}", e)
                    })?;
                round_index += 1;
            }
        }
    }
    Ok(false)
}

/// Emit a `ssh:auth-prompts` event with a fresh request_id and park on a
/// oneshot until the frontend responds via `ssh_submit_auth_prompts`. Per-
/// round timeout is 2 minutes (covers OTP entry from a separate device).
async fn request_prompts_from_user(
    app: &AppHandle,
    pending: &PendingAuthPrompts,
    name: &str,
    instructions: &str,
    prompts: &[russh::client::Prompt],
) -> Result<Vec<String>, String> {
    let request_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = oneshot::channel::<Vec<String>>();
    {
        let mut map = pending.inner.lock();
        map.insert(request_id.clone(), tx);
    }

    let payload = AuthPromptsPayload {
        request_id: request_id.clone(),
        name: name.to_string(),
        instructions: instructions.to_string(),
        prompts: prompts
            .iter()
            .map(|p| AuthPrompt {
                prompt: p.prompt.clone(),
                echo: p.echo,
            })
            .collect(),
    };

    app.emit("ssh:auth-prompts", &payload)
        .map_err(|e| format!("ssh auth: emit prompts event: {}", e))?;

    let timeout = std::time::Duration::from_secs(120);
    let answers = match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(answers)) => answers,
        Ok(Err(_)) => {
            // Sender dropped — frontend cancelled the prompt modal. Clean
            // out our pending entry just in case (defensive).
            pending.inner.lock().remove(&request_id);
            return Err("authentication cancelled by user".to_string());
        }
        Err(_) => {
            pending.inner.lock().remove(&request_id);
            return Err("auth prompt timed out — no response within 2 minutes".to_string());
        }
    };

    Ok(answers)
}

/// Tauri command — frontend calls this with the answers collected from
/// the prompt modal. Routes them back to the parked auth flow via the
/// oneshot keyed by `request_id`.
#[tauri::command]
pub async fn ssh_submit_auth_prompts(
    pending: tauri::State<'_, PendingAuthPrompts>,
    request_id: String,
    answers: Vec<String>,
) -> Result<(), String> {
    let sender = {
        let mut map = pending.inner.lock();
        map.remove(&request_id)
    };
    match sender {
        Some(tx) => tx
            .send(answers)
            .map_err(|_| "auth flow no longer waiting (timed out or cancelled)".to_string()),
        None => Err(format!("no pending auth prompt with id {}", request_id)),
    }
}

// ────────────────────────────────────────────────────────────────────────
// ProxyCommand transport
// ────────────────────────────────────────────────────────────────────────

/// Spawn a ProxyCommand subprocess and wrap its stdin/stdout pair as a
/// duplex stream that russh's `connect_stream` can speak SSH over.
///
/// Substitutes `%h` (host), `%p` (port), `%r` (remote username) in the
/// template, then tokenizes via shell-words to get an argv. Spawns
/// `argv[0]` directly with `argv[1..]` as args — **no `/bin/sh -c`**.
/// This keeps cross-platform behavior identical (no shell-escape diffs
/// between bash/zsh/cmd/PowerShell) and avoids invoking arbitrary shell
/// code from a config file. Users needing pipes / redirections / shell
/// variables wrap the command in a script and point ProxyCommand at it.
///
/// Stderr is captured into a buffer (best-effort) so connect failures
/// can surface the proxy command's own error message instead of just
/// "ssh connect failed".
async fn spawn_proxy_command_stream(
    template: &str,
    host: &str,
    port: i64,
    username: &str,
) -> Result<BoxedTransport, String> {
    let resolved = template
        .replace("%h", host)
        .replace("%p", &port.to_string())
        .replace("%r", username);

    let argv = shell_words::split(&resolved)
        .map_err(|e| format!("ProxyCommand parse error: {}", e))?;
    if argv.is_empty() {
        return Err("ProxyCommand template is empty after substitution".to_string());
    }

    let mut cmd = tokio::process::Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    // Critical: kill the subprocess if our task drops without an explicit
    // close. Without this, a failed SSH handshake would leave cloudflared
    // (or whatever) running until the user kills it manually.
    cmd.kill_on_drop(true);

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("ProxyCommand spawn ({}): {}", argv[0], e))?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "ProxyCommand stdin pipe missing".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "ProxyCommand stdout pipe missing".to_string())?;

    // Drain stderr in a background task so it doesn't block the subprocess
    // when its stderr buffer fills. The buffer is shared with the stream
    // so on connection failure we can surface the proxy's complaint.
    let stderr_buf: Arc<parking_lot::Mutex<Vec<u8>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    if let Some(mut stderr) = child.stderr.take() {
        let buf = stderr_buf.clone();
        tokio::spawn(async move {
            let mut chunk = [0u8; 1024];
            loop {
                match stderr.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut g = buf.lock();
                        // Cap the buffer at 8KB — we only need a glimpse for
                        // error context, not the whole subprocess log.
                        if g.len() < 8192 {
                            let take = (8192 - g.len()).min(n);
                            g.extend_from_slice(&chunk[..take]);
                        }
                    }
                }
            }
        });
    }

    Ok(Box::new(ProxyCommandStream {
        stdin,
        stdout,
        _child: child,
        _stderr_buf: stderr_buf,
    }))
}

/// Duplex stream over a ProxyCommand subprocess. Reads from stdout,
/// writes to stdin. Owns the `Child` so dropping the stream kills the
/// subprocess (via `kill_on_drop` set during spawn).
struct ProxyCommandStream {
    stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
    /// Kept alive solely so the subprocess survives — `kill_on_drop` on
    /// the original Command guarantees teardown when this struct drops.
    _child: tokio::process::Child,
    /// Held for potential future error-reporting use (currently captured
    /// but not surfaced — wiring stderr into connect-error strings is a
    /// follow-up).
    _stderr_buf: Arc<parking_lot::Mutex<Vec<u8>>>,
}

impl AsyncRead for ProxyCommandStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stdout).poll_read(cx, buf)
    }
}

impl AsyncWrite for ProxyCommandStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.stdin).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stdin).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stdin).poll_shutdown(cx)
    }
}

// ────────────────────────────────────────────────────────────────────────
// ProxyJump transport
// ────────────────────────────────────────────────────────────────────────

/// Open a `direct-tcpip` SSH channel through `jump_handle` to
/// `(target_host, target_port)`, and expose it as an AsyncRead+AsyncWrite
/// stream the next SSH handshake can speak over.
///
/// russh 0.45's `Channel<Msg>` doesn't implement AsyncRead/AsyncWrite —
/// it speaks in `ChannelMsg` (per the comment in `tunnel.rs`). Bridging
/// approach: create a `tokio::io::duplex` pipe, hand one end back to the
/// caller (which IS AsyncRead+AsyncWrite), and spawn a pump task that
/// shovels bytes between the other end of the duplex and the russh
/// channel.
///
/// Lifetime: the spawned pump task takes ownership of `jump_handle`,
/// keeping the jump SSH session alive for as long as the pump runs. The
/// pump exits when either side closes — caller's stream drops trigger
/// duplex EOF, channel close triggers loop exit. Either way the jump
/// handle drops with the task and russh closes the upstream session.
async fn open_jump_channel_stream(
    jump_handle: Handle<ClientHandler>,
    target_host: &str,
    target_port: u16,
) -> Result<BoxedTransport, String> {
    let channel = jump_handle
        .channel_open_direct_tcpip(target_host, target_port as u32, "127.0.0.1", 0)
        .await
        .map_err(|e| format!("open direct-tcpip via jump: {}", e))?;

    // 64KB duplex buffer — same order of magnitude as a TCP socket recv buffer.
    // Keeps backpressure sensible without buffering whole pages.
    let (client_side, server_side) = tokio::io::duplex(64 * 1024);

    tokio::spawn(pump_channel_into_duplex(server_side, channel, jump_handle));

    Ok(Box::new(client_side))
}

/// Background pump that bridges a tokio duplex stream to a russh channel.
/// Runs until either side closes; on exit, the russh channel is closed
/// (best-effort) and the jump handle drops.
async fn pump_channel_into_duplex(
    duplex: tokio::io::DuplexStream,
    mut channel: russh::Channel<russh::client::Msg>,
    // Held to keep the jump SSH session alive while this pump runs.
    // Drops with the task when the loop exits.
    _jump_handle_keepalive: Handle<ClientHandler>,
) {
    let (mut duplex_read, mut duplex_write) = tokio::io::split(duplex);
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        tokio::select! {
            // Bytes the destination SSH wants to send → forward to channel.
            n = duplex_read.read(&mut buf) => match n {
                Ok(0) => {
                    let _ = channel.eof().await;
                    break;
                }
                Ok(n) => {
                    if channel.data(&buf[..n]).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            },
            // Bytes from the destination → push into duplex for SSH to read.
            msg = channel.wait() => match msg {
                Some(ChannelMsg::Data { data }) => {
                    let bytes: &[u8] = data.as_ref();
                    if duplex_write.write_all(bytes).await.is_err() {
                        break;
                    }
                }
                Some(ChannelMsg::ExtendedData { data, .. }) => {
                    // direct-tcpip shouldn't carry extended data, but if it
                    // does, surface it on the same wire (mirrors tunnel.rs).
                    let bytes: &[u8] = data.as_ref();
                    if duplex_write.write_all(bytes).await.is_err() {
                        break;
                    }
                }
                Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => {
                    let _ = duplex_write.shutdown().await;
                    break;
                }
                Some(_) => {} // ignore Window* / ExitStatus / etc.
            }
        }
    }
    let _ = channel.close().await;
    // _jump_handle_keepalive drops here, closing the upstream SSH session.
}
