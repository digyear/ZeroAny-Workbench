<script lang="ts">
  import Modal from '$lib/shared/primitives/Modal.svelte';
  import { agentUpdateSession, agentListContexts, agentGetSessionContexts, agentAttachContext, agentDetachContext, agentValidateBinary } from '../commands';
  import { loadAgentSessions, activeAgentSession } from '../stores';
  import { showToast } from '$lib/shared/primitives/toast';
  import type { AgentSession, AgentContext } from '../types';
  import { AGENT_EVENT } from '$lib/shared/constants/events';

  let { show = $bindable(false), session = $bindable<AgentSession | null>(null) } = $props();

  let title = $state('');
  let skipPermissions = $state(false);
  let gitEnabled = $state(false);
  let gitName = $state('');
  let gitEmail = $state('');
  let contextPrompt = $state('');
  let loading = $state(false);
  let useCustomBinary = $state(false);
  let customBinaryPath = $state('');
  let binaryProbeResult = $state<{ kind: 'idle' | 'ok' | 'err' | 'probing'; msg: string }>({ kind: 'idle', msg: '' });

  // Context attachment
  let contextEnabled = $state(false);
  let availableContexts = $state<AgentContext[]>([]);
  let attachedContextIds = $state<Set<string>>(new Set());
  let originalContextIds = $state<Set<string>>(new Set());
  let showContextDropdown = $state(false);

  $effect(() => {
    if (session && show) {
      title = session.title;
      skipPermissions = session.skipPermissions === 1;
      gitName = session.gitName || '';
      gitEmail = session.gitEmail || '';
      gitEnabled = !!(session.gitName || session.gitEmail);
      contextPrompt = session.contextPrompt;
      customBinaryPath = session.binaryPath || '';
      useCustomBinary = !!session.binaryPath;
      binaryProbeResult = { kind: 'idle', msg: '' };
      loadSessionContexts();
    }
    if (!show) {
      showContextDropdown = false;
    }
  });

  async function pickBinaryFile() {
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const isWindows = navigator.userAgent.toLowerCase().includes('windows');
      const selected = await open({
        directory: false,
        multiple: false,
        title: 'Select CLI binary',
        filters: isWindows
          ? [{ name: 'Executables', extensions: ['exe', 'cmd', 'bat'] }, { name: 'All files', extensions: ['*'] }]
          : [{ name: 'All files', extensions: ['*'] }],
      });
      if (typeof selected === 'string' && selected) {
        customBinaryPath = selected;
        await probeBinary(selected);
      }
    } catch (_) {}
  }

  async function probeBinary(path: string) {
    const trimmed = path.trim();
    if (!trimmed) {
      binaryProbeResult = { kind: 'idle', msg: '' };
      return;
    }
    binaryProbeResult = { kind: 'probing', msg: 'Checking…' };
    try {
      const banner = await agentValidateBinary(trimmed);
      binaryProbeResult = { kind: 'ok', msg: banner.split('\n')[0].slice(0, 80) };
    } catch (e: any) {
      const msg = (typeof e === 'string' ? e : e?.message ?? String(e)).split('\n')[0].slice(0, 120);
      binaryProbeResult = { kind: 'err', msg };
    }
  }

  async function loadSessionContexts() {
    if (!session) return;
    try {
      availableContexts = await agentListContexts();
      const attached = await agentGetSessionContexts(session.id);
      const ids = new Set(attached.map(c => c.id));
      attachedContextIds = new Set(ids);
      originalContextIds = new Set(ids);
      contextEnabled = ids.size > 0;
    } catch (_) {}
  }

  function getContextName(id: string): string {
    return availableContexts.find(c => c.id === id)?.name || id;
  }

  async function handleSave() {
    if (!session || !title.trim()) return;
    if (gitEnabled && (!gitName.trim() || !gitEmail.trim())) return;
    loading = true;
    try {
      await agentUpdateSession({
        id: session.id,
        title: title.trim(),
        skipPermissions: skipPermissions,
        gitName: gitEnabled && gitName.trim() ? gitName.trim() : undefined,
        gitEmail: gitEnabled && gitEmail.trim() ? gitEmail.trim() : undefined,
        contextPrompt: contextPrompt,
        // Empty string CLEARS the override (back to $PATH); a non-empty
        // string sets it; omitting the field leaves the existing value
        // untouched. Toggle-off needs the empty-string form.
        binaryPath: useCustomBinary ? customBinaryPath.trim() : '',
      });

      // Sync context attachments — detach removed, attach added
      for (const id of originalContextIds) {
        if (!attachedContextIds.has(id)) {
          await agentDetachContext(session.id, id);
        }
      }
      for (const id of attachedContextIds) {
        if (!originalContextIds.has(id)) {
          await agentAttachContext(session.id, id);
        }
      }

      await loadAgentSessions();
      // Update in-memory active session and relaunch with new settings
      if (session) {
        const updated = {
          ...session,
          title: title.trim(),
          contextPrompt,
          skipPermissions: skipPermissions ? 1 : 0,
          gitName: gitEnabled && gitName.trim() ? gitName.trim() : null,
          gitEmail: gitEnabled && gitEmail.trim() ? gitEmail.trim() : null,
          binaryPath: useCustomBinary && customBinaryPath.trim() ? customBinaryPath.trim() : null,
        };
        activeAgentSession.set(updated);
        // Relaunch terminal with --resume + updated prompt (shows loader, seamless)
        window.dispatchEvent(new CustomEvent(AGENT_EVENT.RELAUNCH_SESSION, { detail: { session: updated } }));
      }
      show = false;
      showToast('Session updated', 'success');
    } catch (e: any) {
      showToast(String(e), 'error');
    } finally {
      loading = false;
    }
  }

  let canSave = $derived(
    !!title && title.trim() !== '' &&
    (!gitEnabled || (!!gitName && gitName.trim() !== '' && !!gitEmail && gitEmail.trim() !== ''))
  );
</script>

<Modal bind:show title="Edit Session" width="440px">
  {#if session}
    <div class="ns-form">
      <label class="ns-field">
        <span class="ns-label">Title</span>
        <input class="ns-input" type="text" bind:value={title} placeholder="Session title" />
      </label>

      <div class="ns-info-row">
        <span class="ns-info-label">Purpose</span>
        <span class="ns-info-value">{session.purpose}</span>
      </div>

      {#if session.purpose === 'Custom'}
        <label class="ns-field">
          <span class="ns-label">System Prompt <span class="ns-optional">(optional)</span></span>
          <textarea
            class="ns-textarea"
            bind:value={contextPrompt}
            placeholder="Custom instructions for this session..."
            rows="3"
          ></textarea>
        </label>
      {/if}
      <div class="ns-info-row">
        <span class="ns-info-label">Project</span>
        <span class="ns-info-value ns-mono">{session.projectPath}</span>
      </div>

      <div class="ns-adv-label">Advanced</div>

      <div class="ns-toggle-row">
        <div class="ns-toggle-info">
          <span class="ns-toggle-text">Skip permissions</span>
          <span class="ns-toggle-hint">Auto-approve all tool calls without confirmation</span>
        </div>
        <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
        <button class="ns-toggle" class:on={skipPermissions} onclick={() => skipPermissions = !skipPermissions}>
          <span class="ns-toggle-knob"></span>
        </button>
      </div>

      <!-- Git Identity toggle -->
      <div class="ns-toggle-row">
        <div class="ns-toggle-info">
          <span class="ns-toggle-text">Git Identity</span>
          <span class="ns-toggle-hint">Override git author name and email for this session</span>
        </div>
        <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
        <button class="ns-toggle" class:on={gitEnabled} onclick={() => gitEnabled = !gitEnabled}>
          <span class="ns-toggle-knob"></span>
        </button>
      </div>
      {#if gitEnabled}
        <div class="ns-adv-body">
          <div class="ns-row">
            <label class="ns-adv-field">
              <span class="ns-adv-label-sm">Name <span class="ns-required">*</span></span>
              <input type="text" class="ns-input" bind:value={gitName} placeholder="e.g. John Doe" />
            </label>
            <label class="ns-adv-field">
              <span class="ns-adv-label-sm">Email <span class="ns-required">*</span></span>
              <input type="text" class="ns-input" bind:value={gitEmail} placeholder="e.g. john@example.com" />
            </label>
          </div>
        </div>
      {/if}

      <!-- Custom binary path — per-session override of the CLI location.
           Mirrors the toggle in NewSessionModal so devs can change it
           after the fact (e.g. picked the wrong binary at creation
           time, or installed a newer version somewhere else). -->
      <div class="ns-toggle-row">
        <div class="ns-toggle-info">
          <span class="ns-toggle-text">Custom Binary Path</span>
          <span class="ns-toggle-hint">Use a specific {session.provider} binary for this session instead of $PATH</span>
        </div>
        <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
        <button class="ns-toggle" class:on={useCustomBinary} onclick={() => { useCustomBinary = !useCustomBinary; if (!useCustomBinary) { customBinaryPath = ''; binaryProbeResult = { kind: 'idle', msg: '' }; } }}>
          <span class="ns-toggle-knob"></span>
        </button>
      </div>
      {#if useCustomBinary}
        <div class="ns-adv-body">
          <div class="ns-path-row">
            <input
              class="ns-input ns-path-input"
              type="text"
              bind:value={customBinaryPath}
              placeholder="/usr/local/bin/{session.provider}"
              onblur={() => probeBinary(customBinaryPath)}
            />
            <button class="ns-btn-browse" onclick={pickBinaryFile}>Browse</button>
          </div>
          {#if binaryProbeResult.kind === 'probing'}
            <div class="ns-probe-hint">{binaryProbeResult.msg}</div>
          {:else if binaryProbeResult.kind === 'ok'}
            <div class="ns-probe-hint ns-probe-ok">✓ {binaryProbeResult.msg}</div>
          {:else if binaryProbeResult.kind === 'err'}
            <div class="ns-probe-hint ns-probe-err">⚠ {binaryProbeResult.msg} — save anyway?</div>
          {/if}
        </div>
      {/if}

      <!-- Attach Contexts toggle -->
      <div class="ns-toggle-row">
        <div class="ns-toggle-info">
          <span class="ns-toggle-text">Attach Contexts</span>
          <span class="ns-toggle-hint">Inject context snippets into the project's agent file before each spawn</span>
        </div>
        <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
        <button class="ns-toggle" class:on={contextEnabled} onclick={() => { contextEnabled = !contextEnabled; if (!contextEnabled) attachedContextIds = new Set(); }}>
          <span class="ns-toggle-knob"></span>
        </button>
      </div>
      {#if contextEnabled}
        <div class="ns-adv-body">
          {#if attachedContextIds.size > 0}
            <div class="ns-ctx-chips">
              {#each [...attachedContextIds] as id}
                <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
                <span class="ns-ctx-chip">
                  {getContextName(id)}
                  <span class="ns-ctx-x" onclick={() => { const next = new Set(attachedContextIds); next.delete(id); attachedContextIds = next; }}>×</span>
                </span>
              {/each}
            </div>
          {/if}
          <div class="ns-ctx-add-wrap">
            <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
            <button class="ns-ctx-add-btn" onclick={(e) => { e.stopPropagation(); showContextDropdown = !showContextDropdown; }}>
              <svg width="10" height="10" viewBox="0 0 16 16" fill="currentColor"><path d="M7.75 2a.75.75 0 01.75.75V7h4.25a.75.75 0 010 1.5H8.5v4.25a.75.75 0 01-1.5 0V8.5H2.75a.75.75 0 010-1.5H7V2.75A.75.75 0 017.75 2z"/></svg>
              Add
            </button>
            {#if showContextDropdown}
              <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
              <div class="ns-ctx-backdrop" onclick={() => showContextDropdown = false}></div>
              <div class="ns-ctx-dropdown">
                {#each availableContexts.filter(c => !attachedContextIds.has(c.id)) as ctx}
                  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
                  <div class="ns-ctx-dd-item" onclick={() => { attachedContextIds = new Set([...attachedContextIds, ctx.id]); showContextDropdown = false; }}>
                    <span class="ns-ctx-dd-name">{ctx.name}</span>
                    <span class="ns-ctx-dd-preview">{ctx.content.slice(0, 60)}</span>
                  </div>
                {:else}
                  <div class="ns-ctx-dd-empty">No more contexts available</div>
                {/each}
              </div>
            {/if}
          </div>
        </div>
      {/if}

      <div class="ns-actions">
        <button class="ns-btn-cancel" onclick={() => show = false}>Cancel</button>
        <button class="ns-btn-create" onclick={handleSave} disabled={!canSave || loading}>
          {loading ? 'Saving...' : 'Save Changes'}
        </button>
      </div>
    </div>
  {/if}
</Modal>

<style>
  .ns-form { display: flex; flex-direction: column; gap: 12px; }
  .ns-field { display: flex; flex-direction: column; gap: 4px; }
  .ns-label { font-size: 12px; font-weight: 600; color: var(--t2); text-transform: uppercase; font-family: var(--ui); }
  .ns-optional { font-size: 10px; color: var(--t3); font-weight: normal; text-transform: none; }
  .ns-input {
    width: 100%; background: var(--e); border: 1px solid var(--b1); border-radius: 6px;
    padding: 8px 10px; font-size: 13px; color: var(--t1); outline: none; box-sizing: border-box;
    font-family: var(--mono); transition: border-color 0.15s;
  }
  .ns-input:focus { border-color: var(--acc); }
  .ns-input::placeholder { color: var(--t3); }
  .ns-textarea {
    width: 100%; padding: 8px 10px; border-radius: 6px; border: 1px solid var(--b1);
    background: var(--e); color: var(--t1); font-size: 12px; font-family: var(--mono);
    resize: vertical; min-height: 50px; line-height: 1.5; outline: none; box-sizing: border-box;
  }
  .ns-textarea:focus { border-color: var(--acc); }
  .ns-textarea::placeholder { color: var(--t3); }
  .ns-path-row { display: flex; gap: 6px; }
  .ns-path-row .ns-input { flex: 1; min-width: 0; }
  .ns-btn-browse {
    height: 34px; padding: 0 12px; background: var(--e);
    border: 1px solid var(--b1); border-radius: 6px; color: var(--t1);
    font-family: var(--ui); font-size: 12px; cursor: pointer;
    transition: border-color 0.15s, background 0.15s; white-space: nowrap;
  }
  .ns-btn-browse:hover { border-color: var(--acc); background: var(--b1); }
  .ns-probe-hint {
    margin-top: 6px;
    font-size: 11px; font-family: var(--mono);
    color: var(--t3); line-height: 1.4; word-break: break-word;
  }
  .ns-probe-ok { color: var(--ok, #2ee08a); }
  .ns-probe-err { color: var(--warn, #f4c150); }
  .ns-info-row {
    display: flex; align-items: center; justify-content: space-between;
    padding: 6px 0;
  }
  .ns-info-label { font-size: 11px; color: var(--t3); font-family: var(--ui); text-transform: uppercase; letter-spacing: 0.05em; }
  .ns-info-value { font-size: 12px; color: var(--t2); font-family: var(--ui); }
  .ns-mono { font-family: var(--mono); font-size: 11px; max-width: 260px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .ns-adv-label {
    font-size: 11px; font-weight: 600; color: var(--t3); text-transform: uppercase;
    letter-spacing: 0.1em; margin-top: 6px; font-family: var(--ui);
  }
  .ns-toggle-row {
    display: flex; align-items: center; justify-content: space-between; margin-top: 4px;
  }
  .ns-toggle-info { display: flex; flex-direction: column; gap: 2px; }
  .ns-toggle-text { font-size: 12px; color: var(--t2); font-family: var(--ui); }
  .ns-toggle-hint { font-size: 10px; color: var(--t4); font-family: var(--ui); }
  .ns-toggle {
    width: 36px; height: 20px; border-radius: 10px; border: 1px solid var(--b1);
    background: var(--surface-hover); cursor: pointer; position: relative;
    transition: all 0.2s; padding: 0; flex-shrink: 0;
  }
  .ns-toggle.on { background: var(--acc); border-color: var(--acc); }
  .ns-toggle-knob {
    position: absolute; top: 2px; left: 2px; width: 14px; height: 14px;
    border-radius: 50%; background: var(--t3); transition: all 0.2s;
  }
  .ns-toggle.on .ns-toggle-knob { left: 18px; background: #fff; }
  .ns-adv-body {
    display: flex; flex-direction: column; gap: 8px; padding: 4px 0 0;
    animation: advIn 0.12s ease;
  }
  @keyframes advIn { from { opacity: 0; } to { opacity: 1; } }
  .ns-row { display: flex; gap: 8px; }
  .ns-adv-field { flex: 1; display: flex; flex-direction: column; gap: 4px; }
  .ns-adv-label-sm { font-size: 11px; color: var(--t3); font-family: var(--ui); }
  .ns-required { color: var(--err, #f85149); font-weight: 600; }
  .ns-ctx-chips { display: flex; flex-wrap: wrap; gap: 4px; }
  .ns-ctx-chip {
    display: flex; align-items: center; gap: 4px; padding: 3px 6px 3px 10px;
    border-radius: 12px; background: color-mix(in srgb, var(--acc) 12%, transparent);
    border: 1px solid color-mix(in srgb, var(--acc) 25%, transparent);
    color: var(--acc); font-size: 11px; font-weight: 500;
  }
  .ns-ctx-x { cursor: pointer; font-size: 14px; line-height: 1; opacity: 0.6; transition: opacity 0.1s; }
  .ns-ctx-x:hover { opacity: 1; }
  /* Click-outside catch for the dropdown. Stays visually invisible (no
     scrim) because we're already inside a modal — dimming the viewport
     a second time would imply a second modal. The transparent fill
     still receives clicks. z-index sits above sibling modal content
     but below the dropdown itself. */
  .ns-ctx-backdrop { position: fixed; inset: 0; z-index: var(--z-popover); background: transparent; }
  .ns-ctx-add-wrap { position: relative; }
  .ns-ctx-add-btn {
    display: flex; align-items: center; gap: 4px; padding: 4px 10px; border-radius: 4px;
    border: 1px dashed var(--b1); background: transparent; color: var(--t3);
    font-size: 11px; font-family: var(--ui); cursor: pointer; transition: all 0.1s;
  }
  .ns-ctx-add-btn:hover { border-color: var(--acc); color: var(--acc); }
  .ns-ctx-dropdown {
    position: absolute; top: calc(100% + 4px); left: 0; width: 250px;
    background: var(--modal-bg, var(--n)); border: 1px solid var(--b1); border-radius: 6px;
    box-shadow: 0 8px 24px rgba(0,0,0,0.4); z-index: calc(var(--z-popover) + 1); max-height: 180px;
    overflow-y: auto; padding: 4px;
  }
  .ns-ctx-dd-item { padding: 6px 10px; border-radius: 4px; cursor: pointer; transition: background 0.1s; }
  .ns-ctx-dd-item:hover { background: var(--surface-hover); }
  .ns-ctx-dd-name { font-size: 12px; font-weight: 500; color: var(--t1); display: block; }
  .ns-ctx-dd-preview { font-size: 10px; color: var(--t3); display: block; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .ns-ctx-dd-empty { padding: 10px; text-align: center; font-size: 11px; color: var(--t3); }
  .ns-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 8px; padding-top: 12px; border-top: 1px solid var(--b1); }
  .ns-btn-cancel {
    padding: 7px 16px; border-radius: 6px; font-size: 13px; cursor: pointer;
    border: 1px solid var(--b1); background: transparent; color: var(--t2); font-family: var(--ui);
  }
  .ns-btn-cancel:hover { background: var(--surface-hover); }
  .ns-btn-create {
    padding: 7px 16px; border-radius: 6px; font-size: 13px; cursor: pointer;
    border: none; background: var(--acc); color: #fff; font-weight: 600; font-family: var(--ui);
  }
  .ns-btn-create:hover:not(:disabled) { filter: brightness(1.1); }
  .ns-btn-create:disabled { opacity: 0.4; cursor: not-allowed; }
</style>
