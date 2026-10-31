// MCP server for the Workspace mode. Lets external agents (Claude
// Code today; Codex / Gemini later) drive Notes + Boards via the
// Model Context Protocol over local HTTP.
//
// Transport: streamable HTTP (POST /mcp returns a JSON-RPC response).
// We don't yet support the SSE side of streamable-http — Claude Code's
// tool-call path doesn't require it, and a single-response POST is
// strictly simpler. Bearer-token auth keeps random local processes
// out; the token is auto-generated on first start and persisted in
// settings (rotatable from the UI).
//
// Layout:
//   server.rs   — McpHandle, McpAppState, start(), HTTP handler
//   actor.rs    — actor_from_request (UA → attribution slug)
//   tools.rs    — tool_descriptors() (the JSON-RPC schema list)
//   dispatch.rs — tools/call dispatch + per-tool helpers
//
// Every mutation runs with actor=<agent slug> so the existing
// attribution + Inbox machinery surfaces agent activity automatically.

mod actor;
mod dispatch;
mod server;
mod ssh;
mod tools;

pub use server::{start, McpHandle};

/// Re-exported so the SSH regression tests can drive the exec collector
/// against a fixture session.
// Handle-level exec: used by the live-session worker (MCP on a reused
// connection) and by the SSH fixture tests.
pub(crate) use ssh::{exec_on_handle, push_capped, split_sentinel_output};
