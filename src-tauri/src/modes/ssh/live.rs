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
        let result = crate::modes::workspace::mcp::exec_on_handle(&handle, &job.command).await;
        let _ = job.reply.send(result);
    }
    log::info!(target: "ssh::auth", "live session closed for {}", profile_id);
}