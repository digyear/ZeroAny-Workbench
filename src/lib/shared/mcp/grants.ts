import { invoke } from '@tauri-apps/api/core';

/** Which saved connections an external agent may reach through the built-in
 *  MCP server, and how far it may go on each.
 *
 *  `resourceKind` is a string on purpose: 'ssh' today, 'sql' next — the
 *  backend table and both commands are kind-agnostic, so adding a mode
 *  means adding rows here, not a new permission model. */

export type McpScope = 'read_only' | 'service_control' | 'full';

export interface McpGrant {
  resourceKind: string;
  resourceId: string;
  enabled: boolean;
  scope: McpScope;
  createdAt: string;
  updatedAt: string;
}

export const mcpGrantsList = (resourceKind: string) =>
  invoke<McpGrant[]>('mcp_grants_list', { resourceKind });

export const mcpGrantSet = (
  resourceKind: string,
  resourceId: string,
  enabled: boolean,
  scope: McpScope,
) => invoke<void>('mcp_grant_set', { resourceKind, resourceId, enabled, scope });

/** Scope copy shared by every surface that offers the choice. `hint` is the
 *  long form shown under a radio; `short` is what fits in a row. */
export const MCP_SCOPES: { value: McpScope; label: string; short: string; hint: string }[] = [
  {
    value: 'read_only',
    label: 'Read only',
    short: 'read-only',
    hint: 'Logs, service status and file reads — journalctl, tail, cat, grep, ps, df, `systemctl status`, `docker logs`. Pipes allowed; no redirects, subshells or background jobs.',
  },
  {
    value: 'service_control',
    label: 'Service control',
    short: 'service control',
    hint: 'Everything above, plus start / stop / restart / reload — `systemctl restart`, `docker restart`, `kubectl rollout`, `service <name> restart`.',
  },
  {
    value: 'full',
    label: 'Full shell',
    short: 'full shell',
    hint: 'Any command, including writes and installs. Only pick this for a host you would hand a shell to.',
  },
];