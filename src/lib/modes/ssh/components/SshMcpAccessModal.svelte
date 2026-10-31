<script lang="ts">
  import Modal from '$lib/shared/primitives/Modal.svelte';
  import { mcpGrantsList, mcpGrantSet, type McpScope } from '../commands';
  import { showToast } from '$lib/shared/primitives/toast';
  import type { SshProfile } from '../types';

  /** Per-connection MCP exposure: whether an external agent may reach this
   *  host through the built-in MCP server, and what it is allowed to run.
   *  Enforced server-side in `modes::ssh::policy` — this modal only records
   *  the choice. */
  let { show = $bindable(false), profile = $bindable<SshProfile | null>(null), onsaved } = $props();

  let enabled = $state(false);
  let scope = $state<McpScope>('read_only');
  let loading = $state(false);
  let saving = $state(false);

  const SCOPES: { value: McpScope; title: string; hint: string }[] = [
    {
      value: 'read_only',
      title: 'Read only',
      hint: 'Logs, service status and file reads — journalctl, tail, cat, grep, ps, df, `systemctl status`, `docker logs`. Pipes allowed; no redirects, subshells or background jobs.',
    },
    {
      value: 'service_control',
      title: 'Service control',
      hint: 'Everything above, plus start / stop / restart / reload — `systemctl restart`, `docker restart`, `kubectl rollout`, `service <name> restart`.',
    },
    {
      value: 'full',
      title: 'Full shell',
      hint: 'Any command, including writes and installs. Only pick this for a host you would hand a shell to.',
    },
  ];

  $effect(() => {
    if (show && profile) void load(profile.id);
  });

  async function load(profileId: string) {
    loading = true;
    try {
      const grants = await mcpGrantsList('ssh');
      const grant = grants.find((g) => g.resourceId === profileId);
      enabled = grant?.enabled ?? false;
      scope = grant?.scope ?? 'read_only';
    } catch (e) {
      showToast(`Could not load MCP access: ${e}`, 'error');
    } finally {
      loading = false;
    }
  }

  async function save() {
    if (!profile) return;
    saving = true;
    try {
      await mcpGrantSet('ssh', profile.id, enabled, scope);
      showToast(
        enabled ? `"${profile.name}" is exposed to MCP (${scope.replace('_', ' ')})` : `"${profile.name}" is hidden from MCP`,
        'success',
      );
      onsaved?.();
      show = false;
    } catch (e) {
      showToast(String(e), 'error');
    } finally {
      saving = false;
    }
  }
</script>

<Modal bind:show title="MCP access" width="520px">
  {#if profile}
    <div class="mcpa-root">
      <div class="mcpa-sub">
        Let an external agent (Claude Code, Codex, Cursor, …) run commands on
        <strong>{profile.name}</strong> through the built-in MCP server.
      </div>

      <label class="mcpa-toggle">
        <input type="checkbox" bind:checked={enabled} disabled={loading} />
        <span>
          Expose this connection over MCP
          <span class="mcpa-hint">Off by default — nothing is shared unless you turn it on.</span>
        </span>
      </label>

      <div class="mcpa-scopes" class:disabled={!enabled}>
        {#each SCOPES as option (option.value)}
          <label class="mcpa-scope" class:active={scope === option.value}>
            <input
              type="radio"
              name="mcp-scope"
              value={option.value}
              bind:group={scope}
              disabled={!enabled}
            />
            <span class="mcpa-scope-body">
              <span class="mcpa-scope-title">{option.title}</span>
              <span class="mcpa-scope-hint">{option.hint}</span>
            </span>
          </label>
        {/each}
      </div>

      <div class="mcpa-note">
        The command check runs before anything reaches the host. It is a guardrail, not a
        sandbox: anything holding the MCP token can still use the scope you picked here.
      </div>

      <div class="mcpa-actions">
        <button class="mcpa-btn" onclick={() => (show = false)} disabled={saving}>Cancel</button>
        <button class="mcpa-btn primary" onclick={save} disabled={saving || loading}>
          {saving ? 'Saving…' : 'Save'}
        </button>
      </div>
    </div>
  {/if}
</Modal>

<style>
  .mcpa-root {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .mcpa-sub {
    font-size: 12.5px;
    line-height: 1.55;
    color: var(--muted-foreground);
  }
  .mcpa-toggle {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    padding: 10px 12px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--card);
    font-size: 13px;
    cursor: pointer;
  }
  .mcpa-toggle input {
    margin-top: 2px;
  }
  .mcpa-hint {
    display: block;
    font-size: 11.5px;
    color: var(--muted-foreground);
    margin-top: 2px;
  }
  .mcpa-scopes {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .mcpa-scopes.disabled {
    opacity: 0.5;
  }
  .mcpa-scope {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    padding: 10px 12px;
    border: 1px solid var(--border);
    border-radius: 8px;
    cursor: pointer;
  }
  .mcpa-scope.active {
    border-color: var(--accent);
  }
  .mcpa-scope input {
    margin-top: 2px;
  }
  .mcpa-scope-body {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .mcpa-scope-title {
    font-size: 13px;
    font-weight: 500;
  }
  .mcpa-scope-hint {
    font-size: 11.5px;
    line-height: 1.5;
    color: var(--muted-foreground);
  }
  .mcpa-note {
    font-size: 11.5px;
    line-height: 1.5;
    color: var(--muted-foreground);
    border-left: 2px solid var(--border);
    padding-left: 10px;
  }
  .mcpa-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .mcpa-btn {
    padding: 6px 14px;
    font-size: 12.5px;
    border-radius: 6px;
    border: 1px solid var(--border);
    background: var(--card);
    color: var(--foreground);
    cursor: pointer;
  }
  .mcpa-btn.primary {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--accent-foreground, #fff);
  }
  .mcpa-btn:disabled {
    opacity: 0.55;
    cursor: default;
  }
</style>