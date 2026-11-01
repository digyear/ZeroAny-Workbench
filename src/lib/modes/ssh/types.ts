// SSH profile shape — mirrors the Rust `SshProfile` struct exactly.
// Tauri v2 returns Rust `Option<T>` as `T | null`; timestamps come back as
// ISO-8601 strings from the Rust serializer.

export type SshAuthType = 'key' | 'password' | 'agent' | 'interactive';

/** Payload emitted by the Rust auth flow when the SSH server requests one
 *  or more keyboard-interactive prompts (PAM). The frontend opens a modal
 *  collecting the answers and submits them via `ssh_submit_auth_prompts`. */
export interface SshAuthPromptsPayload {
  requestId: string;
  name: string;
  instructions: string;
  prompts: { prompt: string; echo: boolean }[];
}

// IMPORTANT: Rust SshProfile uses `#[serde(rename_all = "camelCase")]`.
// All multi-word field names below MUST be camelCase to match the JSON wire
// format — using snake_case here makes the field undefined at runtime.
export interface SshProfile {
  id: string;
  name: string;
  host: string;
  port: number;
  username: string;
  authType: SshAuthType;
  keyPath: string | null;
  accentColor: string | null;
  lastUsedAt: string | null;
  createdAt: string;
  /** ID of another SshProfile to jump through (OpenSSH ProxyJump). NULL =
   *  direct connect. The connect path traverses jump pointers recursively. */
  jumpProfileId: string | null;
  /** Lines typed into the shell right after it opens — the answer to a
   *  bastion's asset menu (one per line). */
  postConnectInputs?: string | null;
  postConnectDelayMs?: number | null;
  /** OpenSSH ProxyCommand template with %h/%p/%r placeholders. Spawned as
   *  a subprocess (no shell) at connect time. NULL = no proxy command.
   *  When both this and jumpProfileId are set, this wins (matches OpenSSH). */
  proxyCommand: string | null;
}

export interface SshCreateProfileArgs {
  name: string;
  host: string;
  port: number;
  username: string;
  authType: SshAuthType;
  keyPath?: string | null;
  accentColor?: string | null;
  // Secret material — never persisted in the DB, sent straight to Keychain.
  secret?: string | null;
  passphrase?: string | null;
  jumpProfileId?: string | null;
  proxyCommand?: string | null;
}

export interface SshUpdateProfileArgs {
  id: string;
  name?: string;
  host?: string;
  port?: number;
  username?: string;
  authType?: SshAuthType;
  keyPath?: string | null;
  accentColor?: string | null;
  // If provided, replace the existing Keychain secret. If undefined, keep.
  secret?: string | null;
  passphrase?: string | null;
  /** Tri-state to match the Rust `Option<Option<String>>` shape:
   *  - undefined         → leave existing value alone
   *  - null              → clear (set DB column to NULL)
   *  - string            → set to value */
  jumpProfileId?: string | null;
  proxyCommand?: string | null;
}

// Channel payload from Rust — same shape used by Agent terminal.
export interface SshTerminalPayload {
  data?: string; // base64-encoded chunk
  exit?: boolean;
}

// One host parsed from ~/.ssh/config. `alreadyExists` means a profile
// with the same name (== ssh_config alias) is already in the DB and
// should be excluded from import (UI shows it greyed out).
export interface SshConfigHost {
  alias: string;
  hostname: string;
  user: string | null;
  port: number;
  identityFile: string | null;
  /** Raw ProxyCommand template if set in ssh_config. Stored verbatim with
   *  %h/%p/%r placeholders. NULL = no ProxyCommand. */
  proxyCommand: string | null;
  /** ProxyJump aliases in OpenSSH order (first = outermost jump). Resolved
   *  to profile IDs at import time. Empty = no ProxyJump. */
  proxyJumpAliases: string[];
  alreadyExists: boolean;
}
