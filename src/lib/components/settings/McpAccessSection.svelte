<script lang="ts">
  import { onMount } from "svelte";
  import { sshListProfiles } from "$lib/modes/ssh/commands";
  import type { SshProfile } from "$lib/modes/ssh/types";
  import { mcpGrantsList, mcpGrantSet, MCP_SCOPES, type McpGrant, type McpScope } from "$lib/shared/mcp/grants";
  import { showToast } from "$lib/shared/primitives/toast";

  /** One card, one list: every connection an agent could reach, with the
   *  scope it is allowed. This is the single place exposure is managed —
   *  nothing is configured from inside the SSH panel. */

  let profiles = $state<SshProfile[]>([]);
  let grants = $state<Record<string, McpGrant>>({});
  let loading = $state(true);
  let busy = $state<string | null>(null);

  const exposedCount = $derived(Object.values(grants).filter((g) => g.enabled).length);

  onMount(() => {
    void load();
  });

  async function load() {
    loading = true;
    try {
      const [list, rows] = await Promise.all([sshListProfiles(), mcpGrantsList("ssh")]);
      profiles = list;
      grants = Object.fromEntries(rows.map((g) => [g.resourceId, g]));
    } catch (e) {
      showToast(`Could not load MCP access: ${e}`, "error");
    } finally {
      loading = false;
    }
  }

  function grantOf(id: string): McpGrant | undefined {
    return grants[id];
  }

  async function save(profile: SshProfile, enabled: boolean, scope: McpScope) {
    busy = profile.id;
    try {
      await mcpGrantSet("ssh", profile.id, enabled, scope);
      grants = {
        ...grants,
        [profile.id]: {
          resourceKind: "ssh",
          resourceId: profile.id,
          enabled,
          scope,
          createdAt: grants[profile.id]?.createdAt ?? "",
          updatedAt: "",
        },
      };
      showToast(
        enabled
          ? `${profile.name} exposed to MCP (${MCP_SCOPES.find((s) => s.value === scope)?.short})`
          : `${profile.name} is hidden from MCP`,
        "success",
      );
    } catch (e) {
      showToast(String(e), "error");
      // Re-read the truth rather than leave the switch lying.
      void load();
    } finally {
      busy = null;
    }
  }

  function toggle(profile: SshProfile, next: boolean) {
    const scope = grantOf(profile.id)?.scope ?? "read_only";
    void save(profile, next, scope);
  }

  function setScope(profile: SshProfile, scope: McpScope) {
    void save(profile, true, scope);
  }
</script>

<section class="stg-card">
  <header class="stg-card-hd">
    <span class="stg-card-icon" aria-hidden="true">
      <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M9 2v6M15 2v6" /><path d="M6 8h12v5a6 6 0 0 1-12 0V8Z" /><path d="M12 19v3" />
      </svg>
    </span>
    <div class="stg-card-titles">
      <h3 class="stg-card-title">Exposed connections</h3>
      <p class="stg-card-sub">
        Any MCP client holding your token can reach the connections you enable here, with
        the scope you pick. Everything else is invisible to it.
      </p>
    </div>
    <span class="mcp-pill" class:on={exposedCount > 0}>
      {exposedCount} of {profiles.length} exposed
    </span>
  </header>

  <div class="stg-card-body">
    {#if loading}
      <div class="mcp-empty">Loading…</div>
    {:else if profiles.length === 0}
      <div class="mcp-empty">No SSH connections yet — add one in the SSH panel first.</div>
    {:else}
      {#each profiles as profile (profile.id)}
        {@const grant = grantOf(profile.id)}
        {@const enabled = grant?.enabled ?? false}
        {@const scope = grant?.scope ?? "read_only"}
        <div class="stg-card-row mcp-row">
          <div class="mcp-who">
            <div class="mcp-line">
              <span class="mcp-name">{profile.name}</span>
              {#if enabled}
                <span class="mcp-tag" class:full={scope === "full"}>
                  {MCP_SCOPES.find((s) => s.value === scope)?.short}
                </span>
              {/if}
            </div>
            <div class="mcp-host">{profile.username}@{profile.host}:{profile.port}</div>
            {#if enabled}
              <div class="mcp-hint">
                {MCP_SCOPES.find((s) => s.value === scope)?.hint}
              </div>
            {/if}
          </div>
          <div class="mcp-ctl">
            <select
              class="stg-select"
              value={scope}
              disabled={!enabled || busy === profile.id}
              onchange={(e) => setScope(profile, e.currentTarget.value as McpScope)}
              title={enabled ? "What the agent may run" : "Enable MCP access first"}
            >
              {#each MCP_SCOPES as option (option.value)}
                <option value={option.value}>{option.label}</option>
              {/each}
            </select>
            <label class="stg-toggle" title={enabled ? "Hide from MCP" : "Expose over MCP"}>
              <input
                type="checkbox"
                checked={enabled}
                disabled={busy === profile.id}
                onchange={(e) => toggle(profile, e.currentTarget.checked)}
              />
              <span class="stg-toggle-slider"></span>
            </label>
          </div>
        </div>
      {/each}
    {/if}

    <p class="mcp-foot">
      Commands are checked against the host's scope before they run — a refused command comes
      back with what <em>is</em> allowed, so the agent can correct itself. This is a guardrail,
      not a sandbox: a full-shell grant is a shell. Database connections will appear in this
      same list when they gain MCP tools.
    </p>
  </div>
</section>

<style>
  /* ---- Shared settings card language (mirrored locally: the parent's
          rules are scoped to SettingsModal.svelte) ---- */
  .stg-card {
    border: 1px solid var(--b1);
    border-radius: 10px;
    background: linear-gradient(180deg, rgba(255, 255, 255, 0.025) 0%, rgba(255, 255, 255, 0.005) 100%);
    overflow: hidden;
  }
  .stg-card-hd {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    padding: 14px 16px 12px;
  }
  .stg-card-hd > :last-child:not(.stg-card-titles):not(.stg-card-icon) {
    margin-left: auto;
    flex-shrink: 0;
  }
  .stg-card-icon {
    flex-shrink: 0;
    width: 28px;
    height: 28px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: 8px;
    border: 1px solid var(--b1);
    background: var(--surface-hover);
    color: var(--t2);
  }
  .stg-card-titles {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .stg-card-title {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
    color: var(--t1);
    font-family: var(--ui);
  }
  .stg-card-sub {
    margin: 0;
    font-size: 11.5px;
    line-height: 1.5;
    color: var(--t3);
    font-family: var(--ui);
    max-width: 420px;
  }
  .stg-card-body {
    padding: 4px 16px 14px;
    display: flex;
    flex-direction: column;
  }
  .stg-card-row {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 0;
    border-top: 1px solid var(--b-subtle, rgba(255, 255, 255, 0.05));
  }
  .stg-card-row:first-child {
    border-top: none;
  }
  .stg-select {
    font-family: var(--ui);
    font-size: 11.5px;
    color: var(--t1);
    background: var(--surface-hover);
    border: 1px solid var(--b1);
    border-radius: 7px;
    padding: 4px 8px;
    cursor: pointer;
    flex-shrink: 0;
  }
  .stg-select:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .stg-toggle {
    position: relative;
    display: inline-block;
    width: 36px;
    height: 20px;
    flex-shrink: 0;
    cursor: pointer;
  }
  .stg-toggle input {
    opacity: 0;
    width: 0;
    height: 0;
    position: absolute;
  }
  .stg-toggle-slider {
    position: absolute;
    inset: 0;
    background: var(--b1);
    border-radius: 10px;
    transition: background 0.2s;
  }
  .stg-toggle-slider::after {
    content: "";
    position: absolute;
    width: 16px;
    height: 16px;
    left: 2px;
    top: 2px;
    background: #fff;
    border-radius: 50%;
    transition: transform 0.2s, background 0.2s;
  }
  .stg-toggle input:checked + .stg-toggle-slider {
    background: var(--acc);
  }
  .stg-toggle input:checked + .stg-toggle-slider::after {
    left: 18px;
  }
  .stg-toggle input:disabled + .stg-toggle-slider {
    opacity: 0.5;
  }

  /* ---- MCP-specific ---- */
  .mcp-pill {
    font-family: var(--ui);
    font-size: 10.5px;
    padding: 2px 8px;
    border-radius: 999px;
    border: 1px solid var(--b1);
    color: var(--t3);
    white-space: nowrap;
  }
  .mcp-pill.on {
    color: var(--acc);
    border-color: color-mix(in srgb, var(--acc) 45%, transparent);
    background: color-mix(in srgb, var(--acc) 10%, transparent);
  }
  .mcp-row {
    align-items: center;
  }
  .mcp-who {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .mcp-line {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .mcp-name {
    font-family: var(--ui);
    font-size: 12.5px;
    font-weight: 500;
    color: var(--t1);
  }
  .mcp-tag {
    font-family: var(--ui);
    font-size: 9.5px;
    line-height: 14px;
    padding: 0 5px;
    border-radius: 4px;
    color: var(--acc);
    border: 1px solid color-mix(in srgb, var(--acc) 45%, transparent);
    background: color-mix(in srgb, var(--acc) 10%, transparent);
  }
  .mcp-tag.full {
    color: #e0904a;
    border-color: rgba(224, 144, 74, 0.5);
    background: rgba(224, 144, 74, 0.12);
  }
  .mcp-host {
    font-family: var(--mono, var(--ui));
    font-size: 11px;
    color: var(--t3);
  }
  .mcp-hint {
    font-family: var(--ui);
    font-size: 10.5px;
    line-height: 1.45;
    color: var(--t3);
    max-width: 460px;
    margin-top: 2px;
  }
  .mcp-ctl {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-shrink: 0;
  }
  .mcp-empty {
    padding: 10px 0;
    font-family: var(--ui);
    font-size: 11.5px;
    color: var(--t3);
  }
  .mcp-foot {
    margin: 10px 0 0;
    padding-top: 10px;
    border-top: 1px solid var(--b-subtle, rgba(255, 255, 255, 0.05));
    font-family: var(--ui);
    font-size: 10.5px;
    line-height: 1.55;
    color: var(--t3);
  }
</style>