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

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::PublicKeyOrCertificate,
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
    /// Whether an unanswered challenge (OTP / password prompt) may be handed
    /// to a human through the app's prompt modal. Terminal, tunnels and
    /// Explorer set `true`; MCP calls set `false` — an agent call has nobody
    /// to answer, and blocking until the timeout is worse than a clear error.
    pub interactive: bool,
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
    interactive: bool,
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
            interactive,
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
    let tcp = open_tcp(&spec).await?;
    match perform_ssh_handshake_and_auth(Box::new(tcp), &spec).await {
        // The primary method (key/agent) failed but the host may want a
        // one-time code. russh cannot run that exchange on a used handle, so
        // connect again and authenticate with keyboard-interactive only.
        Err(e) if e == NEED_KBI_RETRY => {
            log::info!("[ssh] key/agent auth failed; retrying with keyboard-interactive");
            let tcp = open_tcp(&spec).await?;
            perform_ssh_kbi_auth(Box::new(tcp), &spec).await
        }
        other => other,
    }
}

/// Fresh TCP socket with Nagle disabled — critical for interactive SSH
/// (40-200 ms keystroke latency without it).
async fn open_tcp(spec: &SshAuthSpec) -> Result<tokio::net::TcpStream, String> {
    let tcp = tokio::net::TcpStream::connect((spec.host.as_str(), spec.port))
        .await
        .map_err(|e| format!("tcp connect: {}", e))?;
    if let Err(e) = tcp.set_nodelay(true) {
        log::warn!("[ssh] warning: TCP_NODELAY not set ({})", e);
    }
    Ok(tcp)
}

/// Sentinel returned when the primary auth method failed but the host may
/// still want a one-time code. The caller reconnects and drives
/// keyboard-interactive on the fresh connection.
///
/// Why a reconnect: whether a second auth exchange on an already-used handle
/// is wired up correctly is a library detail we don't want to depend on. In
/// russh 0.45 the KBI request state was only armed during the service-accept
/// handshake, so `authenticate_keyboard_interactive_start` after a failed
/// method wrote the request without updating `auth_request.current` — the
/// server's INFO_REQUEST was dropped and the auth future hung until timeout.
/// A fresh connection arms everything through the handshake, which is
/// correct on every version.
const NEED_KBI_RETRY: &str = "__zeroany_need_kbi_retry__";

/// Run the SSH handshake and authentication on the given transport
/// stream. Shared by the direct, ProxyCommand, and ProxyJump paths so
/// auth code stays in one place.
fn ssh_client_config() -> Arc<client::Config> {
    Arc::new(client::Config {
        keepalive_interval: Some(std::time::Duration::from_secs(30)),
        keepalive_max: 3,
        ..client::Config::default()
    })
}

/// SSH handshake over an already-established transport, with the connect
/// timeout (15 s) applied. Shared by the primary and second-factor paths.
async fn ssh_connect_stream(
    transport: BoxedTransport,
) -> Result<Handle<ClientHandler>, String> {
    let connect_fut = client::connect_stream(ssh_client_config(), transport, ClientHandler);
    match tokio::time::timeout(std::time::Duration::from_secs(15), connect_fut).await {
        Ok(Ok(h)) => Ok(h),
        Ok(Err(e)) => Err(format!("ssh connect: {}", e)),
        Err(_) => {
            crate::telemetry::bump("err.ssh_timeout");
            Err("ssh connect: timed out after 15s".to_string())
        }
    }
}

/// True when the error is the "this call cannot answer a challenge" refusal,
/// which must reach the caller instead of being swallowed into a generic
/// "authentication failed" (that is what an agent gets, and it is useless).
fn is_challenge_refusal(error: &str) -> bool {
    error.contains("interactive challenge")
}

/// Second-factor pass on a FRESH connection: nothing but
/// keyboard-interactive, so the OTP round is armed correctly (see
/// `NEED_KBI_RETRY`).
async fn perform_ssh_kbi_auth(
    transport: BoxedTransport,
    spec: &SshAuthSpec,
) -> Result<Handle<ClientHandler>, String> {
    let mut handle = ssh_connect_stream(transport).await?;
    let authed = match tokio::time::timeout(
        std::time::Duration::from_secs(150),
        try_keyboard_interactive_with_ui(
            &mut handle,
            &spec.username,
            spec.secret.as_deref(),
            spec.interactive,
        ),
    )
    .await
    {
        Ok(Ok(ok)) => ok,
        Ok(Err(e)) => return Err(e),
        Err(_) => {
            crate::telemetry::bump("err.ssh_timeout");
            return Err("ssh auth (one-time code): timed out after 150s".to_string());
        }
    };
    if !authed {
        return Err("authentication failed".to_string());
    }
    crate::telemetry::bump("ssh.connect");
    Ok(handle)
}

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
    let mut handle = ssh_connect_stream(transport).await?;

    // Auth phase has its own timeout — separate from the handshake timeout
    // above. If the server closes the connection mid-negotiation (some
    // hardened sshd configs do this after a single rejected method),
    // russh's underlying recv can wait forever. 30s covers slow PAM stacks
    // (e.g. LDAP-backed auth doing a network round-trip) without burning
    // the user's afternoon on a stuck UI.
    // Auth can require human input (keyboard-interactive: password + OTP).
    // The per-prompt wait is 120 s, so a 30 s outer budget guaranteed a
    // timeout before anyone could type a code. Keep enough headroom for one
    // prompt round plus the handshake; a dead server still fails fast on its
    // own (connection attempts have their own 15 s budget).
    let authed = match tokio::time::timeout(
        std::time::Duration::from_secs(150),
        run_auth(&mut handle, spec),
    )
    .await
    {
        Ok(Ok(b)) => b,
        Ok(Err(e)) => return Err(e),
        Err(_) => {
            crate::telemetry::bump("err.ssh_timeout");
            return Err("ssh auth: timed out after 150s".to_string());
        }
    };
    if !authed {
        // A key/agent step proves the first factor but cannot carry a one-time
        // code; ask the caller to retry through keyboard-interactive on a new
        // connection. Password profiles already tried KBI inside `run_auth`.
        if matches!(spec.auth_type.as_str(), "key" | "agent") {
            return Err(NEED_KBI_RETRY.to_string());
        }
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
            let keypair = russh::keys::load_secret_key(key_path, passphrase)
                .map_err(|e| format!("load key: {}", e))?;
            let primary = handle
                .authenticate_publickey(
                    &spec.username,
                    russh::keys::PrivateKeyWithHashAlg::new(Arc::new(keypair), None),
                )
                .await
                .map_err(|e| format!("ssh auth publickey: {}", e))?
                .success();
            if primary {
                true
            } else {
                // Key + OTP hosts: the key satisfies the first factor and the
                // server answers by offering `keyboard-interactive` for the
                // second. Without this fallback such hosts just reported
                // "authentication failed" and the user never saw a code field.
                try_keyboard_interactive_fallback(handle, &spec.username, None, spec.interactive).await?
            }
        }
        "password" => {
            // Three-step ladder, in this order on purpose:
            //
            // 1. keyboard-interactive straight away. Ordinary MFA hosts ask
            //    "Password:" then the OTP inside one KBI exchange; the stored
            //    secret pre-fills the password round so nothing pops up.
            // 2. the plain `password` method. Some sshd/PAM stacks (the
            //    bastions this was written for) reject a KBI *first* attempt
            //    with "remaining: password,publickey", accept the password as
            //    factor #1 and only then advertise keyboard-interactive as
            //    factor #2.
            // 3. keyboard-interactive again for the OTP. This second attempt
            //    does work even though russh 0.45 does not re-arm
            //    `CurrentRequest::KeyboardInteractive` for it: the first
            //    (rejected) attempt armed it during the service-accept
            //    handshake, and a USERAUTH_FAILURE does not clear it.
            let first = try_keyboard_interactive_with_ui(
                handle,
                &spec.username,
                spec.secret.as_deref(),
                spec.interactive,
            )
            .await;
            match first {
                Ok(true) => true,
                other => {
                    if let Err(error) = &other {
                        if is_challenge_refusal(error) {
                            return Err(error.clone());
                        }
                        log::debug!(
                            target: "ssh::auth",
                            "keyboard-interactive (first try) unavailable: {}",
                            error
                        );
                    }
                    let password_ok = match spec.secret.clone() {
                        Some(password) => match handle
                            .authenticate_password(&spec.username, password)
                            .await
                        {
                            Ok(result) => result.success(),
                            Err(error) => {
                                log::debug!(
                                    target: "ssh::auth",
                                    "password method failed: {}",
                                    error
                                );
                                false
                            }
                        },
                        None => false,
                    };
                    if password_ok {
                        true
                    } else {
                        // The host may now be demanding the second factor.
                        match try_keyboard_interactive_with_ui(
                            handle,
                            &spec.username,
                            spec.secret.as_deref(),
                            spec.interactive,
                        )
                        .await
                        {
                            Ok(ok) => ok,
                            Err(error) => {
                                if is_challenge_refusal(&error) {
                                    return Err(error);
                                }
                                log::debug!(
                                    target: "ssh::auth",
                                    "keyboard-interactive (second factor) unavailable: {}",
                                    error
                                );
                                false
                            }
                        }
                    }
                }
            }
        }
        "agent" => {
            if try_agent_auth(handle, &spec.username).await? {
                true
            } else {
                // Agent + OTP: same story as key auth — the agent proves the
                // first factor, keyboard-interactive carries the code.
                try_keyboard_interactive_fallback(handle, &spec.username, None, spec.interactive).await?
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
            try_keyboard_interactive_with_ui(
                handle,
                &spec.username,
                spec.secret.as_deref(),
                spec.interactive,
            )
            .await?
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
    interactive: bool,
) -> Result<bool, String> {
    match try_keyboard_interactive_with_ui(handle, username, pre_stored_secret, interactive).await {
        Ok(ok) => Ok(ok),
        Err(error) => {
            if is_challenge_refusal(&error) {
                return Err(error);
            }
            log::debug!(
                target: "ssh::auth",
                "keyboard-interactive fallback unavailable: {}",
                error
            );
            Ok(false)
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
    interactive: bool,
) -> Result<bool, String> {
    use russh::client::KeyboardInteractiveAuthResponse;

    // Scripted mode (tests / fixture harness) drives the rounds without a UI,
    // so it must not require a live Tauri AppHandle.
    #[cfg(debug_assertions)]
    let scripted = scripted_prompts_armed();
    #[cfg(not(debug_assertions))]
    let scripted = false;

    let ui: Option<(&AppHandle, tauri::State<'_, PendingAuthPrompts>)> = if scripted || !interactive {
        // A non-interactive caller (MCP) has nobody to answer: don't even
        // resolve an AppHandle — the prompt round turns into a refusal below.
        None
    } else {
        let app = app_handle().ok_or_else(|| {
            "keyboard-interactive auth: AppHandle not available (Tauri startup incomplete)"
                .to_string()
        })?;
        let pending: tauri::State<'_, PendingAuthPrompts> = app.state();
        Some((app, pending))
    };

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
            KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
            KeyboardInteractiveAuthResponse::InfoRequest {
                name,
                instructions,
                prompts,
            } => {
                log::debug!(
                    target: "ssh::auth",
                    "keyboard-interactive round {} user={} prompts={:?}",
                    round_index,
                    username,
                    prompts.iter().map(|p| (p.prompt.trim(), p.echo)).collect::<Vec<_>>()
                );
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
                } else if !interactive {
                    // Nobody can answer this call: refuse with the prompt
                    // names instead of leaving a modal dangling until the
                    // caller's timeout expires.
                    let wanted = prompts
                        .iter()
                        .map(|p| p.prompt.trim().to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(format!(
                        "host needs an interactive challenge ({}) — an agent call cannot answer \
                         it. Connect from the SSH panel so a human can, or use an account without MFA.",
                        wanted
                    ));
                } else {
                    match &ui {
                        Some((app, pending)) => {
                            request_prompts_from_user(
                                app,
                                pending,
                                &name,
                                &instructions,
                                &prompts,
                            )
                            .await?
                        }
                        None => {
                            #[cfg(debug_assertions)]
                            {
                                take_scripted_answers(&name, &prompts)?
                            }
                            #[cfg(not(debug_assertions))]
                            {
                                return Err(
                                    "keyboard-interactive auth: no prompt source".to_string()
                                );
                            }
                        }
                    }
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

/// Debug-build hook: answer keyboard-interactive rounds from a script instead
/// of the UI. Used by the MFA regression test, which drives the real auth
/// ladder against `tests/fixtures/mfa_ssh_server.py`; never compiled into a
/// release build.
#[cfg(debug_assertions)]
static SCRIPTED_PROMPT_ANSWERS: std::sync::Mutex<Vec<Vec<String>>> =
    std::sync::Mutex::new(Vec::new());
#[cfg(debug_assertions)]
static SCRIPTED_PROMPTS_ARMED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Arm scripted answers for the next keyboard-interactive rounds (tests only).
#[cfg(debug_assertions)]
pub(crate) fn set_scripted_prompt_answers(rounds: Vec<Vec<String>>) {
    if let Ok(mut guard) = SCRIPTED_PROMPT_ANSWERS.lock() {
        *guard = rounds;
    }
    SCRIPTED_PROMPTS_ARMED.store(true, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(debug_assertions)]
fn scripted_prompts_armed() -> bool {
    SCRIPTED_PROMPTS_ARMED.load(std::sync::atomic::Ordering::SeqCst)
}

/// Take the answers for one InfoRequest round from the script.
#[cfg(debug_assertions)]
fn take_scripted_answers(
    name: &str,
    prompts: &[russh::client::Prompt],
) -> Result<Vec<String>, String> {
    let answers = SCRIPTED_PROMPT_ANSWERS
        .lock()
        .ok()
        .and_then(|mut queue| {
            if queue.is_empty() {
                None
            } else {
                Some(queue.remove(0))
            }
        })
        .ok_or_else(|| format!("scripted prompts exhausted at round {:?}", name))?;
    log::info!(
        target: "ssh::auth",
        "scripted answers: {:?} for {:?}",
        answers,
        prompts.iter().map(|p| p.prompt.trim()).collect::<Vec<_>>()
    );
    Ok(answers)
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

    log::info!(
        target: "ssh::auth",
        "emit ssh:auth-prompts request={} prompts={:?}",
        request_id,
        payload.prompts.iter().map(|p| p.prompt.trim()).collect::<Vec<_>>()
    );
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
            log::warn!(
                target: "ssh::auth",
                "auth prompts request={} never answered (120s) — modal never submitted",
                request_id
            );
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
        Some(tx) => {
            log::info!(
                target: "ssh::auth",
                "auth prompts answered request={} answers={}",
                request_id,
                answers.len()
            );
            tx.send(answers)
                .map_err(|_| "auth flow no longer waiting (timed out or cancelled)".to_string())
        }
        None => {
            log::warn!(
                target: "ssh::auth",
                "auth prompts submit for unknown/expired request={}",
                request_id
            );
            Err(format!("no pending auth prompt with id {}", request_id))
        }
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

#[cfg(test)]
mod mfa_ladder_tests {
    use super::*;
    use std::process::{Child, Command, Stdio};

    const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mfa_ssh_server.py");

    /// The scripted answers are one global queue, so two fixture tests must
    /// never interleave: whichever arms last wins and the other starves.
    static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    struct Fixture {
        child: Child,
        port: u16,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .map(|addr| addr.port())
            .unwrap_or(2222)
    }

    fn start_fixture() -> Option<Fixture> {
        let port = free_port();
        let child = Command::new("python3")
            .arg(FIXTURE)
            .arg(port.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::from(
                std::fs::File::create("/tmp/mfa_fixture_stderr.log").unwrap(),
            ))
            .spawn()
            .ok()?;
        let fixture = Fixture { child, port };
        for _ in 0..100 {
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return Some(fixture);
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        None
    }

    /// Minimal stderr logger so `--nocapture` shows the auth ladder.
    struct StderrLogger;
    impl log::Log for StderrLogger {
        fn enabled(&self, metadata: &log::Metadata) -> bool {
            metadata.level() <= log::Level::Debug
        }
        fn log(&self, record: &log::Record) {
            if self.enabled(record.metadata()) {
                eprintln!("[{}] {}", record.level(), record.args());
            }
        }
        fn flush(&self) {}
    }

    fn init_logger() {
        static LOGGER: StderrLogger = StderrLogger;
        let _ = log::set_logger(&LOGGER);
        log::set_max_level(log::LevelFilter::Debug);
    }

    /// Regression for the password + OTP bastion: it rejects a
    /// keyboard-interactive first attempt, accepts the password as factor #1
    /// and only then advertises keyboard-interactive for the code. The ladder
    /// must come back around to a working KBI exchange and consume the OTP.
    #[tokio::test]
    #[ignore = "spawns python3 + paramiko to emulate a password+OTP bastion"]
    async fn password_plus_otp_ladder_reaches_the_code_round() {
        let _serial = SERIAL.lock().await;
        init_logger();
        let Some(fixture) = start_fixture() else {
            panic!("fixture unavailable — needs python3 with paramiko");
        };
        // Round 0 ("Password: ") is answered from the stored secret, so the
        // script only has to carry the code round.
        set_scripted_prompt_answers(vec![vec!["123456".to_string()]]);

        let spec = SshAuthSpec {
            host: "127.0.0.1".to_string(),
            port: fixture.port,
            username: "test".to_string(),
            auth_type: "password".to_string(),
            key_path: None,
            secret: Some("pw1".to_string()),
            interactive: true,
        };

        let handle = open_authenticated_ssh_session_with_spec(spec)
            .await
            .expect("auth must succeed once the OTP is supplied");
        drop(handle);
    }

    /// A non-interactive caller (MCP) must not dangle on an OTP prompt it
    /// cannot answer: it gets a refusal naming the challenge, immediately,
    /// instead of waiting out the exec timeout with a modal left open in
    /// the UI.
    #[tokio::test]
    #[ignore = "spawns python3 + paramiko to emulate a password+OTP bastion"]
    async fn non_interactive_auth_refuses_the_otp_round() {
        let _serial = SERIAL.lock().await;
        init_logger();
        let Some(fixture) = start_fixture() else {
            panic!("fixture unavailable — needs python3 with paramiko");
        };

        let spec = SshAuthSpec {
            host: "127.0.0.1".to_string(),
            port: fixture.port,
            username: "test".to_string(),
            auth_type: "password".to_string(),
            key_path: None,
            secret: Some("pw1".to_string()),
            interactive: false,
        };

        let started = std::time::Instant::now();
        let error = match open_authenticated_ssh_session_with_spec(spec).await {
            Ok(_) => panic!("an unanswered OTP round must fail, not block"),
            Err(e) => e,
        };
        let elapsed = started.elapsed();
        assert!(
            error.contains("interactive challenge"),
            "unexpected error: {}",
            error
        );
        assert!(
            error.contains("OTP") || error.to_lowercase().contains("code"),
            "the refusal should name the challenge: {}",
            error
        );
        assert!(
            elapsed < std::time::Duration::from_secs(20),
            "refusal took {:?} — it should fail fast",
            elapsed
        );
    }

    /// Once a terminal session is up, its connection is parked in the live
    /// registry: MCP commands then run as extra channels on it, with no
    /// second authentication (the only way in on an OTP-only host).
    #[tokio::test]
    #[ignore = "spawns python3 + paramiko to emulate a password+OTP bastion"]
    async fn live_session_serves_commands_without_reauthenticating() {
        let _serial = SERIAL.lock().await;
        init_logger();
        let Some(fixture) = start_fixture() else {
            panic!("fixture unavailable — needs python3 with paramiko");
        };
        set_scripted_prompt_answers(vec![vec!["123456".to_string()]]);

        let spec = SshAuthSpec {
            host: "127.0.0.1".to_string(),
            port: fixture.port,
            username: "test".to_string(),
            auth_type: "password".to_string(),
            key_path: None,
            secret: Some("pw1".to_string()),
            interactive: true,
        };
        let handle = open_authenticated_ssh_session_with_spec(spec)
            .await
            .expect("auth must succeed");
        let profile_id = "live-test-profile";
        let guard = super::super::live::attach(profile_id, handle);
        assert!(super::super::live::has(profile_id));

        let result = super::super::live::run(
            profile_id,
            "uname -a",
            std::time::Duration::from_secs(15),
        )
        .await
        .expect("a live session must be found")
        .expect("exec on the live session must succeed");
        assert_eq!(result["exitCode"], 0);
        assert!(
            result["stdout"]
                .as_str()
                .unwrap_or("")
                .contains("EXEC OK: uname -a"),
            "unexpected stdout: {}",
            result["stdout"]
        );

        drop(guard);
        assert!(!super::super::live::has(profile_id), "guard must unregister");
        assert!(
            super::super::live::run(profile_id, "uname -a", std::time::Duration::from_secs(2))
                .await
                .is_none(),
            "after the session ends there is nothing to reuse"
        );
    }

    /// The MCP `ssh_exec` path: exec a command on an authenticated session
    /// and collect stdout / exit code. The fixture answers every exec with
    /// `EXEC OK: <command>` and status 0.
    #[tokio::test]
    #[ignore = "spawns python3 + paramiko to emulate a password+OTP bastion"]
    async fn mcp_exec_collects_stdout_and_exit_code() {
        let _serial = SERIAL.lock().await;
        init_logger();
        let Some(fixture) = start_fixture() else {
            panic!("fixture unavailable — needs python3 with paramiko");
        };
        set_scripted_prompt_answers(vec![vec!["123456".to_string()]]);

        let spec = SshAuthSpec {
            host: "127.0.0.1".to_string(),
            port: fixture.port,
            username: "test".to_string(),
            auth_type: "password".to_string(),
            key_path: None,
            secret: Some("pw1".to_string()),
            interactive: true,
        };
        let handle = open_authenticated_ssh_session_with_spec(spec)
            .await
            .expect("auth must succeed");

        let result = crate::modes::workspace::mcp::exec_on_handle(&handle, "uname -a")
            .await
            .expect("exec must succeed");
        assert_eq!(result["exitCode"], 0);
        assert!(
            result["stdout"].as_str().unwrap_or("").contains("EXEC OK: uname -a"),
            "unexpected stdout: {}",
            result["stdout"]
        );
    }
}
