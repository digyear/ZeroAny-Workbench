//! Live-session registry: MCP commands run on the SSH connection the user
//! already authenticated.
//!
//! A host behind a one-time-code bastion can't be reached by a stateless
//! "connect, run, disconnect" call — an agent would have to answer the OTP.
//! So while a terminal session for a profile is alive, its `Handle` is
//! parked here and commands run as extra channels on *that* connection.
//!
//! Closing the terminal does NOT tear the connection down: the entry becomes
//! *detached* and stays usable for [`IDLE_TIMEOUT`], refreshed on every use.
//! That is the point — log in once with the code, then let agents work with
//! no tab sitting open. Detached entries expire on their own, and the user
//! can drop one immediately from Settings → MCP.
//!
//! The handle never leaves its worker task (russh's `Handle` is not `Sync`),
//! so concurrent MCP calls queue as messages instead of sharing state.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use russh::client::Handle;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use crate::modes::ssh::ssh_session::ClientHandler;

/// How long a session stays usable after its terminal closed.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

struct ExecJob {
    command: String,
    reply: oneshot::Sender<Result<Value, String>>,
}

struct Entry {
    /// Distinguishes this attachment from a later reconnect to the same
    /// profile, so a stale worker can't delete a fresh registration.
    id: u64,
    tx: mpsc::UnboundedSender<ExecJob>,
    connected_at: Instant,
    last_used: Instant,
    /// Set when the terminal that owned this connection closed.
    detached_at: Option<Instant>,
}

impl Entry {
    /// Idle clock runs from the last use, so a session in active use is
    /// never expired out from under a command.
    fn expired(&self) -> bool {
        self.detached_at.is_some() && self.last_used.elapsed() > IDLE_TIMEOUT
    }

    fn expires_in(&self) -> Option<u64> {
        self.detached_at
            .map(|_| IDLE_TIMEOUT.saturating_sub(self.last_used.elapsed()).as_secs())
    }
}

fn registry() -> &'static Mutex<HashMap<String, Entry>> {
    static LIVE: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    LIVE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Keeps the entry attached while the caller owns it. Dropping it *detaches*
/// the session (idle-expiring) instead of closing it: closing the terminal
/// tab must not take the agent's access away.
pub struct LiveGuard {
    profile_id: String,
    id: u64,
}

impl Drop for LiveGuard {
    fn drop(&mut self) {
        let mut reg = registry().lock().unwrap();
        if let Some(entry) = reg.get_mut(&self.profile_id) {
            if entry.id == self.id {
                entry.detached_at = Some(Instant::now());
            }
        }
    }
}

/// Park an authenticated handle for `profile_id`. Call it once the caller
/// has finished opening its own channels — after that the worker owns the
/// handle and keeps the connection alive.
pub fn attach(profile_id: &str, handle: Handle<ClientHandler>) -> LiveGuard {
    let id = next_id();
    let (tx, rx) = mpsc::unbounded_channel();
    let now = Instant::now();
    {
        let mut reg = registry().lock().unwrap();
        // A newer attachment (the user reconnected) replaces the old one;
        // the previous guard's drop then sees a different id and leaves it.
        reg.insert(
            profile_id.to_string(),
            Entry {
                id,
                tx,
                connected_at: now,
                last_used: now,
                detached_at: None,
            },
        );
    }
    tokio::spawn(worker(profile_id.to_string(), handle, rx));
    spawn_reaper();
    LiveGuard {
        profile_id: profile_id.to_string(),
        id,
    }
}

/// Drop detached sessions that outlived [`IDLE_TIMEOUT`].
fn prune() {
    let mut expired: Vec<String> = Vec::new();
    {
        let mut reg = registry().lock().unwrap();
        let ids: Vec<String> = reg
            .iter()
            .filter(|(_, e)| e.expired())
            .map(|(k, _)| k.clone())
            .collect();
        for id in ids {
            reg.remove(&id);
            expired.push(id);
        }
    }
    for id in expired {
        log::info!(
            target: "ssh::auth",
            "live session for {} expired after {}s idle",
            id,
            IDLE_TIMEOUT.as_secs()
        );
        // Removing the entry drops the sender, which ends the worker's recv
        // loop and drops the handle — the connection closes with it.
    }
}

fn spawn_reaper() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        tokio::spawn(async {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                prune();
            }
        });
    });
}

/// Run one command on the live connection for `profile_id`, if there is one.
/// `None` means "no live session" — the caller should connect on its own.
pub async fn run(
    profile_id: &str,
    command: &str,
    timeout: Duration,
) -> Option<Result<Value, String>> {
    let (tx, rx) = oneshot::channel();
    {
        let mut reg = registry().lock().unwrap();
        let entry = reg.get_mut(profile_id)?;
        if entry
            .tx
            .send(ExecJob {
                command: command.to_string(),
                reply: tx,
            })
            .is_err()
        {
            // Worker is gone; forget it so the caller dials fresh.
            reg.remove(profile_id);
            return None;
        }
        // Activity keeps a detached session alive (expiry counts from here).
        entry.last_used = Instant::now();
    }
    match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(result)) => Some(result),
        Ok(Err(_)) => None,
        Err(_) => Some(Err(format!(
            "command still running on the live session after {}s",
            timeout.as_secs()
        ))),
    }
}

/// Whether a connection is parked for this profile (diagnostics/tests).
pub fn has(profile_id: &str) -> bool {
    registry().lock().unwrap().contains_key(profile_id)
}

/// Close a parked connection immediately (Settings → MCP "Disconnect").
pub fn disconnect(profile_id: &str) -> bool {
    registry().lock().unwrap().remove(profile_id).is_some()
}

/// Snapshot for the UI: what is parked right now, and for how much longer.
pub fn snapshot() -> Vec<Value> {
    let reg = registry().lock().unwrap();
    let mut rows: Vec<Value> = reg
        .iter()
        .map(|(profile_id, e)| {
            json!({
                "profileId": profile_id,
                "attached": e.detached_at.is_none(),
                "connectedSecs": e.connected_at.elapsed().as_secs(),
                "idleSecs": e.last_used.elapsed().as_secs(),
                "expiresInSecs": e.expires_in(),
                "idleTimeoutSecs": IDLE_TIMEOUT.as_secs(),
            })
        })
        .collect();
    rows.sort_by(|a, b| {
        a["profileId"]
            .as_str()
            .unwrap_or("")
            .cmp(b["profileId"].as_str().unwrap_or(""))
    });
    rows
}

async fn worker(
    profile_id: String,
    handle: Handle<ClientHandler>,
    mut rx: mpsc::UnboundedReceiver<ExecJob>,
) {
    log::info!(target: "ssh::auth", "live session parked for {}", profile_id);
    while let Some(job) = rx.recv().await {
        let result = crate::modes::workspace::mcp::exec_on_handle(&handle, &job.command, None).await;
        let _ = job.reply.send(result);
    }
    log::info!(target: "ssh::auth", "live session closed for {}", profile_id);
}

// ── Shared (borrow) mode ───────────────────────────────────────────────────
// Some hosts only work over a shell, and sometimes the user *wants* to watch
// what the agent runs. Both are served by typing the command into the
// terminal session the user already has open, with the output captured from
// the same fan-out the UI reads.

struct ShellLink {
    terminal_id: String,
    tx: mpsc::UnboundedSender<crate::modes::ssh::models::SshCommand>,
}

fn shells() -> &'static Mutex<HashMap<String, ShellLink>> {
    static SHELLS: OnceLock<Mutex<HashMap<String, ShellLink>>> = OnceLock::new();
    SHELLS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// One shared command at a time: two injections into the same terminal would
/// interleave their sentinels and nobody could tell whose output was whose.
static SHARED_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Ties a shell link to the terminal session that owns it.
pub struct ShellGuard {
    profile_id: String,
    terminal_id: String,
}

impl Drop for ShellGuard {
    fn drop(&mut self) {
        let mut map = shells().lock().unwrap();
        if map
            .get(&self.profile_id)
            .map(|l| l.terminal_id == self.terminal_id)
            == Some(true)
        {
            map.remove(&self.profile_id);
        }
    }
}

/// Publish the terminal's shell channel for shared-mode commands.
pub fn attach_shell(
    profile_id: &str,
    terminal_id: &str,
    tx: mpsc::UnboundedSender<crate::modes::ssh::models::SshCommand>,
) -> ShellGuard {
    shells().lock().unwrap().insert(
        profile_id.to_string(),
        ShellLink {
            terminal_id: terminal_id.to_string(),
            tx,
        },
    );
    ShellGuard {
        profile_id: profile_id.to_string(),
        terminal_id: terminal_id.to_string(),
    }
}

pub fn has_shell(profile_id: &str) -> bool {
    shells().lock().unwrap().contains_key(profile_id)
}

/// Run `command` inside the user's terminal session: the command line and its
/// output are visible there, and the result is captured in parallel.
/// `None` = no terminal is open for this profile.
pub async fn run_shared(
    profile_id: &str,
    command: &str,
    timeout: Duration,
) -> Option<Result<Value, String>> {
    let (terminal_id, tx) = {
        let map = shells().lock().unwrap();
        let link = map.get(profile_id)?;
        (link.terminal_id.clone(), link.tx.clone())
    };
    // Subscribe BEFORE typing, so no early output can be missed.
    let mut attached = crate::companion::fanout::attach(&terminal_id)?;

    let tag = uuid::Uuid::new_v4().simple().to_string();
    let begin = format!("__ZA_B_{}__", tag);
    let end = format!("__ZA_E_{}__", tag);
    let script = format!(
        "printf '\\n{0}\\n'; {1}\nprintf '\\n{2} %s\\n' \"$?\"\n",
        begin, command, end
    );

    let _serial = SHARED_LOCK.lock().await;
    if tx
        .send(crate::modes::ssh::models::SshCommand::Write(
            script.into_bytes(),
        ))
        .is_err()
    {
        return None;
    }

    let deadline = tokio::time::Instant::now() + timeout;
    let mut buf: Vec<u8> = Vec::new();
    let mut truncated = false;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return Some(Err(format!(
                "shared command still running after {}s",
                timeout.as_secs()
            )));
        }
        match tokio::time::timeout(left, attached.rx.recv()).await {
            Ok(Ok(crate::companion::fanout::FanoutEvent::Out(bytes))) => {
                crate::modes::workspace::mcp::push_capped(&mut buf, &bytes, &mut truncated);
                if String::from_utf8_lossy(&buf).contains(&end) {
                    // Let the exit status (printed after the end marker) land.
                    let grace = tokio::time::Instant::now() + Duration::from_millis(250);
                    while tokio::time::Instant::now() < grace {
                        match tokio::time::timeout(
                            Duration::from_millis(50),
                            attached.rx.recv(),
                        )
                        .await
                        {
                            Ok(Ok(crate::companion::fanout::FanoutEvent::Out(bytes))) => {
                                crate::modes::workspace::mcp::push_capped(
                                    &mut buf,
                                    &bytes,
                                    &mut truncated,
                                )
                            }
                            Ok(Ok(_)) => {}
                            _ => break,
                        }
                    }
                    break;
                }
            }
            Ok(Ok(_)) => {}
            Ok(Err(_)) | Err(_) => break,
        }
    }

    let text = String::from_utf8_lossy(&buf).to_string();
    let (body, exit_code) = crate::modes::workspace::mcp::split_sentinel_output(&text, &begin, &end)?;
    Some(Ok(json!({
        "command": command,
        "stdout": body,
        "stderr": "",
        "exitCode": exit_code,
        "truncated": truncated,
        "mode": "shared",
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::companion::fanout;
    use crate::modes::ssh::models::SshCommand;

    /// The shared path is pure plumbing: type into the terminal's channel,
    /// read the terminal's fan-out. A fake terminal lets us check that
    /// without SSH — including the PTY echo, which contains the markers.
    #[tokio::test]
    async fn shared_mode_types_into_the_terminal_and_captures_output() {
        let profile = format!("shared-test-{}", uuid::Uuid::new_v4().simple());
        let terminal_id = format!("term-{}", uuid::Uuid::new_v4().simple());
        fanout::register(&terminal_id, fanout::TermKind::Ssh, "test");

        let (tx, mut rx) = mpsc::unbounded_channel::<SshCommand>();
        let tid = terminal_id.clone();
        let fake_shell = tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                let SshCommand::Write(bytes) = cmd else { continue };
                let script = String::from_utf8_lossy(&bytes).to_string();
                // PTY echo first — it contains both marker literals.
                fanout::publish(&tid, script.as_bytes());
                let tag_start = script.find("__ZA_B_").expect("begin marker") + "__ZA_B_".len();
                let tag_end = script[tag_start..].find("__").expect("tag end") + tag_start;
                let tag = &script[tag_start..tag_end];
                fanout::publish(&tid, format!("\r\n__ZA_B_{}__\r\n", tag).as_bytes());
                fanout::publish(&tid, b"Linux fakebox 6.1\r\n");
                fanout::publish(&tid, format!("\r\n__ZA_E_{}__ 0\r\n", tag).as_bytes());
            }
        });

        let guard = attach_shell(&profile, &terminal_id, tx);
        let result = run_shared(&profile, "uname -a", Duration::from_secs(5))
            .await
            .expect("a shell is attached")
            .expect("the fake shell answers");
        assert_eq!(result["mode"], serde_json::json!("shared"));
        assert_eq!(result["exitCode"], serde_json::json!(0));
        assert_eq!(result["stdout"], serde_json::json!("Linux fakebox 6.1"));

        drop(guard);
        assert!(!has_shell(&profile), "guard must remove the link");
        assert!(
            run_shared(&profile, "uname -a", Duration::from_secs(1)).await.is_none(),
            "with no terminal open shared mode has nowhere to type"
        );
        fake_shell.abort();
        fanout::unregister(&terminal_id);
    }
}
