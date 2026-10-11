<script lang="ts">
  // Per-board project override. Two ways to set it:
  //   • Folder path — points at a local clone; sync runs `gh issue list`
  //     (or glab) from that cwd, picking up the remote automatically.
  //   • Project URL — direct GitHub/GitLab URL; sync uses `--repo
  //     owner/repo` so no local clone is needed.
  // User can fill either or both. If both, folder wins on scan (it's
  // strictly more accurate context). Cleared when both empty.

  import Modal from '$lib/shared/primitives/Modal.svelte';
  import { showToast } from '$lib/shared/primitives/toast';
  import { errorToast } from '$lib/utils/errors';
  import { open as openDialog } from '@tauri-apps/plugin-dialog';
  import { workspaceBoardSetProject, workspaceGitAuthStatus, workspaceGitSetToken, workspaceGitClearToken } from '../commands';
  import type { GitAuthStatus } from '../types';

  interface Props {
    show: boolean;
    boardId: string;
    /** Current values from the board's source_config; either may be null. */
    initialPath: string | null;
    initialUrl: string | null;
    onclose?: () => void;
    onsaved?: (next: { path: string | null; url: string | null }) => void;
  }

  let {
    show = $bindable(),
    boardId,
    initialPath,
    initialUrl,
    onclose,
    onsaved,
  }: Props = $props();

  let projectPath = $state(initialPath ?? '');
  let projectUrl = $state(initialUrl ?? '');
  let saving = $state(false);

  // ── Git account (token mode) ──────────────────────────────────────
  // With a personal access token the board talks to GitHub/GitLab over
  // REST: issues, ticket comments and PRs all work without `gh`/`glab`.
  let auth = $state<GitAuthStatus | null>(null);
  let tokenInput = $state('');
  let authBusy = $state(false);

  /** URL the Git account section applies to (typed, else saved). */
  const authUrl = $derived((projectUrl.trim() || initialUrl || '').trim());
  const authSupported = $derived(auth?.provider === 'github' || auth?.provider === 'gitlab');

  $effect(() => {
    const url = authUrl;
    if (!show || !url) {
      auth = null;
      return;
    }
    let cancelled = false;
    workspaceGitAuthStatus(url)
      .then((status) => { if (!cancelled) auth = status; })
      .catch(() => { if (!cancelled) auth = null; });
    return () => { cancelled = true; };
  });

  async function connectToken() {
    const url = authUrl;
    const token = tokenInput.trim();
    if (!url || !token) return;
    authBusy = true;
    try {
      const identity = await workspaceGitSetToken(url, token);
      tokenInput = '';
      auth = await workspaceGitAuthStatus(url);
      showToast(`Connected to ${auth?.host ?? 'git host'} as @${identity}`);
    } catch (e) {
      errorToast('Token rejected', e);
    } finally {
      authBusy = false;
    }
  }

  async function disconnectToken() {
    const url = authUrl;
    if (!url) return;
    authBusy = true;
    try {
      await workspaceGitClearToken(url);
      auth = await workspaceGitAuthStatus(url);
      showToast('Git token removed');
    } catch (e) {
      errorToast('Could not remove token', e);
    } finally {
      authBusy = false;
    }
  }

  $effect(() => {
    if (show) {
      projectPath = initialPath ?? '';
      projectUrl = initialUrl ?? '';
      saving = false;
    }
  });

  async function pickFolder() {
    try {
      const selected = await openDialog({ directory: true, multiple: false });
      if (typeof selected === 'string') {
        projectPath = selected;
      }
    } catch (e) {
      errorToast('Folder pick failed', e);
    }
  }

  async function save() {
    saving = true;
    try {
      const path = projectPath.trim() || null;
      const url = projectUrl.trim() || null;
      await workspaceBoardSetProject(boardId, path, url);
      onsaved?.({ path, url });
      show = false;
      onclose?.();
    } catch (e) {
      errorToast('Save failed', e);
    } finally {
      saving = false;
    }
  }

  async function clear() {
    projectPath = '';
    projectUrl = '';
    saving = true;
    try {
      await workspaceBoardSetProject(boardId, null, null);
      onsaved?.({ path: null, url: null });
      show = false;
      onclose?.();
    } catch (e) {
      errorToast('Clear failed', e);
    } finally {
      saving = false;
    }
  }
</script>

<Modal bind:show title="Board project" width="440px" {onclose}>
  <div class="bc-form">
    <p class="bc-help">
      Bind this board to a project. Pick a local folder, paste a GitHub /
      GitLab URL, or both. Leave both blank to inherit from the workspace.
    </p>

    <label class="bc-field">
      <span class="bc-label">Project folder</span>
      <div class="bc-row">
        <input
          class="bc-input"
          type="text"
          placeholder="/path/to/project"
          bind:value={projectPath}
          spellcheck="false"
        />
        <button class="bc-browse" onclick={pickFolder}>Browse</button>
      </div>
    </label>

    <div class="bc-or"><span>or</span></div>

    <label class="bc-field">
      <span class="bc-label">Project URL</span>
      <input
        class="bc-input"
        type="text"
        placeholder="https://github.com/owner/repo"
        bind:value={projectUrl}
        spellcheck="false"
      />
      <span class="bc-hint">No local clone needed — issues are fetched via the URL.</span>
    </label>

    {#if authSupported && auth}
      <div class="bc-field bc-auth">
        <span class="bc-label">Git account</span>
        <div class="bc-auth-status">
          <span class="bc-auth-dot" class:on={auth.hasToken}></span>
          <span class="bc-auth-text">
            {#if auth.hasToken}
              Connected to {auth.host}{auth.identity ? ` as @${auth.identity}` : ''}
            {:else}
              No token for {auth.host} — board actions use the {auth.cliTool} CLI{auth.cliInstalled
                ? ''
                : ' (not installed)'}
            {/if}
          </span>
        </div>
        <div class="bc-row">
          <input
            class="bc-input"
            type="password"
            placeholder={auth.hasToken ? 'Replace token' : 'Paste a personal access token'}
            bind:value={tokenInput}
            spellcheck="false"
            autocomplete="off"
          />
          <button class="bc-browse" onclick={connectToken} disabled={authBusy || !tokenInput.trim()}>
            {authBusy ? 'Checking…' : 'Connect'}
          </button>
          {#if auth.hasToken}
            <button class="bc-browse" onclick={disconnectToken} disabled={authBusy}>Remove</button>
          {/if}
        </div>
        <span class="bc-hint">
          With a token the board talks to {auth.provider === 'github' ? 'GitHub' : 'GitLab'}
          directly — importing issues, ticket comments and raising PRs need no CLI. The token is
          stored in your system credential store and verified against the host before saving.
        </span>
      </div>
    {/if}

    <div class="bc-actions">
      {#if initialPath || initialUrl}
        <button class="bc-btn-clear" onclick={clear} disabled={saving}>Use workspace project</button>
      {/if}
      <span style="flex:1"></span>
      <button class="bc-btn-cancel" onclick={() => { show = false; onclose?.(); }}>Cancel</button>
      <button class="bc-btn-save" onclick={save} disabled={saving}>
        {saving ? 'Saving…' : 'Save'}
      </button>
    </div>
  </div>
</Modal>

<style>
  .bc-form {
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 16px 20px;
  }
  .bc-help {
    margin: 0;
    font-size: 11.5px;
    color: var(--t3);
    font-family: var(--ui);
    line-height: 1.5;
  }
  .bc-field { display: flex; flex-direction: column; gap: 6px; }
  .bc-label {
    font-family: var(--ui);
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0.04em;
    color: var(--t2);
  }
  .bc-row { display: flex; gap: 6px; }
  .bc-input {
    flex: 1;
    background: var(--surface-hover);
    border: 1px solid var(--b1);
    border-radius: 6px;
    padding: 7px 10px;
    color: var(--t1);
    font-family: var(--mono);
    font-size: 12px;
    outline: none;
    transition: border-color 0.12s;
  }
  .bc-input:focus { border-color: var(--acc); }
  .bc-hint {
    margin-top: 4px;
    font-size: 10.5px;
    color: var(--t4);
    font-family: var(--ui);
  }
  /* Git account row — status dot + host/account line above the token field. */
  .bc-auth {
    padding-top: 12px;
    border-top: 1px solid var(--b1);
  }
  .bc-auth-status {
    display: flex;
    align-items: center;
    gap: 7px;
    font-size: 11.5px;
    font-family: var(--ui);
    color: var(--t2);
  }
  .bc-auth-dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--t4);
    flex: none;
  }
  .bc-auth-dot.on { background: var(--acc); }
  .bc-auth-text { min-width: 0; }
  /* Subtle "or" divider between the two input rows. */
  .bc-or {
    position: relative;
    text-align: center;
    margin: 2px 0;
  }
  .bc-or::before {
    content: '';
    position: absolute;
    left: 0; right: 0; top: 50%;
    height: 1px;
    background: var(--b1);
  }
  .bc-or span {
    position: relative;
    display: inline-block;
    padding: 0 10px;
    background: var(--modal-bg, var(--n, #0d1117));
    color: var(--t4);
    font-family: var(--ui);
    font-size: 10px;
    font-weight: 600;
    letter-spacing: 0.1em;
    text-transform: uppercase;
  }
  .bc-browse {
    padding: 6px 12px;
    border-radius: 6px;
    border: 1px solid var(--b1);
    background: transparent;
    color: var(--t2);
    font-size: 11.5px;
    font-family: var(--ui);
    cursor: default;
    transition: border-color 0.12s, color 0.12s;
  }
  .bc-browse:hover { border-color: var(--b2); color: var(--t1); }
  .bc-actions {
    display: flex;
    gap: 8px;
    align-items: center;
    margin-top: 4px;
  }
  .bc-btn-clear,
  .bc-btn-cancel {
    height: 30px;
    padding: 0 14px;
    border-radius: 6px;
    border: 1px solid var(--b1);
    background: transparent;
    color: var(--t2);
    font-size: 11.5px;
    font-family: var(--ui);
    cursor: default;
    transition: border-color 0.12s, color 0.12s;
  }
  .bc-btn-clear:hover,
  .bc-btn-cancel:hover { border-color: var(--b2); color: var(--t1); }
  .bc-btn-save {
    height: 30px;
    padding: 0 16px;
    border-radius: 6px;
    border: none;
    background: var(--acc);
    color: #fff;
    font-size: 11.5px;
    font-weight: 600;
    font-family: var(--ui);
    cursor: default;
    transition: opacity 0.12s;
  }
  .bc-btn-save:hover:not(:disabled) { opacity: 0.9; }
  .bc-btn-save:disabled { opacity: 0.5; }
</style>
