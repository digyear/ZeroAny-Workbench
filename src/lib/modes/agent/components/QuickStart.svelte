<script lang="ts">
  // Quick Bar — intent-first session creation (product plan §5).
  // One line in, Enter, session created. Configuration chips default from the
  // project's remembered preferences; the Advanced modal stays available.
  import { onMount } from 'svelte';
  import { get } from 'svelte/store';
  import type { AgentSession, AgentContext } from '../types';
  import { AGENT_PROVIDERS } from '../types';
  import type { AgentProvider } from '$lib/shared/agent/providers';
  import {
    agentCreateSession,
    agentGetProjectPreferences,
    agentListContexts,
    agentIsGitRepo,
  } from '../commands';
  import {
    agentSessions,
    agentSessionActivity,
    agentContexts,
    activeAgentSession,
    loadAgentSessions,
  } from '../stores';
  import { tabs as tabsStore, addTab, activateTab } from '$lib/shared/stores/tabs';
  import { showToast } from '$lib/shared/primitives/toast';
  import { SESSION_PURPOSES, getPurposeColor } from '../ai/prompt';

  interface Props {
    show?: boolean;
    /** Open the full New Session modal, carrying the current draft over. */
    onAdvanced?: (draft: { projectPath: string; prompt: string; worktree: boolean }) => void;
  }
  let { show = $bindable(false), onAdvanced }: Props = $props();

  type ProjectChoice = { path: string; root: string; name: string };

  let prompt = $state('');
  let projectPath = $state('');
  let projectName = $state('');
  let provider = $state<AgentProvider>('claude');
  let purpose = $state('Development');
  let worktree = $state(false);
  let contextNames = $state<string[]>([]);
  let busy = $state(false);
  let showProjects = $state(false);
  let rememberedNote = $state('');

  const projects = $derived.by<ProjectChoice[]>(() => {
    const seen = new Map<string, ProjectChoice>();
    for (const s of $agentSessions as AgentSession[]) {
      const root = s.projectRoot || s.projectPath;
      if (!root || seen.has(root)) continue;
      seen.set(root, { path: s.projectPath, root, name: s.projectName || root.split('/').pop() || root });
    }
    return [...seen.values()];
  });

  /** Same-project sessions currently producing output — the concurrency hint
   *  that makes a worktree-off default safe (decision D1). */
  const runningHere = $derived.by(() => {
    if (!projectPath) return 0;
    const root = projectRootOf(projectPath);
    const activity = $agentSessionActivity;
    let n = 0;
    for (const s of $agentSessions as AgentSession[]) {
      if ((s.projectRoot || s.projectPath) !== root) continue;
      if (activity.get(s.id) === 'running') n += 1;
    }
    return n;
  });

  function projectRootOf(path: string): string {
    return projects.find((p) => p.path === path || p.root === path)?.root ?? path;
  }

  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!show) return;
      if (e.key === 'Escape') {
        e.preventDefault();
        show = false;
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

  // On open: default the project to the most recently used one, then load the
  // remembered preferences for it so the chips show what will actually apply.
  $effect(() => {
    if (!show) return;
    if (projectPath) {
      void loadPrefs(projectPath);
      return;
    }
    const recent = [...($agentSessions as AgentSession[])].sort(
      (a, b) => (b.lastUsedAt || '').localeCompare(a.lastUsedAt || ''),
    )[0];
    if (recent) {
      projectPath = recent.projectPath;
      projectName = recent.projectName || '';
      void loadPrefs(projectPath);
    }
  });

  async function loadPrefs(path: string) {
    try {
      const prefs = await agentGetProjectPreferences(path);
      if (!prefs) {
        rememberedNote = '';
        return;
      }
      if (prefs.provider) provider = prefs.provider as AgentProvider;
      if (prefs.purpose) purpose = prefs.purpose;
      if (prefs.contextNames?.length) contextNames = prefs.contextNames;
      rememberedNote = 'remembered';
    } catch {
      rememberedNote = '';
    }
  }

  async function chooseProject(p: ProjectChoice) {
    projectPath = p.path;
    projectName = p.name;
    showProjects = false;
    await loadPrefs(p.path);
  }

  async function browseProject() {
    const { open } = await import('@tauri-apps/plugin-dialog');
    const picked = await open({ directory: true, multiple: false, title: 'Choose project folder' });
    if (typeof picked !== 'string' || !picked) return;
    projectPath = picked;
    projectName = picked.split('/').pop() || picked;
    showProjects = false;
    await loadPrefs(picked);
  }

  /** `/branch` and `@context` are stripped from the prompt and applied as
   *  options; `#123` / issue URLs stay in the prompt (the agent reads them). */
  function parseDirectives(raw: string): { text: string; branch?: string; contexts: string[] } {
    const tokens = raw.split(/\s+/);
    let branch: string | undefined;
    const wanted: string[] = [];
    const kept: string[] = [];
    for (const token of tokens) {
      if (token.startsWith('/') && token.length > 1 && !branch) {
        branch = token.slice(1);
        continue;
      }
      if (token.startsWith('@') && token.length > 1) {
        const name = token.slice(1);
        if (($agentContexts as AgentContext[]).some((c) => c.name === name)) {
          wanted.push(name);
          continue;
        }
      }
      kept.push(token);
    }
    return { text: kept.join(' ').trim(), branch, contexts: wanted };
  }

  async function submit() {
    if (busy) return;
    const raw = prompt.trim();
    if (!projectPath) {
      showToast('Pick a project first', 'error');
      return;
    }
    if (!raw) {
      showToast('Describe what you want the agent to do', 'error');
      return;
    }
    busy = true;
    try {
      const parsed = parseDirectives(raw);
      const isGit = await agentIsGitRepo(projectPath);
      const session = await agentCreateSession(projectPath, parsed.text || raw, {
        provider,
        purpose,
        worktreeEnabled: worktree && isGit,
        contextNames: [...contextNames, ...parsed.contexts],
        baseBranch: parsed.branch,
        branchName: parsed.branch,
      });
      await loadAgentSessions();
      const allTabs = get(tabsStore);
      const existing = allTabs.find((t) => t.mode === 'agent' && t.key === session.id);
      if (existing) activateTab(existing.id);
      else addTab(session.title, 'agent', session.id, getPurposeColor(session.purpose));
      activeAgentSession.set(session);
      prompt = '';
      show = false;
    } catch (e) {
      showToast(String(e), 'error');
    } finally {
      busy = false;
    }
  }

  function advanced() {
    const draft = { projectPath, prompt, worktree };
    show = false;
    onAdvanced?.(draft);
  }
</script>

{#if show}
  <div class="qs-scrim" role="presentation" onclick={() => (show = false)}>
    <div class="qs" role="dialog" aria-label="New session" onclick={(e) => e.stopPropagation()}>
      <div class="qs-proj">
        <button class="qs-projbtn" onclick={() => (showProjects = !showProjects)} title="Choose project">
          <span class="qs-dot" class:off={!projectPath}></span>
          <span class="qs-projname">{projectName || projectPath || 'Choose a project'}</span>
          <span class="qs-caret">▾</span>
        </button>
        {#if showProjects}
          <div class="qs-projlist">
            {#each projects.slice(0, 8) as p (p.root)}
              <button class="qs-projrow" onclick={() => chooseProject(p)}>
                <span>{p.name}</span>
                <span class="qs-path">{p.path}</span>
              </button>
            {/each}
            <button class="qs-projrow action" onclick={browseProject}>Choose folder…</button>
          </div>
        {/if}
      </div>

      <textarea
        class="qs-input"
        bind:value={prompt}
        placeholder="What should the agent do?  (#123 · /branch · @context)"
        rows="3"
        onkeydown={(e) => {
          if (e.key === 'Enter' && !e.shiftKey) {
            e.preventDefault();
            void submit();
          }
        }}
      ></textarea>

      <div class="qs-chips">
        <select class="qs-chip" bind:value={provider} title="Provider">
          {#each AGENT_PROVIDERS as p (p.id)}
            <option value={p.id}>{p.label}</option>
          {/each}
        </select>
        <select class="qs-chip" bind:value={purpose} title="Purpose">
          {#each SESSION_PURPOSES as p (p.id)}
            <option value={p.id}>{p.id}</option>
          {/each}
        </select>
        <button
          class="qs-chip toggle"
          class:on={worktree}
          onclick={() => (worktree = !worktree)}
          title="Isolate this session in a new git worktree"
        >worktree {worktree ? 'on' : 'off'}</button>
        {#if contextNames.length}
          <span class="qs-chip static">{contextNames.length} context</span>
        {/if}
        {#if rememberedNote}
          <span class="qs-hint">defaults from this project</span>
        {/if}
      </div>

      {#if runningHere > 0 && !worktree}
        <div class="qs-warn">
          {runningHere} session{runningHere > 1 ? 's' : ''} already running in this project — they share one
          working tree. <button class="qs-link" onclick={() => (worktree = true)}>Use a worktree</button>
        </div>
      {/if}

      <div class="qs-foot">
        <button class="qs-advanced" onclick={advanced}>Advanced…</button>
        <button class="qs-go" onclick={() => void submit()} disabled={busy}>
          {busy ? 'Starting…' : 'Start ↵'}
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .qs-scrim {
    position: fixed;
    inset: 0;
    background: var(--scrim-strong, rgba(0, 0, 0, 0.45));
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 12vh;
    z-index: var(--z-modal, 200);
  }
  .qs {
    width: min(680px, 92vw);
    background: var(--n2, #17171b);
    border: 1px solid var(--b1, #2a2a31);
    border-radius: 12px;
    padding: 12px;
    box-shadow: 0 18px 60px rgba(0, 0, 0, 0.45);
    font-family: var(--ui);
    color: var(--t1, #ededf0);
  }
  .qs-proj { position: relative; margin-bottom: 8px; }
  .qs-projbtn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    background: transparent;
    border: 1px solid transparent;
    border-radius: 7px;
    padding: 4px 8px;
    color: var(--t2, #b9b9c2);
    font: inherit;
    font-size: 12.5px;
    cursor: pointer;
  }
  .qs-projbtn:hover { background: var(--surface-hover); color: var(--t1); }
  .qs-dot { width: 6px; height: 6px; border-radius: 50%; background: var(--acc); }
  .qs-dot.off { background: var(--t4, #6b6b75); }
  .qs-caret { color: var(--t4); font-size: 10px; }
  .qs-projlist {
    position: absolute;
    top: 32px;
    left: 0;
    min-width: 320px;
    max-height: 260px;
    overflow: auto;
    background: var(--n2, #17171b);
    border: 1px solid var(--b1);
    border-radius: 9px;
    padding: 4px;
    z-index: 2;
  }
  .qs-projrow {
    display: flex;
    justify-content: space-between;
    gap: 14px;
    width: 100%;
    background: transparent;
    border: 0;
    border-radius: 6px;
    padding: 6px 8px;
    color: var(--t2);
    font: inherit;
    font-size: 12.5px;
    text-align: left;
    cursor: pointer;
  }
  .qs-projrow:hover { background: var(--surface-hover); color: var(--t1); }
  .qs-projrow.action { color: var(--acc); }
  .qs-path { color: var(--t4); font-family: var(--mono); font-size: 11px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 240px; }
  .qs-input {
    width: 100%;
    resize: none;
    background: var(--b-subtle, #101014);
    border: 1px solid var(--b1);
    border-radius: 9px;
    padding: 10px 12px;
    color: var(--t1);
    font: inherit;
    font-size: 14px;
    line-height: 1.5;
    outline: none;
  }
  .qs-input:focus { border-color: var(--acc); }
  .qs-chips { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; margin-top: 8px; }
  .qs-chip {
    background: var(--b-subtle, #101014);
    border: 1px solid var(--b1);
    border-radius: 999px;
    padding: 3px 9px;
    color: var(--t2);
    font: inherit;
    font-size: 11.5px;
    cursor: pointer;
  }
  .qs-chip:hover { color: var(--t1); border-color: var(--b2, #3a3a44); }
  .qs-chip.on { color: var(--acc); border-color: var(--acc); }
  .qs-chip.static { cursor: default; color: var(--t3); }
  .qs-hint { color: var(--t4); font-size: 11px; }
  .qs-warn {
    margin-top: 8px;
    padding: 7px 10px;
    border-radius: 8px;
    background: color-mix(in srgb, var(--acc) 12%, transparent);
    color: var(--t2);
    font-size: 12px;
  }
  .qs-link { background: none; border: 0; color: var(--acc); font: inherit; font-size: 12px; cursor: pointer; padding: 0; text-decoration: underline; }
  .qs-foot { display: flex; align-items: center; justify-content: space-between; margin-top: 10px; }
  .qs-advanced { background: none; border: 0; color: var(--t3); font: inherit; font-size: 12px; cursor: pointer; }
  .qs-advanced:hover { color: var(--t1); }
  .qs-go {
    background: var(--acc);
    border: 0;
    border-radius: 8px;
    padding: 6px 14px;
    color: #0d0d0f;
    font: inherit;
    font-size: 12.5px;
    font-weight: 600;
    cursor: pointer;
  }
  .qs-go:disabled { opacity: 0.6; cursor: default; }
</style>