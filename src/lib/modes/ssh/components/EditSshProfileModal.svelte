<script lang="ts">
  import Modal from '$lib/shared/primitives/Modal.svelte';
  import jsQR from 'jsqr';
  import { sshUpdateProfile, sshHasTotp, sshSetTotp, sshTestTotp } from '../commands';
  import { loadSshProfiles, sshProfiles } from '../stores';
  import { showToast } from '$lib/shared/primitives/toast';
  import type { SshAuthType, SshProfile } from '../types';
  import { SSH_EVENT } from '$lib/shared/constants/events';

  let { show = $bindable(false), profile = $bindable<SshProfile | null>(null) } = $props();

  type Tab = 'general' | 'advanced';
  type ConnectionMode = 'direct' | 'jump' | 'command';

  let activeTab = $state<Tab>('general');

  let name = $state('');
  let host = $state('');
  let port = $state(22);
  let username = $state('');
  let authType = $state<SshAuthType>('key');
  let keyPath = $state('');
  let passphrase = $state('');
  let password = $state('');
  // TOTP enrolment: lets the app answer the one-time-code round itself.
  let totpHas = $state(false);
  let totpSeed = $state('');
  let totpCode = $state<string | null>(null);
  let totpBusy = $state(false);
  let totpTest = $state<{ ok: boolean; detail: string } | null>(null);
  let showTotpHelp = $state(false);
  let qrInput: HTMLInputElement | undefined = $state();
  let revealSecret = $state(false);
  let loading = $state(false);
  let connectionMode = $state<ConnectionMode>('direct');
  let jumpProfileId = $state<string>('');
  let proxyCommand = $state('');

  $effect(() => {
    if (profile && show) {
      name = profile.name;
      host = profile.host;
      port = profile.port;
      username = profile.username;
      // Legacy 'interactive' auth_type displays as 'password' — backend
      // treats them equivalently now (password falls back to interactive
      // prompts on rejection); the explicit 'interactive' chip was
      // removed. Saving will persist the new value.
      authType = profile.authType === 'interactive' ? 'password' : profile.authType;
      keyPath = profile.keyPath ?? '';
      passphrase = '';
      password = '';
      totpCode = null;
      void loadTotp(profile.id);
      revealSecret = false;
      // ProxyCommand wins over jumpProfileId if both populated — matches
      // the connect path's precedence so the user sees what's used.
      if (profile.proxyCommand) {
        connectionMode = 'command';
        proxyCommand = profile.proxyCommand;
        jumpProfileId = profile.jumpProfileId ?? '';
      } else if (profile.jumpProfileId) {
        connectionMode = 'jump';
        jumpProfileId = profile.jumpProfileId;
        proxyCommand = '';
      } else {
        connectionMode = 'direct';
        jumpProfileId = '';
        proxyCommand = '';
      }
      // Always land on General when opening — the dot indicator on
      // Advanced signals "something custom here" so the user can decide
      // whether to switch tabs.
      activeTab = 'general';
    }
  });

  async function pickKeyFile() {
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const selected = await open({
        multiple: false,
        title: 'Select SSH Private Key',
        filters: [
          { name: 'SSH Keys', extensions: ['pem', 'key', 'ppk'] },
          { name: 'All Files', extensions: ['*'] },
        ],
      });
      if (typeof selected === 'string') keyPath = selected;
    } catch {
      /* ignore */
    }
  }

  async function loadTotp(id: string) {
    try {
      totpHas = await sshHasTotp(id);
    } catch {
      totpHas = false;
    }
  }

  async function saveTotp() {
    if (!profile || !totpSeed.trim()) return;
    totpBusy = true;
    try {
      const res = await sshSetTotp(profile.id, totpSeed.trim());
      totpHas = res.enrolled;
      totpCode = res.code ?? null;
      totpSeed = '';
      showToast(res.enrolled ? `TOTP enrolled — current code ${res.code}` : 'TOTP cleared', 'success');
    } catch (e) {
      showToast(String(e), 'error');
    } finally {
      totpBusy = false;
    }
  }

  /** Decode an `otpauth://` QR straight from a screenshot — the host shows a
   *  QR while enrolling, never the seed itself, so this is the realistic path
   *  (no third-party scanner needed). */
  async function importQr(file: File) {
    if (!profile) return;
    totpBusy = true;
    totpTest = null;
    try {
      const bitmap = await createImageBitmap(file);
      const canvas = document.createElement('canvas');
      canvas.width = bitmap.width;
      canvas.height = bitmap.height;
      const ctx = canvas.getContext('2d');
      if (!ctx) throw new Error('canvas unavailable');
      ctx.drawImage(bitmap, 0, 0);
      const pixels = ctx.getImageData(0, 0, canvas.width, canvas.height);
      const qr = jsQR(pixels.data, pixels.width, pixels.height);
      if (!qr) throw new Error('no QR code found in that image');
      const text = qr.data.trim();
      if (!/^otpauth:\/\//i.test(text)) {
        throw new Error(`that QR is not an otpauth URI (${text.slice(0, 40)}…)`);
      }
      const res = await sshSetTotp(profile.id, text);
      totpHas = res.enrolled;
      totpCode = res.code ?? null;
      totpSeed = '';
      showToast(`TOTP enrolled — current code ${res.code}`, 'success');
    } catch (e) {
      showToast(String(e), 'error');
    } finally {
      totpBusy = false;
      if (qrInput) qrInput.value = '';
    }
  }

  async function testTotp() {
    if (!profile) return;
    totpBusy = true;
    totpTest = null;
    try {
      totpTest = await sshTestTotp(profile.id);
    } catch (e) {
      totpTest = { ok: false, detail: String(e) };
    } finally {
      totpBusy = false;
    }
  }

  async function clearTotp() {
    if (!profile) return;
    totpBusy = true;
    try {
      await sshSetTotp(profile.id, '');
      totpHas = false;
      totpCode = null;
      showToast('TOTP seed removed', 'success');
    } catch (e) {
      showToast(String(e), 'error');
    } finally {
      totpBusy = false;
    }
  }

  async function handleSave() {
    if (!profile) return;
    if (!name.trim() || !host.trim() || !username.trim()) return;
    if (authType === 'key' && !keyPath.trim()) return;

    loading = true;
    try {
      const updated = await sshUpdateProfile({
        id: profile.id,
        name: name.trim(),
        host: host.trim(),
        port: Number(port) || 22,
        username: username.trim(),
        authType,
        keyPath: authType === 'key' ? keyPath.trim() : null,
        accentColor: profile.accentColor,
        // Only send secret/passphrase when user typed something —
        // `undefined` keeps the existing keychain value untouched.
        secret: authType === 'password' && password ? password : undefined,
        passphrase: authType === 'key' && passphrase ? passphrase : undefined,
        // Empty string clears the column, present value sets it. Rust
        // update path treats "" as a clear sentinel for both fields.
        jumpProfileId:
          connectionMode === 'jump' && jumpProfileId ? jumpProfileId : '',
        proxyCommand:
          connectionMode === 'command' && proxyCommand.trim() ? proxyCommand.trim() : '',
      });
      await loadSshProfiles();
      window.dispatchEvent(new CustomEvent(SSH_EVENT.PROFILE_UPDATED, { detail: updated }));
      show = false;
      showToast('SSH profile updated', 'success');
    } catch (e: any) {
      showToast(String(e), 'error');
    } finally {
      loading = false;
    }
  }

  let canSave = $derived(
    name.trim() !== '' &&
      host.trim() !== '' &&
      username.trim() !== '' &&
      (authType !== 'key' || keyPath.trim() !== '')
  );

  let advancedDirty = $derived(connectionMode !== 'direct');
</script>

<Modal bind:show title="Edit SSH connection" width="560px">
  {#if profile}
    <div class="sshd-root">
      <div class="sshd-tabs" role="tablist">
        <button type="button" role="tab" class="sshd-tab" class:active={activeTab === 'general'} aria-selected={activeTab === 'general'} onclick={() => (activeTab = 'general')}>General</button>
        <button type="button" role="tab" class="sshd-tab" class:active={activeTab === 'advanced'} aria-selected={activeTab === 'advanced'} onclick={() => (activeTab = 'advanced')}>
          Advanced
          {#if advancedDirty}<span class="sshd-tab-dot" aria-hidden="true"></span>{/if}
        </button>
      </div>

      {#if activeTab === 'general'}
        <div class="sshd-block">
          <span class="sshd-label">Connection name</span>
          <input class="sshd-input" type="text" bind:value={name} />
        </div>

        <div class="sshd-row">
          <div class="sshd-block grow">
            <span class="sshd-label">Host</span>
            <input class="sshd-input mono" type="text" bind:value={host} />
          </div>
          <div class="sshd-block narrow">
            <span class="sshd-label">Port</span>
            <input class="sshd-input mono" type="number" min="1" max="65535" bind:value={port} />
          </div>
        </div>

        <div class="sshd-block">
          <span class="sshd-label">Username</span>
          <input class="sshd-input mono" type="text" bind:value={username} />
        </div>

        <div class="sshd-block">
          <span class="sshd-label">Authentication</span>
          <div class="sshd-tile-row">
            <button type="button" class="sshd-tile" class:active={authType === 'key'} onclick={() => (authType = 'key')}>
              <span class="sshd-tile-icon"><svg viewBox="0 0 24 24" width="15" height="15" fill="none"><circle cx="8" cy="15" r="4" stroke="currentColor" stroke-width="1.6"/><path d="M11 12l8-8M15 8l3 3M17 6l3 3" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/></svg></span>
              <span class="sshd-tile-label">Key</span>
            </button>
            <button type="button" class="sshd-tile" class:active={authType === 'password'} onclick={() => (authType = 'password')}>
              <span class="sshd-tile-icon"><svg viewBox="0 0 24 24" width="15" height="15" fill="none"><circle cx="6.5" cy="12" r="1.5" fill="currentColor"/><circle cx="12" cy="12" r="1.5" fill="currentColor"/><circle cx="17.5" cy="12" r="1.5" fill="currentColor"/><rect x="3" y="7" width="18" height="10" rx="2" stroke="currentColor" stroke-width="1.5"/></svg></span>
              <span class="sshd-tile-label">Password</span>
            </button>
            <button type="button" class="sshd-tile" class:active={authType === 'agent'} onclick={() => (authType = 'agent')}>
              <span class="sshd-tile-icon"><svg viewBox="0 0 24 24" width="15" height="15" fill="none"><path d="M4 7h12a3 3 0 010 6H8a3 3 0 000 6h12" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg></span>
              <span class="sshd-tile-label">Agent</span>
            </button>
          </div>
        </div>

        {#if authType === 'agent'}
          <div class="sshd-info">Uses keys from ssh-agent. Run <code>ssh-add</code> first. Required for hardware tokens.</div>
        {:else if authType === 'key'}
          <div class="sshd-block">
            <span class="sshd-label">Private key file</span>
            <div class="sshd-with-suffix">
              <input class="sshd-input mono" type="text" bind:value={keyPath} />
              <button class="sshd-suffix-btn" onclick={pickKeyFile} type="button" title="Choose file" aria-label="Choose file">
                <svg viewBox="0 0 24 24" width="14" height="14" fill="none"><path d="M4 7a2 2 0 012-2h3l2 2h7a2 2 0 012 2v8a2 2 0 01-2 2H6a2 2 0 01-2-2V7z" stroke="currentColor" stroke-width="1.6"/></svg>
              </button>
            </div>
          </div>
          <div class="sshd-block">
            <span class="sshd-label">Passphrase</span>
            <div class="sshd-with-suffix">
              <input class="sshd-input" type={revealSecret ? 'text' : 'password'} bind:value={passphrase} placeholder="Blank = keep existing" autocomplete="off" />
              <button class="sshd-suffix-btn" type="button" onclick={() => (revealSecret = !revealSecret)} aria-label={revealSecret ? 'Hide passphrase' : 'Show passphrase'}>
                {#if revealSecret}<svg viewBox="0 0 24 24" width="14" height="14" fill="none"><path d="M4 4l16 16" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/><path d="M9.5 5.5A9 9 0 0121 12c-.6 1.2-1.4 2.3-2.4 3.2M14.5 18.5A9 9 0 013 12c.6-1.2 1.4-2.3 2.4-3.2" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/><circle cx="12" cy="12" r="3" stroke="currentColor" stroke-width="1.6"/></svg>{:else}<svg viewBox="0 0 24 24" width="14" height="14" fill="none"><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z" stroke="currentColor" stroke-width="1.6"/><circle cx="12" cy="12" r="3" stroke="currentColor" stroke-width="1.6"/></svg>{/if}
              </button>
            </div>
          </div>
        {:else}
          <div class="sshd-block">
            <span class="sshd-label">Password <span class="sshd-optional">(optional)</span></span>
            <div class="sshd-with-suffix">
              <input class="sshd-input" type={revealSecret ? 'text' : 'password'} bind:value={password} placeholder="Blank = keep existing" autocomplete="off" />
              <button class="sshd-suffix-btn" type="button" onclick={() => (revealSecret = !revealSecret)} aria-label={revealSecret ? 'Hide password' : 'Show password'}>
                {#if revealSecret}<svg viewBox="0 0 24 24" width="14" height="14" fill="none"><path d="M4 4l16 16" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/><path d="M9.5 5.5A9 9 0 0121 12c-.6 1.2-1.4 2.3-2.4 3.2M14.5 18.5A9 9 0 013 12c.6-1.2 1.4-2.3 2.4-3.2" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/><circle cx="12" cy="12" r="3" stroke="currentColor" stroke-width="1.6"/></svg>{:else}<svg viewBox="0 0 24 24" width="14" height="14" fill="none"><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z" stroke="currentColor" stroke-width="1.6"/><circle cx="12" cy="12" r="3" stroke="currentColor" stroke-width="1.6"/></svg>{/if}
              </button>
            </div>
          </div>
        {/if}

        <div class="sshd-block">
          <span class="sshd-label">
            Second factor (TOTP)
            <span class="sshd-optional">(optional)</span>
          </span>
          <div class="sshd-with-suffix">
            <input
              class="sshd-input mono"
              type={revealSecret ? 'text' : 'password'}
              bind:value={totpSeed}
              placeholder={totpHas ? 'Enrolled — paste a new seed to replace' : 'Base32 seed or otpauth:// URI'}
              autocomplete="off"
            />
            <button
              class="sshd-suffix-btn"
              type="button"
              onclick={saveTotp}
              disabled={totpBusy || !totpSeed.trim()}
              title="Verify and save the seed"
            >{totpBusy ? '…' : 'Save'}</button>
          </div>
          <div class="sshd-totp-note">
            {#if totpCode}
              Enrolled — the code right now is <strong class="mono">{totpCode}</strong>.
              If the host rejects it, the seed is wrong or the clocks disagree.
            {:else if totpHas}
              Enrolled. The app answers the one-time-code round itself, so agents need no
              human.
              <button class="sshd-totp-clear" type="button" onclick={testTotp} disabled={totpBusy}>Test against host</button>
              ·
              <button class="sshd-totp-clear" type="button" onclick={clearTotp} disabled={totpBusy}>Remove</button>
            {:else}
              Without this the app asks you for the code on every connect, and an agent
              cannot authenticate at all. Like any stored secret, it makes the host rely on
              this machine alone.
            {/if}
            {#if totpTest}
              <div class="sshd-totp-result" class:ok={totpTest.ok}>{totpTest.detail}</div>
            {/if}
          </div>
          <div class="sshd-totp-actions">
            <button class="sshd-totp-help-toggle" type="button" onclick={() => (showTotpHelp = !showTotpHelp)}>
              {showTotpHelp ? 'Hide' : 'Where do I get the seed?'}
            </button>
            <span class="sshd-totp-sep">·</span>
            <button class="sshd-totp-help-toggle" type="button" onclick={() => qrInput?.click()} disabled={totpBusy}>
              Import QR screenshot
            </button>
            <input
              class="sshd-totp-file"
              type="file"
              accept="image/*"
              bind:this={qrInput}
              onchange={(e) => {
                const f = e.currentTarget.files?.[0];
                if (f) void importQr(f);
              }}
            />
          </div>
          {#if showTotpHelp}
            <div class="sshd-totp-help">
              The host never shows an existing seed — it is only visible while enrolling.
              For JumpServer: <em>个人设置 → MFA/OTP → 重新绑定</em>, and the QR it shows
              encodes <span class="mono">otpauth://totp/…?secret=…</span>. Paste that URI
              here (or just the <span class="mono">secret</span> part). Scan the same QR into
              your phone as well if you want both to keep working — one seed, several
              authenticators. Rebinding invalidates the previous binding, so do it while you
              have the QR in front of you.
            </div>
          {/if}
        </div>
      {:else}
        <div class="sshd-block">
          <span class="sshd-label">Routing</span>
          <div class="sshd-tile-row">
            <button type="button" class="sshd-tile" class:active={connectionMode === 'direct'} onclick={() => (connectionMode = 'direct')}>
              <span class="sshd-tile-icon"><svg viewBox="0 0 24 24" width="15" height="15" fill="none"><path d="M4 12h14M14 7l5 5-5 5" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg></span>
              <span class="sshd-tile-label">Direct</span>
            </button>
            <button type="button" class="sshd-tile" class:active={connectionMode === 'jump'} onclick={() => (connectionMode = 'jump')}>
              <span class="sshd-tile-icon"><svg viewBox="0 0 24 24" width="15" height="15" fill="none"><circle cx="5" cy="17" r="2" stroke="currentColor" stroke-width="1.6"/><circle cx="12" cy="7" r="2" stroke="currentColor" stroke-width="1.6"/><circle cx="19" cy="17" r="2" stroke="currentColor" stroke-width="1.6"/><path d="M6.6 15.5L10.6 8.5M13.4 8.5L17.4 15.5" stroke="currentColor" stroke-width="1.6"/></svg></span>
              <span class="sshd-tile-label">Jump host</span>
            </button>
            <button type="button" class="sshd-tile" class:active={connectionMode === 'command'} onclick={() => (connectionMode = 'command')}>
              <span class="sshd-tile-icon"><svg viewBox="0 0 24 24" width="15" height="15" fill="none"><rect x="3" y="5" width="18" height="14" rx="2" stroke="currentColor" stroke-width="1.6"/><path d="M7 10l3 2-3 2M12 14h5" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg></span>
              <span class="sshd-tile-label">Proxy cmd</span>
            </button>
          </div>
        </div>

        {#if connectionMode === 'direct'}
          <div class="sshd-info">Direct TCP to the host. No bastion or proxy.</div>
        {:else if connectionMode === 'jump'}
          <div class="sshd-block">
            <span class="sshd-label">Jump profile</span>
            <div class="sshd-select-wrap">
              <select class="sshd-input mono sshd-select" bind:value={jumpProfileId}>
                <option value="">— Select a jump profile —</option>
                {#each $sshProfiles.filter((p) => p.id !== profile.id) as p (p.id)}<option value={p.id}>{p.name}</option>{/each}
              </select>
              <svg class="sshd-select-chev" viewBox="0 0 24 24" width="14" height="14" fill="none" aria-hidden="true"><path d="M6 9l6 6 6-6" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/></svg>
            </div>
            <span class="sshd-caption">Connect to this profile first, then open a tunneled channel to the destination. For multi-hop chains, set the jump profile's own jump host.</span>
          </div>
        {:else}
          <div class="sshd-block">
            <span class="sshd-label">Proxy command</span>
            <input class="sshd-input mono" type="text" bind:value={proxyCommand} placeholder="cloudflared access ssh --hostname %h" autocomplete="off" spellcheck="false" />
            <span class="sshd-caption"><code>%h</code> host · <code>%p</code> port · <code>%r</code> user · tokenized as argv, no shell.</span>
          </div>
        {/if}
      {/if}

      <div class="sshd-footer">
        <span class="sshd-keychain-note" title="Secrets stay in your system keychain. Blank passphrase / password keeps the existing one.">
          <svg viewBox="0 0 24 24" width="11" height="11" fill="none" aria-hidden="true"><rect x="5" y="11" width="14" height="9" rx="2" stroke="currentColor" stroke-width="1.6"/><path d="M8 11V8a4 4 0 018 0v3" stroke="currentColor" stroke-width="1.6"/></svg>
          blank keeps existing
        </span>
        <div class="sshd-spacer"></div>
        <button type="button" class="sshd-btn outline" onclick={() => (show = false)}>Cancel</button>
        <button type="button" class="sshd-btn primary" onclick={handleSave} disabled={!canSave || loading}>
          {#if loading}<svg viewBox="0 0 24 24" width="13" height="13" fill="none" class="sshd-spin" aria-hidden="true"><circle cx="12" cy="12" r="8" stroke="currentColor" stroke-width="2" stroke-opacity="0.3"/><path d="M12 4a8 8 0 018 8" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg>Saving…{:else}Save changes{/if}
        </button>
      </div>
    </div>
  {/if}
</Modal>

<style>
  .sshd-totp-note {
    margin-top: 6px;
    font-size: 11px;
    line-height: 1.5;
    color: var(--t3);
  }
  .sshd-totp-result {
    margin-top: 6px;
    font-size: 11px;
    line-height: 1.5;
    color: var(--danger, #e05a5a);
  }
  .sshd-totp-result.ok {
    color: var(--success, #4caf7d);
  }
  .sshd-totp-actions {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 6px;
  }
  .sshd-totp-sep {
    color: var(--t4, var(--t3));
    font-size: 11px;
  }
  .sshd-totp-file {
    display: none;
  }
  .sshd-totp-help-toggle {
    margin-top: 0;
    background: none;
    border: none;
    padding: 0;
    color: var(--t3);
    font-size: 11px;
    cursor: pointer;
    text-decoration: underline;
  }
  .sshd-totp-help {
    margin-top: 4px;
    padding: 8px 10px;
    border-left: 2px solid var(--b1);
    font-size: 11px;
    line-height: 1.55;
    color: var(--t3);
  }
  .sshd-totp-clear {
    background: none;
    border: none;
    padding: 0;
    margin-left: 4px;
    color: var(--acc);
    font-size: 11px;
    cursor: pointer;
    text-decoration: underline;
  }
  .sshd-root {
    display: flex;
    flex-direction: column;
    gap: 14px;
    margin: -4px 0 -4px;
  }

  /* ── Tabs (underline style, matches Agent NewSessionModal) ──────── */
  .sshd-tabs {
    display: flex;
    gap: 4px;
    margin: -4px -4px 4px;
    border-bottom: 1px solid var(--b1);
  }
  .sshd-tab {
    position: relative;
    background: transparent;
    border: none;
    padding: 10px 16px;
    font-family: var(--ui);
    font-size: 13px;
    color: var(--t3);
    cursor: default;
    transition: color 0.12s;
    border-radius: 0;
    display: inline-flex;
    align-items: center;
    gap: 7px;
  }
  .sshd-tab:hover { color: var(--t1); }
  .sshd-tab.active {
    color: var(--t1);
    font-weight: 600;
  }
  .sshd-tab.active::after {
    content: '';
    position: absolute;
    left: 12px;
    right: 12px;
    bottom: -1px;
    height: 2px;
    background: var(--ssh, var(--acc));
    border-radius: 2px 2px 0 0;
  }
  .sshd-tab-dot {
    width: 6px;
    height: 6px;
    border-radius: 99px;
    background: var(--ssh, var(--acc));
  }

  /* ── Labels + inputs ─────────────────────────────────────────────── */
  .sshd-block {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
  }
  .sshd-block.grow { flex: 1 1 0; }
  .sshd-block.narrow { flex: 0 0 110px; }
  .sshd-row { display: flex; gap: 10px; }
  .sshd-label {
    font-family: var(--mono);
    font-size: 10.5px;
    font-weight: 500;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: var(--t3);
  }
  .sshd-optional {
    text-transform: none;
    letter-spacing: 0;
    font-weight: normal;
    color: var(--t3);
    font-size: 10.5px;
  }
  .sshd-input {
    width: 100%;
    box-sizing: border-box;
    background: var(--e);
    border: 1px solid var(--b1);
    border-radius: 9px;
    padding: 9px 12px;
    color: var(--t1);
    font-size: 13.5px;
    font-family: var(--ui);
    outline: none;
    transition: border-color 0.15s, box-shadow 0.15s;
  }
  .sshd-input.mono { font-family: var(--mono); }
  .sshd-input::placeholder { color: var(--t3); }
  .sshd-input:focus {
    border-color: var(--ssh, var(--acc));
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--ssh, var(--acc)) 16%, transparent);
  }
  .sshd-caption {
    font-family: var(--ui);
    font-size: 11.5px;
    color: var(--t3);
    line-height: 1.5;
  }
  .sshd-caption code {
    font-family: var(--mono);
    color: var(--t2);
    padding: 0 4px;
    border-radius: 4px;
    background: var(--e);
    font-size: 10.5px;
  }

  /* ── Input with suffix button ────────────────────────────────────── */
  .sshd-with-suffix { position: relative; }
  .sshd-with-suffix .sshd-input { padding-right: 40px; }
  .sshd-suffix-btn {
    position: absolute;
    right: 5px;
    top: 50%;
    transform: translateY(-50%);
    width: 28px;
    height: 28px;
    display: grid;
    place-items: center;
    background: var(--c);
    border: 1px solid var(--b1);
    border-radius: 7px;
    color: var(--t2);
    cursor: default;
    transition: color 0.15s, border-color 0.15s;
  }
  .sshd-suffix-btn:hover { color: var(--t1); border-color: var(--b2); }

  /* ── Tile row ────────────────────────────────────────────────────── */
  .sshd-tile-row {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 6px;
  }
  .sshd-tile {
    display: flex;
    flex-direction: row;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 9px 10px;
    background: var(--e);
    border: 1px solid var(--b1);
    border-radius: 9px;
    cursor: default;
    transition: border-color 0.15s, background 0.15s;
  }
  .sshd-tile:hover { border-color: var(--b2); }
  .sshd-tile.active {
    background: color-mix(in srgb, var(--ssh, var(--acc)) 12%, transparent);
    border-color: var(--ssh, var(--acc));
  }
  .sshd-tile-icon {
    line-height: 0;
    color: var(--t2);
    opacity: 0.85;
    transition: color 0.15s, opacity 0.15s;
  }
  .sshd-tile:hover .sshd-tile-icon { opacity: 1; }
  .sshd-tile.active .sshd-tile-icon { color: var(--ssh, var(--acc)); opacity: 1; }
  .sshd-tile-label {
    font-family: var(--ui);
    font-size: 12.5px;
    font-weight: 500;
    color: var(--t2);
  }
  .sshd-tile.active .sshd-tile-label { color: var(--t1); }

  /* ── Select ──────────────────────────────────────────────────────── */
  .sshd-select-wrap { position: relative; }
  .sshd-select {
    appearance: none;
    -webkit-appearance: none;
    padding-right: 34px;
    cursor: default;
  }
  .sshd-select-chev {
    position: absolute;
    right: 12px;
    top: 50%;
    transform: translateY(-50%);
    pointer-events: none;
    color: var(--t3);
  }

  /* ── Info card ───────────────────────────────────────────────────── */
  .sshd-info {
    padding: 9px 12px;
    background: var(--e);
    border: 1px solid var(--b1);
    border-radius: 8px;
    font-family: var(--ui);
    font-size: 11.5px;
    color: var(--t2);
    line-height: 1.5;
  }
  .sshd-info code {
    font-family: var(--mono);
    color: var(--t1);
    padding: 1px 4px;
    border-radius: 3px;
    background: var(--c);
    font-size: 10.5px;
  }

  /* ── Footer ──────────────────────────────────────────────────────── */
  .sshd-footer {
    display: flex;
    align-items: center;
    gap: 10px;
    padding-top: 14px;
    margin-top: 2px;
    border-top: 1px solid var(--b1);
  }
  .sshd-spacer { flex: 1; }
  .sshd-keychain-note {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-family: var(--ui);
    font-size: 11px;
    color: var(--t3);
    padding: 3px 9px;
    background: color-mix(in srgb, var(--ssh, var(--acc)) 8%, transparent);
    border: 1px solid color-mix(in srgb, var(--ssh, var(--acc)) 18%, transparent);
    border-radius: 99px;
  }
  .sshd-keychain-note > svg { flex-shrink: 0; color: var(--ssh, var(--acc)); }
  .sshd-btn {
    height: 34px;
    padding: 0 16px;
    border-radius: 9px;
    font-family: var(--ui);
    font-size: 12.5px;
    font-weight: 500;
    cursor: default;
    display: inline-flex;
    align-items: center;
    gap: 7px;
    transition: background 0.15s, border-color 0.15s, color 0.15s, opacity 0.15s;
  }
  .sshd-btn.outline {
    background: transparent;
    border: 1px solid var(--b1);
    color: var(--t2);
  }
  .sshd-btn.outline:hover { border-color: var(--b2); color: var(--t1); }
  .sshd-btn.primary {
    background: var(--ssh, var(--acc));
    border: 1px solid var(--ssh, var(--acc));
    color: #fff;
    font-weight: 600;
    padding: 0 20px;
    box-shadow: 0 6px 16px -8px color-mix(in srgb, var(--ssh, var(--acc)) 80%, transparent);
  }
  .sshd-btn.primary:hover:not(:disabled) { filter: brightness(1.05); }
  .sshd-btn.primary:disabled { opacity: 0.45; }
  .sshd-spin { animation: sshd-spin 0.7s linear infinite; }
  @keyframes sshd-spin { to { transform: rotate(360deg); } }

  @media (max-width: 520px) {
    .sshd-row { flex-direction: column; }
    .sshd-block.narrow { flex: 1 1 auto; }
  }
</style>
