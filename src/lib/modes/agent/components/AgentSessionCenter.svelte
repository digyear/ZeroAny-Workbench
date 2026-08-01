<script lang="ts">
  import { onMount } from 'svelte';
  import { get } from 'svelte/store';
  import type { AgentDiscoveredSession, AgentProvider, AgentSession } from '../types';
  import {
    agentAdoptDiscoveredSession,
    agentFsReveal,
    agentHideDiscoveredSession,
    agentListDiscoveredSessions,
    agentResolveProjectRoots,
    agentScanDiscoveredSessions,
    agentUnhideDiscoveredSession,
  } from '../commands';
  import {
    activeAgentSession,
    agentSessionCenterOpen,
    loadAgentDiscoveredSessions,
    loadAgentSessions,
  } from '../stores';
  import { addTab, activateTab, tabs } from '$lib/shared/stores/tabs';
  import { showToast } from '$lib/shared/primitives/toast';
  import { friendlyError } from '$lib/utils/errors';

  type StatusFilter = 'external' | 'managed' | 'hidden' | 'all';
  type RangeFilter = '7' | '30' | '90' | 'all';

  interface ProjectGroup {
    key: string;
    name: string;
    path: string;
    sessions: AgentDiscoveredSession[];
    latest: string;
    providers: string[];
    unscoped: boolean;
  }

  let catalog = $state<AgentDiscoveredSession[]>([]);
  let projectRoots = $state<Record<string, string>>({});
  let loading = $state(true);
  let refreshing = $state(false);
  let busyIds = $state<string[]>([]);
  let query = $state('');
  let provider = $state<'all' | AgentProvider>('all');
  let range = $state<RangeFilter>('30');
  let status = $state<StatusFilter>('external');
  let selectedProject = $state('all');
  let selectedIds = $state<string[]>([]);
  let visibleLimit = $state(100);

  function sessionStatus(session: AgentDiscoveredSession): Exclude<StatusFilter, 'all'> {
    if (session.hidden) return 'hidden';
    if (session.adoptedAgentSessionId) return 'managed';
    return 'external';
  }

  function canonicalProjectPath(session: AgentDiscoveredSession): string {
    const path = session.projectPath?.trim();
    const persistedRoot = session.projectRoot?.trim();
    if (!path && !persistedRoot) return '__unscoped__';
    const root = persistedRoot || (path ? projectRoots[path] : '') || path || '';
    return isBroadDirectory(root) ? '__unscoped__' : root;
  }

  function pathName(path: string): string {
    if (path === '__unscoped__') return 'Unscoped';
    const normalized = path.replace(/[\\/]+$/, '');
    return normalized.split(/[\\/]/).pop() || normalized;
  }

  function isBroadDirectory(path: string): boolean {
    if (path === '__unscoped__') return true;
    const normalized = path.replace(/[\\/]+$/, '');
    const parts = normalized.split(/[\\/]/).filter(Boolean);
    return parts.length <= 2 || ['workspace', 'workspaces', 'projects', 'src'].includes((parts.at(-1) || '').toLowerCase());
  }

  function matchesRange(session: AgentDiscoveredSession): boolean {
    if (range === 'all') return true;
    const timestamp = new Date(session.updatedAt).getTime();
    if (!Number.isFinite(timestamp)) return true;
    return Date.now() - timestamp <= Number(range) * 86_400_000;
  }

  const baseFiltered = $derived.by(() => {
    const needle = query.trim().toLowerCase();
    return catalog
      .filter((session) => provider === 'all' || session.provider === provider)
      .filter((session) => status === 'all' || sessionStatus(session) === status)
      .filter(matchesRange)
      .filter((session) => {
        if (!needle) return true;
        const root = canonicalProjectPath(session);
        return [
          session.title,
          session.preview,
          session.externalSessionId,
          session.projectName,
          session.projectPath,
          session.projectRoot,
          root,
          session.provider,
          session.profile,
        ].some((value) => (value || '').toLowerCase().includes(needle));
      })
      .sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
  });

  const projectGroups = $derived.by(() => {
    const groups = new Map<string, ProjectGroup>();
    for (const session of baseFiltered) {
      const path = canonicalProjectPath(session);
      const key = path;
      let group = groups.get(key);
      if (!group) {
        group = {
          key,
          name: isBroadDirectory(path) ? 'Unscoped' : pathName(path),
          path,
          sessions: [],
          latest: session.updatedAt,
          providers: [],
          unscoped: isBroadDirectory(path),
        };
        groups.set(key, group);
      }
      group.sessions.push(session);
      if (!group.providers.includes(session.provider)) group.providers.push(session.provider);
      if (session.updatedAt > group.latest) group.latest = session.updatedAt;
    }
    return [...groups.values()].sort((a, b) => {
      if (a.unscoped !== b.unscoped) return a.unscoped ? 1 : -1;
      return b.latest.localeCompare(a.latest);
    });
  });

  const filteredSessions = $derived(
    selectedProject === 'all'
      ? baseFiltered
      : baseFiltered.filter((session) => canonicalProjectPath(session) === selectedProject),
  );
  const visibleSessions = $derived(filteredSessions.slice(0, visibleLimit));
  const selectedVisibleCount = $derived(visibleSessions.filter((session) => selectedIds.includes(session.id)).length);
  const allVisibleSelected = $derived(visibleSessions.length > 0 && selectedVisibleCount === visibleSessions.length);

  const statusCounts = $derived.by(() => ({
    external: catalog.filter((session) => sessionStatus(session) === 'external').length,
    managed: catalog.filter((session) => sessionStatus(session) === 'managed').length,
    hidden: catalog.filter((session) => sessionStatus(session) === 'hidden').length,
  }));

  function providerName(value: string): string {
    return value === 'claude' ? 'Claude'
      : value === 'codex' ? 'Codex'
      : value === 'opencode' ? 'OpenCode'
      : value === 'hermes' ? 'Hermes'
      : value === 'gemini' ? 'Antigravity'
      : value;
  }

  function providerIcon(value: string): string {
    return value === 'codex' ? '/codex.svg'
      : value === 'opencode' ? '/opencode-dark.svg'
      : value === 'hermes' ? '/hermes.png'
      : value === 'gemini' ? '/gemini.svg'
      : '/code-no-action.svg';
  }

  function relativeTime(iso: string): string {
    const diff = Date.now() - new Date(iso).getTime();
    if (!Number.isFinite(diff)) return '';
    if (diff < 60_000) return 'now';
    if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m`;
    if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}h`;
    if (diff < 2_592_000_000) return `${Math.floor(diff / 86_400_000)}d`;
    return new Date(iso).toLocaleDateString();
  }

  function sessionTitle(session: AgentDiscoveredSession): string {
    return session.title || session.preview || session.externalSessionId;
  }

  function setBusy(id: string, busy: boolean) {
    busyIds = busy ? [...busyIds, id] : busyIds.filter((value) => value !== id);
  }

  function openManagedSession(session: AgentSession) {
    const existing = get(tabs).find((tab) => tab.mode === 'agent' && tab.key === session.id);
    if (existing) activateTab(existing.id);
    else addTab(session.title, 'agent', session.id, '#7c5cf8');
    activeAgentSession.set(session);
    agentSessionCenterOpen.set(false);
  }

  async function loadCatalog() {
    loading = true;
    try {
      const sessions = await agentListDiscoveredSessions({ includeHidden: true });
      catalog = sessions;
      // Rows scanned by current builds carry a persistent main-repository
      // identity. Resolve only legacy rows so catalog loading does not spawn
      // Git once per path on every open.
      const paths = [...new Set(sessions
        .filter((session) => !session.projectRoot?.trim())
        .map((session) => session.projectPath?.trim())
        .filter((value): value is string => !!value))];
      projectRoots = paths.length ? await agentResolveProjectRoots(paths) : {};
      selectedIds = selectedIds.filter((id) => sessions.some((session) => session.id === id));
    } catch (error) {
      showToast(`Session catalog failed: ${friendlyError(error)}`, 'error');
    } finally {
      loading = false;
    }
  }

  async function refreshCatalog() {
    if (refreshing) return;
    refreshing = true;
    try {
      const summary = await agentScanDiscoveredSessions();
      await loadCatalog();
      await loadAgentDiscoveredSessions();
      if (summary.errors.length) showToast(`Refreshed with ${summary.errors.length} provider warning(s)`, 'info');
      else showToast(`Refreshed ${summary.upserted} sessions`, 'success');
    } catch (error) {
      showToast(`Refresh failed: ${friendlyError(error)}`, 'error');
    } finally {
      refreshing = false;
    }
  }

  async function adopt(session: AgentDiscoveredSession) {
    if (busyIds.includes(session.id)) return;
    setBusy(session.id, true);
    try {
      const managed = await agentAdoptDiscoveredSession(session.id);
      await Promise.all([loadAgentSessions(), loadAgentDiscoveredSessions()]);
      catalog = catalog.map((item) => item.id === session.id ? { ...item, adoptedAgentSessionId: managed.id } : item);
      openManagedSession(managed);
      showToast('Session opened in ZeroAny Workbench', 'success');
    } catch (error) {
      showToast(`Open failed: ${friendlyError(error)}`, 'error');
    } finally {
      setBusy(session.id, false);
    }
  }

  async function setHidden(session: AgentDiscoveredSession, hidden: boolean) {
    if (busyIds.includes(session.id)) return;
    setBusy(session.id, true);
    try {
      if (hidden) await agentHideDiscoveredSession(session.id);
      else await agentUnhideDiscoveredSession(session.id);
      catalog = catalog.map((item) => item.id === session.id ? { ...item, hidden: hidden ? 1 : 0 } : item);
      selectedIds = selectedIds.filter((id) => id !== session.id);
      await loadAgentDiscoveredSessions();
    } catch (error) {
      showToast(`${hidden ? 'Hide' : 'Restore'} failed: ${friendlyError(error)}`, 'error');
    } finally {
      setBusy(session.id, false);
    }
  }

  async function bulkSetHidden(hidden: boolean) {
    const targets = catalog.filter((session) => selectedIds.includes(session.id) && Boolean(session.hidden) !== hidden);
    if (!targets.length) return;
    busyIds = [...new Set([...busyIds, ...targets.map((session) => session.id)])];
    let succeeded = 0;
    const succeededIds = new Set<string>();
    for (const session of targets) {
      try {
        if (hidden) await agentHideDiscoveredSession(session.id);
        else await agentUnhideDiscoveredSession(session.id);
        succeeded += 1;
        succeededIds.add(session.id);
      } catch { /* continue so one provider row cannot block the rest */ }
    }
    catalog = catalog.map((session) => succeededIds.has(session.id) ? { ...session, hidden: hidden ? 1 : 0 } : session);
    busyIds = busyIds.filter((id) => !targets.some((session) => session.id === id));
    selectedIds = [];
    await loadAgentDiscoveredSessions();
    showToast(`${hidden ? 'Hidden' : 'Restored'} ${succeeded} session${succeeded === 1 ? '' : 's'}`, succeeded === targets.length ? 'success' : 'info');
  }

  function toggleSelected(id: string) {
    selectedIds = selectedIds.includes(id) ? selectedIds.filter((value) => value !== id) : [...selectedIds, id];
  }

  function toggleAllVisible() {
    if (allVisibleSelected) {
      const visible = new Set(visibleSessions.map((session) => session.id));
      selectedIds = selectedIds.filter((id) => !visible.has(id));
    } else {
      selectedIds = [...new Set([...selectedIds, ...visibleSessions.map((session) => session.id)])];
    }
  }

  function chooseProject(key: string) {
    selectedProject = key;
    selectedIds = [];
    visibleLimit = 100;
  }

  onMount(loadCatalog);
</script>

<div class="session-center">
  <header class="sc-header">
    <div>
      <div class="sc-eyebrow">AGENT</div>
      <h1>Session Center</h1>
      <p>Browse, filter, resume, and clean up sessions discovered from your local agents.</p>
    </div>
    <div class="sc-header-actions">
      <button class="sc-btn" disabled={refreshing} onclick={refreshCatalog}>
        <svg class:spin={refreshing} viewBox="0 0 24 24"><path d="M21 12a9 9 0 1 1-2.64-6.36M21 3v6h-6"/></svg>
        {refreshing ? 'Refreshing…' : 'Refresh'}
      </button>
      <button class="sc-close" title="Close Session Center" onclick={() => agentSessionCenterOpen.set(false)}>×</button>
    </div>
  </header>

  <div class="sc-filters">
    <label class="sc-search">
      <svg viewBox="0 0 24 24"><circle cx="11" cy="11" r="7"/><path d="m20 20-4-4"/></svg>
      <input bind:value={query} placeholder="Search titles, paths, session IDs…" />
    </label>
    <select bind:value={provider} title="Provider">
      <option value="all">All providers</option>
      <option value="claude">Claude</option>
      <option value="codex">Codex</option>
      <option value="opencode">OpenCode</option>
      <option value="hermes">Hermes</option>
      <option value="gemini">Antigravity</option>
    </select>
    <select bind:value={range} title="Last activity">
      <option value="7">Last 7 days</option>
      <option value="30">Last 30 days</option>
      <option value="90">Last 90 days</option>
      <option value="all">All time</option>
    </select>
    <div class="sc-status-tabs">
      <button class:active={status === 'external'} onclick={() => { status = 'external'; selectedProject = 'all'; }}>External <span>{statusCounts.external}</span></button>
      <button class:active={status === 'managed'} onclick={() => { status = 'managed'; selectedProject = 'all'; }}>Managed <span>{statusCounts.managed}</span></button>
      <button class:active={status === 'hidden'} onclick={() => { status = 'hidden'; selectedProject = 'all'; }}>Hidden <span>{statusCounts.hidden}</span></button>
      <button class:active={status === 'all'} onclick={() => { status = 'all'; selectedProject = 'all'; }}>All</button>
    </div>
  </div>

  <div class="sc-content">
    <aside class="sc-projects">
      <div class="sc-pane-title">PROJECTS <span>{projectGroups.length}</span></div>
      <button class="sc-project" class:active={selectedProject === 'all'} onclick={() => chooseProject('all')}>
        <span class="sc-project-icon all">⌁</span>
        <span class="sc-project-copy"><strong>All projects</strong><small>Across discovered agents</small></span>
        <span class="sc-project-count">{baseFiltered.length}</span>
      </button>
      {#each projectGroups as group (group.key)}
        <button class="sc-project" class:active={selectedProject === group.key} onclick={() => chooseProject(group.key)} title={group.path}>
          <span class="sc-project-icon">{group.unscoped ? '…' : '⌂'}</span>
          <span class="sc-project-copy">
            <strong>{group.name}</strong>
            <small>{group.path === '__unscoped__' ? 'No project directory' : group.path}</small>
            <span class="sc-project-providers">
              {#each group.providers as item}<img src={providerIcon(item)} alt={providerName(item)} title={providerName(item)} />{/each}
              <em>{relativeTime(group.latest)}</em>
            </span>
          </span>
          <span class="sc-project-count">{group.sessions.length}</span>
        </button>
      {/each}
    </aside>

    <main class="sc-sessions">
      <div class="sc-list-header">
        <div>
          <h2>{selectedProject === 'all' ? 'All sessions' : (projectGroups.find((group) => group.key === selectedProject)?.name || 'Sessions')}</h2>
          <p>{filteredSessions.length} result{filteredSessions.length === 1 ? '' : 's'} · newest first</p>
        </div>
        {#if selectedIds.length > 0}
          <div class="sc-bulk">
            <span>{selectedIds.length} selected</span>
            {#if status === 'hidden'}
              <button onclick={() => bulkSetHidden(false)}>Restore selected</button>
            {:else}
              <button class="danger" onclick={() => bulkSetHidden(true)}>Hide selected</button>
            {/if}
            <button onclick={() => selectedIds = []}>Clear</button>
          </div>
        {/if}
      </div>

      {#if loading}
        <div class="sc-empty">Loading the local session catalog…</div>
      {:else if filteredSessions.length === 0}
        <div class="sc-empty">
          <strong>No matching sessions</strong>
          <span>Try a wider time range or clear one of the filters.</span>
        </div>
      {:else}
        <div class="sc-table-head">
          <label><input type="checkbox" checked={allVisibleSelected} onchange={toggleAllVisible} aria-label="Select visible sessions" /></label>
          <span>Session</span><span>Project</span><span>Updated</span><span></span>
        </div>
        <div class="sc-rows">
          {#each visibleSessions as session (session.id)}
            {@const rowStatus = sessionStatus(session)}
            {@const root = canonicalProjectPath(session)}
            <div class="sc-row" class:selected={selectedIds.includes(session.id)}>
              <label class="sc-check"><input type="checkbox" checked={selectedIds.includes(session.id)} onchange={() => toggleSelected(session.id)} aria-label={`Select ${sessionTitle(session)}`} /></label>
              <div class="sc-session-main">
                <div class="sc-session-title">
                  <img src={providerIcon(session.provider)} alt="" />
                  <strong title={sessionTitle(session)}>{sessionTitle(session)}</strong>
                  <span class="sc-provider">{providerName(session.provider)}</span>
                  {#if session.profile}<span class="sc-profile" title="Hermes profile">{session.profile}</span>{/if}
                  <span class="sc-state {rowStatus}">{rowStatus}</span>
                </div>
                {#if session.preview && session.preview !== session.title}<p>{session.preview}</p>{/if}
                <code title={session.externalSessionId}>{session.externalSessionId}</code>
              </div>
              <div class="sc-row-project" title={root === '__unscoped__' ? (session.projectPath || '') : root}>
                <strong>{isBroadDirectory(root) ? 'Unscoped' : pathName(root)}</strong>
                <span>{session.projectPath || 'No directory'}</span>
              </div>
              <time title={new Date(session.updatedAt).toLocaleString()}>{relativeTime(session.updatedAt)}</time>
              <div class="sc-row-actions">
                {#if rowStatus === 'external'}
                  <button class="primary" disabled={busyIds.includes(session.id)} onclick={() => adopt(session)}>Open in ZeroAny Workbench</button>
                  <button disabled={!session.projectPath} onclick={() => session.projectPath && agentFsReveal(session.projectPath)}>Reveal</button>
                  <button onclick={() => setHidden(session, true)}>Hide</button>
                {:else if rowStatus === 'hidden'}
                  <button onclick={() => setHidden(session, false)}>Restore</button>
                {:else}
                  <span class="managed-label">Already managed</span>
                {/if}
              </div>
            </div>
          {/each}
        </div>
        {#if visibleSessions.length < filteredSessions.length}
          <button class="sc-more" onclick={() => visibleLimit += 100}>Show 100 more · {filteredSessions.length - visibleSessions.length} remaining</button>
        {/if}
      {/if}
    </main>
  </div>
</div>

<style>
  .session-center { position:absolute; inset:0; z-index:40; display:flex; flex-direction:column; min-width:0; min-height:0; overflow:hidden; background:var(--n); color:var(--t1); font-family:var(--ui); }
  .sc-header { min-height:96px; padding:20px 24px 17px; display:flex; justify-content:space-between; gap:24px; border-bottom:1px solid var(--b1); background:linear-gradient(120deg, color-mix(in srgb, var(--acc) 8%, var(--n)), var(--n) 55%); }
  .sc-eyebrow { color:var(--acc); font-size:9px; font-weight:800; letter-spacing:.14em; }
  h1 { margin:3px 0 4px; font-size:22px; line-height:1.2; font-weight:650; }
  .sc-header p, .sc-list-header p { margin:0; color:var(--t3); font-size:11px; }
  .sc-header-actions { display:flex; align-items:flex-start; gap:8px; }
  button, select, input { font:inherit; }
  .sc-btn, .sc-close { border:1px solid var(--b1); background:var(--n2); color:var(--t2); border-radius:6px; height:30px; cursor:pointer; }
  .sc-btn { padding:0 11px; display:flex; align-items:center; gap:6px; font-size:11px; }
  .sc-btn svg { width:13px; height:13px; fill:none; stroke:currentColor; stroke-width:1.8; stroke-linecap:round; }
  .sc-btn svg.spin { animation:spin .9s linear infinite; }
  .sc-close { width:30px; font-size:20px; line-height:1; }
  .sc-btn:hover, .sc-close:hover { background:var(--surface-hover); color:var(--t1); }
  @keyframes spin { to { transform:rotate(360deg); } }

  .sc-filters { min-height:52px; padding:9px 16px; display:flex; align-items:center; gap:8px; border-bottom:1px solid var(--b1); background:var(--s); }
  .sc-search { height:32px; min-width:240px; max-width:430px; flex:1; display:flex; align-items:center; gap:7px; border:1px solid var(--b1); background:var(--n2); border-radius:6px; padding:0 9px; }
  .sc-search:focus-within { border-color:color-mix(in srgb, var(--acc) 60%, var(--b1)); }
  .sc-search svg { width:14px; height:14px; flex:none; fill:none; stroke:var(--t4); stroke-width:1.8; }
  .sc-search input { width:100%; border:0; outline:0; background:transparent; color:var(--t1); font-size:11px; }
  .sc-filters select { height:32px; border:1px solid var(--b1); border-radius:6px; background:var(--n2); color:var(--t2); padding:0 25px 0 9px; font-size:11px; }
  .sc-status-tabs { display:flex; height:32px; border:1px solid var(--b1); border-radius:6px; overflow:hidden; margin-left:auto; }
  .sc-status-tabs button { border:0; border-right:1px solid var(--b1); background:var(--n2); color:var(--t3); padding:0 8px; font-size:10px; cursor:pointer; }
  .sc-status-tabs button:last-child { border-right:0; }
  .sc-status-tabs button.active { background:color-mix(in srgb, var(--acc) 16%, var(--n2)); color:var(--acc); }
  .sc-status-tabs span { margin-left:3px; font-family:var(--mono); opacity:.75; }

  .sc-content { flex:1; min-height:0; display:grid; grid-template-columns:minmax(210px, 24%) 1fr; }
  .sc-projects { min-height:0; overflow-y:auto; border-right:1px solid var(--b1); background:var(--s); padding:10px 7px 16px; }
  .sc-pane-title { padding:5px 7px 8px; font-size:9px; color:var(--t4); font-weight:800; letter-spacing:.08em; }
  .sc-pane-title span { margin-left:5px; font-family:var(--mono); }
  .sc-project { width:100%; min-height:48px; display:flex; align-items:flex-start; gap:8px; padding:7px; border:1px solid transparent; border-radius:6px; background:transparent; color:var(--t2); text-align:left; cursor:pointer; }
  .sc-project:hover { background:var(--n2); }
  .sc-project.active { background:color-mix(in srgb, var(--acc) 10%, var(--n2)); border-color:color-mix(in srgb, var(--acc) 28%, transparent); }
  .sc-project-icon { width:22px; height:22px; flex:none; display:grid; place-items:center; border-radius:5px; background:var(--c); color:var(--t3); font-size:13px; }
  .sc-project-icon.all { color:var(--acc); }
  .sc-project-copy { flex:1; min-width:0; display:flex; flex-direction:column; gap:2px; }
  .sc-project-copy strong { overflow:hidden; text-overflow:ellipsis; white-space:nowrap; font-size:11px; font-weight:600; }
  .sc-project-copy small { overflow:hidden; text-overflow:ellipsis; white-space:nowrap; color:var(--t4); font-family:var(--mono); font-size:8.5px; }
  .sc-project-providers { display:flex; align-items:center; gap:3px; min-height:12px; }
  .sc-project-providers img { width:11px; height:11px; object-fit:contain; }
  .sc-project-providers em { margin-left:3px; color:var(--t4); font-style:normal; font-size:8.5px; }
  .sc-project-count { flex:none; min-width:22px; padding:1px 4px; text-align:center; border:1px solid var(--b1); border-radius:4px; color:var(--t3); font:9px var(--mono); }

  .sc-sessions { min-width:0; min-height:0; display:flex; flex-direction:column; overflow:hidden; }
  .sc-list-header { min-height:61px; padding:12px 16px; display:flex; align-items:center; justify-content:space-between; gap:16px; border-bottom:1px solid var(--b1); }
  .sc-list-header h2 { margin:0 0 3px; font-size:14px; font-weight:600; }
  .sc-bulk { display:flex; align-items:center; gap:6px; color:var(--t3); font-size:10px; }
  .sc-bulk button, .sc-row-actions button { height:25px; border:1px solid var(--b1); border-radius:5px; background:var(--n2); color:var(--t2); padding:0 7px; font-size:9.5px; cursor:pointer; }
  .sc-bulk button:hover, .sc-row-actions button:hover { background:var(--surface-hover); color:var(--t1); }
  .sc-bulk button.danger { color:var(--err); }
  .sc-table-head, .sc-row { display:grid; grid-template-columns:30px minmax(240px, 1.6fr) minmax(130px, .75fr) 58px minmax(150px, auto); align-items:center; }
  .sc-table-head { flex:none; min-height:30px; padding:0 12px; border-bottom:1px solid var(--b1); color:var(--t4); font-size:9px; font-weight:700; text-transform:uppercase; letter-spacing:.05em; }
  .sc-table-head label { display:flex; }
  .sc-rows { flex:1; min-height:0; overflow:auto; }
  .sc-row { min-height:74px; padding:7px 12px; border-bottom:1px solid color-mix(in srgb, var(--b-subtle) 75%, transparent); }
  .sc-row:hover, .sc-row.selected { background:var(--c); }
  .sc-check { align-self:start; padding-top:7px; }
  input[type='checkbox'] { accent-color:var(--acc); }
  .sc-session-main, .sc-row-project { min-width:0; padding-right:12px; }
  .sc-session-title { min-width:0; display:flex; align-items:center; gap:6px; }
  .sc-session-title img { width:18px; height:18px; flex:none; object-fit:contain; }
  .sc-session-title strong { min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; font-size:11.5px; font-weight:550; }
  .sc-provider, .sc-profile, .sc-state { flex:none; border-radius:3px; padding:1px 4px; font-size:8px; font-weight:700; }
  .sc-provider { color:var(--t3); background:var(--n2); border:1px solid var(--b1); }
  .sc-profile { max-width:100px; overflow:hidden; text-overflow:ellipsis; color:var(--acc); background:color-mix(in srgb, var(--acc) 10%, var(--n2)); border:1px solid color-mix(in srgb, var(--acc) 28%, var(--b1)); font-family:var(--mono); }
  .sc-state.external { color:#d29922; background:rgba(210,153,34,.12); }
  .sc-state.managed { color:#3fb950; background:rgba(63,185,80,.12); }
  .sc-state.hidden { color:var(--t4); background:var(--n2); }
  .sc-session-main p { margin:4px 0 2px 24px; color:var(--t3); font-size:9.5px; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  .sc-session-main code { display:block; margin-left:24px; color:var(--t4); font:8.5px var(--mono); overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  .sc-row-project { display:flex; flex-direction:column; gap:3px; }
  .sc-row-project strong { overflow:hidden; text-overflow:ellipsis; white-space:nowrap; color:var(--t2); font-size:10px; }
  .sc-row-project span { overflow:hidden; text-overflow:ellipsis; white-space:nowrap; color:var(--t4); font:8.5px var(--mono); }
  .sc-row time { color:var(--t4); font-size:9px; }
  .sc-row-actions { display:flex; justify-content:flex-end; flex-wrap:wrap; gap:4px; }
  .sc-row-actions button.primary { color:var(--acc); border-color:color-mix(in srgb, var(--acc) 35%, var(--b1)); }
  .sc-row-actions button:disabled { opacity:.45; cursor:default; }
  .managed-label { color:#3fb950; font-size:9px; }
  .sc-empty { flex:1; display:flex; flex-direction:column; align-items:center; justify-content:center; gap:7px; color:var(--t4); font-size:11px; }
  .sc-empty strong { color:var(--t2); font-size:13px; }
  .sc-more { flex:none; height:34px; border:0; border-top:1px solid var(--b1); background:var(--s); color:var(--acc); font-size:10px; cursor:pointer; }
  .sc-more:hover { background:var(--n2); }

  @media (max-width:900px) {
    .sc-content { grid-template-columns:190px 1fr; }
    .sc-table-head, .sc-row { grid-template-columns:28px minmax(210px, 1fr) 55px minmax(120px, auto); }
    .sc-table-head span:nth-of-type(2), .sc-row-project { display:none; }
    .sc-status-tabs button { padding:0 5px; }
  }
</style>
