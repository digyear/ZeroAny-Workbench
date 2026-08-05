<script lang="ts">
  import { onMount } from 'svelte';
  import { agentSessions, activeAgentSession, agentContextUsage, agentSessionActivity, agentSessionAwaiting, agentDiscoveredSessions, agentSessionCenterOpen, loadAgentDiscoveredSessions, scanAgentDiscoveredSessions, loadAgentSessions } from '../stores';
  import { mode } from '$lib/stores/app';
  import { showContextMenu } from '$lib/shared/primitives/contextmenu';
  import type { AgentDiscoveredSession, AgentSession } from '../types';
  import { tabs, addTab, activateTab } from '$lib/shared/stores/tabs';
  import { get } from 'svelte/store';
  import { AGENT_EVENT } from '$lib/shared/constants/events';
  import ConfirmDialog from '$lib/shared/primitives/ConfirmDialog.svelte';
  import { agentAdoptDiscoveredSession, agentWorktreeIsDirty } from '../commands';
  import { PROVIDER_INSTALL_INFO, type AgentProvider } from '$lib/shared/agent/providers';
  import { showToast } from '$lib/shared/primitives/toast';
  import { friendlyError } from '$lib/utils/errors';

  interface Props {
    searchQuery?: string;
  }

  let { searchQuery = '' }: Props = $props();

  // Confirm dialog
  let confirmShow = $state(false);
  let confirmTitle = $state('');
  let confirmMessage = $state('');
  let confirmDanger = $state(false);
  let confirmText = $state('Confirm');
  let confirmAction: (() => Promise<void>) | null = $state(null);

  /** Set up + show the shared ConfirmDialog. Centralised so the Reset
   *  vs Delete branches don't have to duplicate state-write boilerplate. */
  function showConfirm(opts: { title: string; message: string; danger: boolean; confirmText: string; action: () => Promise<void> }) {
    confirmTitle = opts.title;
    confirmMessage = opts.message;
    confirmDanger = opts.danger;
    confirmText = opts.confirmText;
    confirmAction = opts.action;
    confirmShow = true;
  }

  // Collapsed project groups — persisted across app reloads so the user's
  // organisation choices survive a restart. localStorage (sync, instant)
  // is fine here since this is purely device-local UI state.
  const COLLAPSED_KEY = 'clauge.agent.collapsedProjects';
  function loadCollapsed(): Set<string> {
    try {
      const raw = localStorage.getItem(COLLAPSED_KEY);
      if (!raw) return new Set();
      const arr = JSON.parse(raw);
      if (Array.isArray(arr)) return new Set(arr.filter((v): v is string => typeof v === 'string'));
    } catch { /* corrupt entry — ignore and start fresh */ }
    return new Set();
  }
  function saveCollapsed(set: Set<string>) {
    try { localStorage.setItem(COLLAPSED_KEY, JSON.stringify([...set])); } catch { /* quota / private mode — silent */ }
  }
  let collapsedProjects = $state<Set<string>>(loadCollapsed());

  const purposeColors: Record<string, string> = {
    'Brainstorming': '#d2a8ff',
    'Development': '#3fb950',
    'Code Review': '#58a6ff',
    'PR Review': '#d29922',
    'Debugging': '#f85149',
    'Custom': '#8b949e',
  };

  function purposeColor(purpose: string): string {
    return purposeColors[purpose] ?? '#8b949e';
  }

  const filteredSessions = $derived(
    searchQuery
      ? $agentSessions.filter(s =>
          s.title.toLowerCase().includes(searchQuery.toLowerCase()) ||
          s.projectName.toLowerCase().includes(searchQuery.toLowerCase())
        )
      : $agentSessions
  );

  const filteredDiscoveredSessions = $derived.by(() => {
    const q = searchQuery.trim().toLowerCase();
    return $agentDiscoveredSessions
      .filter((s) => !s.adoptedAgentSessionId)
      .filter((s) => {
        if (!q) return true;
        return [s.provider, s.externalSessionId, s.projectName, s.projectPath, s.projectRoot, s.title, s.preview]
          .some((v) => (v ?? '').toLowerCase().includes(q));
      })
      .sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
  });

  const recentDiscoveredSessions = $derived(
    filteredDiscoveredSessions.slice(0, searchQuery.trim() ? 20 : 6),
  );

  let discoveredScanning = $state(false);
  let discoveredActionIds = $state<string[]>([]);

  function providerName(provider: string): string {
    return PROVIDER_INSTALL_INFO[provider as AgentProvider]?.name ?? provider;
  }

  function discoveredTitle(session: AgentDiscoveredSession): string {
    return session.title || session.preview || session.externalSessionId;
  }

  function discoveredProjectName(session: AgentDiscoveredSession): string {
    const root = session.projectRoot?.replace(/[\\/]+$/, '');
    if (root) return root.split(/[\\/]/).pop() || root;
    return session.projectName || session.projectPath || providerName(session.provider);
  }


  function isDiscoveredBusy(id: string): boolean {
    return discoveredActionIds.includes(id);
  }

  function setDiscoveredBusy(id: string, busy: boolean) {
    if (busy) {
      if (!discoveredActionIds.includes(id)) discoveredActionIds = [...discoveredActionIds, id];
    } else {
      discoveredActionIds = discoveredActionIds.filter((v) => v !== id);
    }
  }

  async function refreshDiscovered(scan = false) {
    if (scan) {
      if (discoveredScanning) return;
      discoveredScanning = true;
      try {
        const summary = await scanAgentDiscoveredSessions();
        if (summary.errors.length > 0) {
          console.warn('[agent-discovery] scan warnings', summary.errors);
        }
      } catch (e) {
        console.warn('[agent-discovery] scan failed', e);
      } finally {
        discoveredScanning = false;
      }
    }
    await loadAgentDiscoveredSessions();
  }

  async function handleRefreshDiscovered() {
    await refreshDiscovered(true);
  }

  async function handleAdoptDiscovered(session: AgentDiscoveredSession) {
    if (isDiscoveredBusy(session.id)) return;
    setDiscoveredBusy(session.id, true);
    try {
      const adopted = await agentAdoptDiscoveredSession(session.id);
      await loadAgentSessions();
      await loadAgentDiscoveredSessions();
      handleSelectSession(adopted);
      showToast('External session opened in ZeroAny Workbench', 'success');
    } catch (e) {
      showToast(`Open failed: ${friendlyError(e)}`, 'error');
    } finally {
      setDiscoveredBusy(session.id, false);
    }
  }


  onMount(() => {
    void refreshDiscovered(true);
  });

  const groupedByProject = $derived.by(() => {
    const groups = new Map<string, { name: string; sessions: AgentSession[] }>();
    for (const s of filteredSessions) {
      const key = s.projectRoot || s.projectPath || s.projectName || 'Untitled';
      if (!groups.has(key)) groups.set(key, { name: s.projectName || 'Untitled', sessions: [] });
      groups.get(key)!.sessions.push(s);
    }
    // Sort sessions within each group by lastUsedAt descending
    for (const [, group] of groups) {
      group.sessions.sort((a, b) => b.lastUsedAt.localeCompare(a.lastUsedAt));
    }
    return groups;
  });

  function handleNewSession() {
    window.dispatchEvent(new CustomEvent(AGENT_EVENT.NEW_SESSION));
  }

  function handleSelectSession(session: AgentSession) {
    agentSessionCenterOpen.set(false);
    // Open or focus the tab for this session
    const currentTabs = get(tabs);
    const existing = currentTabs.find(t => t.mode === 'agent' && t.key === session.id);
    if (existing) {
      activateTab(existing.id);
    } else {
      addTab(session.title, 'agent', session.id, purposeColor(session.purpose));
    }

    // Don't re-select the already active session
    if ($activeAgentSession?.id === session.id) return;
    activeAgentSession.set(session);
    window.dispatchEvent(new CustomEvent(AGENT_EVENT.SELECT_SESSION, { detail: { session } }));
  }

  function toggleProject(name: string) {
    const next = new Set(collapsedProjects);
    if (next.has(name)) {
      next.delete(name);
    } else {
      next.add(name);
    }
    collapsedProjects = next;
    saveCollapsed(next);
  }

  function contextPercent(sessionId: string): number | null {
    const usage = $agentContextUsage.get(sessionId);
    return usage ? Math.round(usage.fillPercent) : null;
  }

  function contextClass(pct: number): string {
    if (pct >= 85) return 'ctx-red';
    if (pct >= 70) return 'ctx-yellow';
    return 'ctx-green';
  }

  function activityStatus(sessionId: string): 'running' | 'done' | null {
    return $agentSessionActivity.get(sessionId) ?? null;
  }

  // Whether this session is currently waiting for the user's input. Shown for
  // every provider (unlike the claude-only activity icon).
  function isAwaiting(sessionId: string): boolean {
    return $agentSessionAwaiting.has(sessionId);
  }

  function relativeTime(iso: string): string {
    const diff = Date.now() - new Date(iso).getTime();
    if (diff < 60000) return 'just now';
    if (diff < 3600000) return `${Math.floor(diff / 60000)}m ago`;
    if (diff < 86400000) return `${Math.floor(diff / 3600000)}h ago`;
    return `${Math.floor(diff / 86400000)}d ago`;
  }

  function showSessionMenu(e: MouseEvent, session: AgentSession) {
    e.preventDefault();
    e.stopPropagation();

    showContextMenu(e.clientX, e.clientY, [
      {
        label: 'Edit',
        icon: '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M11 4H4a2 2 0 00-2 2v14a2 2 0 002 2h14a2 2 0 002-2v-7"/><path d="M18.5 2.5a2.121 2.121 0 013 3L12 15l-4 1 1-4 9.5-9.5z"/></svg>',
        action: () => {
          window.dispatchEvent(new CustomEvent(AGENT_EVENT.EDIT_SESSION, { detail: { session } }));
        },
      },
      {
        label: 'Reset Session',
        icon: '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M23 4v6h-6"/><path d="M1 20v-6h6"/><path d="M3.51 9a9 9 0 0114.85-3.36L23 10M1 14l4.64 4.36A9 9 0 0020.49 15"/></svg>',
        action: () => showConfirm({
          title: 'Reset Session',
          message: `Reset "${session.title}"? This will clear the Claude session ID and start fresh.`,
          danger: false,
          confirmText: 'Reset',
          action: async () => {
            window.dispatchEvent(new CustomEvent(AGENT_EVENT.RESET_SESSION, { detail: { session } }));
          },
        }),
      },
      { label: '', action: () => {}, separator: true },
      {
        label: 'Delete',
        icon: '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 01-2 2H7a2 2 0 01-2-2V6m3 0V4a2 2 0 012-2h4a2 2 0 012 2v2"/></svg>',
        danger: true,
        action: async () => {
          // Preflight so destructive deletion requires an explicit second
          // confirmation. The backend still enforces force=false/true, which
          // protects against changes appearing after this probe.
          let dirty = false;
          if (session.worktreePath) {
            try { dirty = await agentWorktreeIsDirty(session.worktreePath); } catch { /* probe error → treat as clean and let the normal flow run */ }
          }
          if (dirty) {
            showConfirm({
              title: 'Force Delete Session?',
              message: `"${session.title}" has uncommitted changes. Force deleting will permanently discard modified, staged, and untracked files in ${session.worktreePath}.`,
              danger: true,
              confirmText: 'Force Delete',
              action: async () => {
                window.dispatchEvent(new CustomEvent(AGENT_EVENT.DELETE_SESSION, { detail: { session, force: true } }));
              },
            });
            return;
          }
          showConfirm({
            title: 'Delete Session',
            message: `Delete "${session.title}"? This cannot be undone.`,
            danger: true,
            confirmText: 'Delete',
            action: async () => {
              window.dispatchEvent(new CustomEvent(AGENT_EVENT.DELETE_SESSION, { detail: { session, force: false } }));
            },
          });
        },
      },
    ]);
  }

  async function handleConfirmOk() {
    confirmShow = false;
    if (confirmAction) await confirmAction();
    confirmAction = null;
  }
</script>

<div class="agent-nav">
  {#if filteredSessions.length === 0}
    <div class="nav-empty">
      {#if searchQuery}
        <span>No results for "{searchQuery}"</span>
      {:else}
        <span>No sessions yet</span>
        <button class="nav-empty-btn" onclick={handleNewSession}>
          + New Session
        </button>
      {/if}
    </div>
  {:else}
    {#each [...groupedByProject] as [projectKey, group] (projectKey)}
      {@const isCollapsed = collapsedProjects.has(projectKey)}
      <div class="ncoll">
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div class="ncoll-hdr" onclick={() => toggleProject(projectKey)}>
          <div class="coll-icon coll-icon-accent">
            <svg viewBox="0 0 24 24"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>
          </div>
          <div class="ncoll-text">
            <div class="ncoll-row-top">
              <span class="ncoll-name">{group.name}</span>
            </div>
            <div class="ncoll-row-bot">
              <span class="ncoll-sub">{group.sessions.length} session{group.sessions.length === 1 ? '' : 's'}</span>
            </div>
          </div>
          <svg class="ncoll-arr" class:open={!isCollapsed} viewBox="0 0 24 24">
            <path d="M9 18l6-6-6-6" stroke="currentColor" fill="none" stroke-width="1.8" stroke-linecap="round"/>
          </svg>
        </div>

      {#if !isCollapsed}
        {#each group.sessions as session (session.id)}
          {@const pct = contextPercent(session.id)}
          {@const activity = activityStatus(session.id)}
          {@const awaiting = isAwaiting(session.id)}
          <button
            class="session-item"
            class:active={$activeAgentSession?.id === session.id}
            class:awaiting
            onclick={() => handleSelectSession(session)}
            oncontextmenu={(e) => showSessionMenu(e, session)}
          >
            <span class="session-icon">
              {#if awaiting}
                <span class="awaiting-dot" title="Waiting for your input"></span>
              {/if}
              {#if session.provider === 'codex'}
                <img src="/codex.svg" alt="Codex" width="22" height="22" class="session-icon-img codex" />
              {:else if session.provider === 'gemini'}
                <img src="/gemini.svg" alt="Antigravity" width="22" height="22" class="session-icon-img gemini" />
              {:else if session.provider === 'opencode'}
                <img src="/opencode-dark.svg" alt="OpenCode" width="22" height="22" class="session-icon-img opencode" />
              {:else if session.provider === 'hermes'}
                <img src="/hermes.png" alt="Hermes" width="22" height="22" class="session-icon-img hermes" />
              {:else if activity === 'running'}
                <img src="/code-in-action.svg" alt="Claude" width="36" height="26" />
              {:else}
                <img src="/code-no-action.svg" alt="Claude" width="22" height="22" />
              {/if}
            </span>
            <div class="session-body">
              <div class="session-row-top">
                <span class="session-title">{session.title}</span>
                {#if pct !== null}
                  <span class="ctx-badge {contextClass(pct)}" title="{pct}% context window used">{pct}%</span>
                {/if}
              </div>
              <div class="session-row-bot">
                <span class="purpose-badge" style="color:{purposeColor(session.purpose)};background:{purposeColor(session.purpose)}22">{session.purpose}</span>
                {#if session.worktreePath}
                  <span class="wt-badge" title="Isolated worktree: {session.worktreeBranch}">WT</span>
                {/if}
                <span class="session-time-spacer"></span>
                <span class="session-time">{relativeTime(session.lastUsedAt)}</span>
              </div>
            </div>
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <span
              class="session-ellipsis"
              role="button"
              tabindex="-1"
              title="More"
              onclick={(e) => { e.stopPropagation(); showSessionMenu(e, session); }}
            >
              <svg viewBox="0 0 24 24" fill="currentColor"><circle cx="12" cy="5" r="1.5"/><circle cx="12" cy="12" r="1.5"/><circle cx="12" cy="19" r="1.5"/></svg>
            </span>
          </button>
        {/each}
      {/if}
      </div>
    {/each}
  {/if}

  <div class="discovered-section">
    <div class="discovered-header">
      <div class="discovered-heading">
        <span>DISCOVERED</span>
        {#if filteredDiscoveredSessions.length > 0}
          <span class="discovered-count">{filteredDiscoveredSessions.length}</span>
        {/if}
      </div>
      <button class="discovered-refresh" title="Refresh discovered sessions" disabled={discoveredScanning} onclick={handleRefreshDiscovered}>
        <svg class:spin={discoveredScanning} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
          <path d="M21 12a9 9 0 1 1-2.64-6.36"/>
          <path d="M21 3v6h-6"/>
        </svg>
      </button>
    </div>

    {#if filteredDiscoveredSessions.length === 0}
      <div class="discovered-empty">{discoveredScanning ? 'Scanning...' : 'No external sessions found'}</div>
    {:else}
      <div class="discovered-recent-label">{searchQuery.trim() ? 'MATCHES' : 'RECENT EXTERNAL'}</div>
      {#each recentDiscoveredSessions as discovered (discovered.id)}
        {@const busy = isDiscoveredBusy(discovered.id)}
        <button class="discovered-recent" disabled={busy} title={discovered.projectPath || ''} onclick={() => handleAdoptDiscovered(discovered)}>
          <span class="discovered-recent-icon">
            {#if discovered.provider === 'codex'}
              <img src="/codex.svg" alt="Codex" />
            {:else if discovered.provider === 'gemini'}
              <img src="/gemini.svg" alt="Antigravity" />
            {:else if discovered.provider === 'opencode'}
              <img src="/opencode-dark.svg" alt="OpenCode" />
            {:else if discovered.provider === 'hermes'}
              <img src="/hermes.png" alt="Hermes" />
            {:else}
              <img src="/code-no-action.svg" alt="Claude" />
            {/if}
          </span>
          <span class="discovered-recent-copy">
            <strong>{discoveredTitle(discovered)}</strong>
            <small>{discoveredProjectName(discovered)} · {providerName(discovered.provider)}</small>
          </span>
          <time>{relativeTime(discovered.updatedAt)}</time>
        </button>
      {/each}
      <button class="discovered-browse" onclick={() => agentSessionCenterOpen.set(true)}>
        <span>Browse all {filteredDiscoveredSessions.length} sessions</span>
        <svg viewBox="0 0 24 24"><path d="m9 18 6-6-6-6"/></svg>
      </button>
    {/if}
  </div>
</div>

<!-- Confirm Dialog — shared primitive across all modes (header bar, body,
     footer with proper dividers; teleports to body so nav stacking
     contexts can't clip it). -->
<ConfirmDialog
  bind:show={confirmShow}
  title={confirmTitle}
  message={confirmMessage}
  confirmText={confirmText}
  confirmColor={confirmDanger ? 'var(--err)' : 'var(--acc)'}
  onconfirm={handleConfirmOk}
/>

<style>
  .agent-nav {
    display: flex;
    flex-direction: column;
    min-height: 0;
    overflow-y: auto;
    overflow-x: hidden;
  }
  .agent-nav::-webkit-scrollbar { width: 3px; }
  .agent-nav::-webkit-scrollbar-thumb { background: var(--b1); border-radius: 2px; }

  .nav-empty {
    padding: 24px 12px;
    color: var(--t3);
    font-size: 12px;
    font-family: var(--ui);
    text-align: center;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
  }
  .nav-empty-btn {
    padding: 5px 12px;
    border-radius: 5px;
    border: 1px solid var(--b1);
    background: transparent;
    color: var(--t2);
    font-size: 11px;
    font-family: var(--ui);
    cursor: pointer;
    transition: background 0.12s, border-color 0.12s, color 0.12s;
  }
  .nav-empty-btn:hover { background: var(--c); border-color: var(--b2); color: var(--t1); }

  .ncoll {
    border-bottom: 1px solid var(--b1);
  }
  .ncoll-hdr {
    min-height: 44px;
    padding: 6px 8px;
    display: flex;
    align-items: center;
    gap: 8px;
    cursor: pointer;
    transition: background 0.1s;
    user-select: none;
  }
  .ncoll-hdr:hover { background: var(--n2); }
  .ncoll-text {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .ncoll-row-top, .ncoll-row-bot {
    display: flex;
    align-items: center;
    min-width: 0;
  }
  .ncoll-name {
    font-size: 12.5px;
    font-weight: 500;
    color: var(--t2);
    flex: 1;
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .ncoll-sub {
    font-size: 10.5px;
    font-family: var(--mono);
    color: var(--t4);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .coll-icon {
    width: 22px;
    height: 22px;
    border-radius: 5px;
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }
  .coll-icon-accent {
    background: color-mix(in srgb, var(--acc) 18%, transparent);
    color: var(--acc);
  }
  .coll-icon svg {
    width: 13px;
    height: 13px;
    stroke: currentColor;
    fill: none;
    stroke-width: 1.8;
    stroke-linecap: round;
  }
  .ncoll-arr {
    width: 12px;
    height: 12px;
    stroke: var(--t3);
    fill: none;
    stroke-width: 1.8;
    stroke-linecap: round;
    flex-shrink: 0;
    transition: transform 0.18s;
  }
  .ncoll-arr.open { transform: rotate(90deg); }

  /* Session item */
  .session-item {
    width: 100%;
    min-height: 46px;
    border: none;
    background: transparent;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 8px 5px 8px;
    cursor: pointer;
    transition: background 0.08s;
    text-align: left;
    position: relative;
    /* Divider between sessions inside a project group. The :last-child
       rule below clears it on the trailing item so groups don't run into
       the project header underneath. */
    border-bottom: 1px solid var(--b-subtle);
  }
  .session-item:last-child { border-bottom: none; }
  .session-item:hover { background: var(--c); }
  .session-item.active { background: color-mix(in srgb, var(--agent, var(--acc)) 10%, transparent); }

  .session-icon {
    width: 28px;
    height: 28px;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    position: relative;
  }
  .session-icon img {
    display: block;
  }

  /* "Waiting for input" adornment — a small pulsing accent dot overlaid on the
     top-right of the provider icon. Provider-agnostic (shows for claude/codex/
     gemini/opencode alike) and laid on top of the existing icon so it never
     disturbs the icon / activity layout. */
  .awaiting-dot {
    position: absolute;
    top: -1px;
    right: -1px;
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--acc, #d29922);
    box-shadow: 0 0 0 2px var(--n0, var(--bg, #0d0d18));
    animation: awaiting-pulse 1.4s ease-in-out infinite;
    pointer-events: none;
    z-index: 1;
  }
  @keyframes awaiting-pulse {
    0%, 100% { opacity: 1; transform: scale(1); }
    50% { opacity: 0.45; transform: scale(0.82); }
  }

  .session-body {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .session-row-top {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .session-title {
    font-family: var(--ui);
    font-size: 12px;
    color: var(--t2);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1;
    min-width: 0;
  }
  .session-item.active .session-title { color: var(--t1); }
  .session-item.awaiting .session-title { color: var(--t1); font-weight: 600; }

  .session-row-bot {
    display: flex;
    align-items: center;
    gap: 5px;
  }

  .purpose-badge {
    font-size: 10px;
    font-family: var(--ui);
    font-weight: 600;
    padding: 1px 6px;
    border-radius: 4px;
    white-space: nowrap;
    line-height: 1.4;
  }

  /* Per-provider session-row icon — non-Claude brand marks. The Codex
   * mark is mono so it picks up app text colour; OpenCode's brand
   * stripes are baked into its SVG. */
  .session-icon-img.codex { color: var(--t1); }

  .session-time-spacer {
    flex: 1;
  }

  .session-time {
    font-family: var(--ui);
    font-size: 9px;
    color: var(--t4);
    white-space: nowrap;
  }

  /* WT (worktree) is a single fixed identity tag — like the brand badges
     (Postgres, S3, Mongo). Kept theme-independent so it doesn't follow the
     user's accent. */
  .wt-badge {
    font-size: 8px;
    font-family: var(--mono);
    font-weight: 700;
    color: #7c5cf8;
    background: rgba(124, 92, 248, 0.12);
    padding: 1px 4px;
    border-radius: 3px;
    flex-shrink: 0;
  }

  /* Context usage badge */
  .ctx-badge {
    font-size: 9px;
    font-family: var(--mono);
    font-weight: 600;
    padding: 1px 5px;
    border-radius: 8px;
    flex-shrink: 0;
  }
  .ctx-green { color: #3fb950; background: rgba(63, 185, 80, 0.12); }
  .ctx-yellow { color: #d29922; background: rgba(210, 153, 34, 0.12); }
  .ctx-red { color: #f85149; background: rgba(248, 81, 73, 0.12); animation: ctx-pulse 1.5s ease-in-out infinite; }
  @keyframes ctx-pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.6; }
  }

  /* Ellipsis button */
  .session-ellipsis {
    width: 18px; height: 18px;
    display: none; align-items: center; justify-content: center;
    border-radius: 3px; flex-shrink: 0; cursor: default;
    color: var(--t3); transition: background 0.1s, color 0.1s;
  }
  .session-ellipsis svg { width: 14px; height: 14px; }
  .session-item:hover .session-ellipsis { display: flex; }
  .session-ellipsis:hover { background: var(--surface-hover); color: var(--t1); }

  .discovered-section {
    border-top: 1px solid var(--b1);
    margin-top: 4px;
    padding-bottom: 8px;
  }
  .discovered-header {
    min-height: 34px;
    padding: 7px 8px 5px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  .discovered-heading {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    font-family: var(--ui);
    font-size: 10px;
    font-weight: 700;
    color: var(--t4);
    letter-spacing: 0.04em;
  }
  .discovered-count {
    font-family: var(--mono);
    font-size: 9px;
    font-weight: 600;
    color: var(--t3);
    background: var(--n2);
    border: 1px solid var(--b1);
    border-radius: 4px;
    padding: 0 4px;
    line-height: 15px;
  }
  .discovered-refresh {
    width: 24px;
    height: 24px;
    border: 1px solid var(--b1);
    border-radius: 5px;
    background: transparent;
    color: var(--t3);
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
  }
  .discovered-refresh:hover:not(:disabled) { background: var(--c); color: var(--t1); }
  .discovered-refresh:disabled { opacity: 0.6; cursor: default; }
  .discovered-refresh svg { width: 13px; height: 13px; }
  .discovered-refresh svg.spin { animation: discovered-spin 1s linear infinite; }
  @keyframes discovered-spin { to { transform: rotate(360deg); } }

  .discovered-empty {
    padding: 10px 12px 14px;
    color: var(--t4);
    font-size: 11px;
    font-family: var(--ui);
  }
  .discovered-recent-label {
    padding: 3px 9px 4px;
    color: var(--t4);
    font: 8px var(--ui);
    font-weight: 700;
    letter-spacing: 0.06em;
  }
  .discovered-recent {
    width: 100%;
    min-height: 43px;
    padding: 6px 8px;
    border: 0;
    border-top: 1px solid color-mix(in srgb, var(--b-subtle) 70%, transparent);
    background: transparent;
    color: var(--t2);
    display: flex;
    align-items: center;
    gap: 7px;
    text-align: left;
    cursor: pointer;
  }
  .discovered-recent:hover { background: var(--c); }
  .discovered-recent:disabled { opacity: .55; cursor: default; }
  .discovered-recent-icon { width: 22px; height: 22px; flex:none; display:grid; place-items:center; }
  .discovered-recent-icon img { width: 19px; height: 19px; object-fit: contain; }
  .discovered-recent-copy { flex:1; min-width:0; display:flex; flex-direction:column; gap:2px; }
  .discovered-recent-copy strong,
  .discovered-recent-copy small { overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  .discovered-recent-copy strong { font: 10.5px var(--ui); font-weight:550; color:var(--t2); }
  .discovered-recent-copy small { font: 8.5px var(--ui); color:var(--t4); }
  .discovered-recent time { flex:none; color:var(--t4); font:8.5px var(--ui); }
  .discovered-browse {
    width: calc(100% - 16px);
    height: 30px;
    margin: 7px 8px 3px;
    padding: 0 9px;
    border: 1px solid color-mix(in srgb, var(--acc) 28%, var(--b1));
    border-radius: 5px;
    background: color-mix(in srgb, var(--acc) 7%, var(--n2));
    color: var(--acc);
    display:flex;
    align-items:center;
    justify-content:space-between;
    font:10px var(--ui);
    cursor:pointer;
  }
  .discovered-browse:hover { background:color-mix(in srgb, var(--acc) 13%, var(--n2)); }
  .discovered-browse svg { width:13px; height:13px; fill:none; stroke:currentColor; stroke-width:1.8; }


  /* Claude plan badge */
  .plan-badge-row {
    padding: 8px 10px 4px;
    margin-top: auto;
  }
  .plan-badge {
    font-size: 9px;
    font-family: var(--ui);
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--acc, #6366f1);
    background: rgba(99, 102, 241, 0.10);
    padding: 2px 7px;
    border-radius: 4px;
  }
</style>
