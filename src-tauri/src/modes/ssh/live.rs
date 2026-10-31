//! Live-session registry: MCP commands run on the SSH connection the user
//! already authenticated.
//!
//! A host behind a one-time-code bastion can't be reached by a stateless
//! "connect, run, disconnect" call — an agent would have to answer the OTP.
//! So while a terminal session for a profile is alive, its `Handle` is
//! parked here and commands run as extra channels on *that* connection.
//! When the last terminal for the profile closes, the guard drops, the
//! worker exits, and the connection goes away with it.
//!
//! The handle never leaves its worker task (russh's `Handle` is not `Sync`),
//! so concurrent MCP calls queue as messages instead of sharing state.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use russh::client::Handle;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use crate::modes::ssh::ssh_session::ClientHandler;

struct ExecJob {
    command: String,
    reply: oneshot::Sender<Result<Value, String>>,
}

struct Entry {
    /// Distinguishes this attachment from a later reconnect to the same
    /// profile, so a stale worker can't delete a fresh registration.
    id: u64,
    tx: mpsc::UnboundedSender<ExecJob>,
}

fn registry() -> &'static Mutex<HashMap<String, Entry>> {
    static LIVE: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    LIVE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Keeps the registry entry alive for as long as the caller owns it. Drop it
/// when the terminal session ends and the connection closes with it.
pub struct LiveGuard {
    profile_id: String,
    id: u64,
}

impl Drop for LiveGuard {
    fn drop(&mut self) {
        let mut reg = registry().lock().unwrap();
        if reg.get(&self.profile_id).map(|e| e.id) == Some(self.id) {
            reg.remove(&self.profile_id);
        }
    }
}

/// Park an authenticated handle for `profile_id`. Call it once the caller
/// has finished opening its own channels — after that the worker owns the
/// handle and keeps the connection alive.
pub fn attach(profile_id: &str, handle: Handle<ClientHandler>) -> LiveGuard {
    let id = next_id();
    let (tx, rx) = mpsc::unbounded_channel();
    {
        let mut reg = registry().lock().unwrap();
        // A newer attachment (the user reconnected) replaces the old one;
        // the previous guard's drop then sees a different id and leaves it.
        reg.insert(profile_id.to_string(), Entry { id, tx });
    }
    tokio::spawn(worker(profile_id.to_string(), handle, rx));
    LiveGuard {
        profile_id: profile_id.to_string(),
        id,
    }
}

/// Run one command on the live connection for `profile_id`, if there is one.
/// `None` means "no live session" — the caller should connect on its own.
pub async fn run(profile_id: &str, command: &str, timeout: Duration) -> Option<Result<Value, String>> {
    let (tx, rx) = oneshot::channel();
    {
        let reg = registry().lock().unwrap();
        let entry = reg.get(profile_id)?;
        if entry
            .tx
            .send(ExecJob {
                command: command.to_string(),
                reply: tx,
            })
            .is_err()
        {
            // Worker is gone; treat as no live session.
            return None;
        }
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

async fn worker(
    profile_id: String,
    handle: Handle<ClientHandler>,
    mut rx: mpsc::UnboundedReceiver<ExecJob>,
) {
    while let Some(job) = rx.recv().await {
        let result =
            crate::modes::workspace::mcp::exec_on_handle(&handle, &job.command).await;
        let _ = job.reply.send(result);
    }
}

/// Whether a live connection is parked for this profile (diagnostics/tests).
pub fn has(profile_id: &str) -> bool {
    registry().lock().unwrap().contains_key(profile_id)
}