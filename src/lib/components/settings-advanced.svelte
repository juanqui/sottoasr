<script lang="ts">
  import { onMount } from 'svelte';
  import { checkAllPermissions, requestAccessibilityPermission, openMicrophoneSettings, deleteLlmModel, openUrl } from '../utils/tauri';
  import { settingsStore } from '../stores/settings.svelte';
  import ConfirmDialog from './confirm-dialog.svelte';
  import type { PermissionStatus } from '../utils/tauri';
  import type { CleanupSetup } from '../stores/cleanup-setup.svelte';
  let { cleanup }: { cleanup: CleanupSetup } = $props();
  let permissions = $state<PermissionStatus | null>(null);
  let checking = $state(false);
  let error = $state('');
  let deleting = $state(false);
  let confirmDelete = $state(false);
  let disposed = false;

  async function refreshPermissions() {
    if (checking) return;
    checking = true;
    error = '';
    try { const result = await checkAllPermissions(); if (!disposed) permissions = result; }
    catch (err) { if (!disposed) error = String(err); }
    finally { if (!disposed) checking = false; }
  }
  async function requestPermission(kind: 'microphone' | 'accessibility') {
    try { await (kind === 'microphone' ? openMicrophoneSettings() : requestAccessibilityPermission()); }
    catch (err) { if (!disposed) error = String(err); }
  }
  async function removeModel() {
    deleting = true;
    error = '';
    try { await deleteLlmModel(); confirmDelete = false; await cleanup.refresh(); }
    catch (err) { error = String(err); }
    finally { deleting = false; }
  }
  onMount(() => { void refreshPermissions(); return () => { disposed = true; }; });
</script>
<section class="setting-card">
  <h3>Permissions</h3>
  <p class="setting-hint">Microphone access records your voice. Accessibility lets SottoASR paste at your cursor.</p>
  <div class="permission-row"><span>Microphone</span><span>{permissions ? permissions.microphone === 'authorized' ? 'Allowed' : 'Needs access' : 'Checking…'}</span>
    {#if permissions && permissions.microphone !== 'authorized'}<button class="secondary-button" type="button" onclick={() => requestPermission('microphone')}>Open Settings</button>{/if}
  </div>
  <div class="permission-row"><span>Accessibility</span><span>{permissions ? permissions.accessibility_functional ? 'Allowed' : permissions.accessibility_api ? 'Restart needed' : 'Needs access' : 'Checking…'}</span>
    {#if permissions && !permissions.accessibility_functional}<button class="secondary-button" type="button" onclick={() => requestPermission('accessibility')}>Open Settings</button>{/if}
  </div>
  <button class="secondary-button" type="button" disabled={checking} onclick={refreshPermissions}>{checking ? 'Checking…' : 'Check again'}</button>
  {#if permissions?.needs_restart}<p class="setting-hint">Restart SottoASR after granting access. If pasting still fails, remove and re-add SottoASR in Privacy & Security → Accessibility.</p>{/if}
</section>
<section class="setting-card">
  <h3>Local models</h3>
  <p class="setting-hint">Speech recognition uses NVIDIA Parakeet v3. Recordings and transcripts stay on your Mac.</p>
  <div class="model-details">
    <span class="field-label">AI cleanup</span>
    {#if cleanup.status}
      <p>{cleanup.status.model_name}</p>
      <p class="setting-hint">{cleanup.status.loaded ? 'Ready in memory' : cleanup.status.downloaded ? 'Downloaded' : 'Not downloaded'} · About {cleanup.status.download_size_mb} MB</p>
      <a href={cleanup.status.model_url} onclick={(event) => { event.preventDefault(); if (cleanup.status) void openUrl(cleanup.status.model_url).catch((err) => { error = String(err); }); }}>Model details ↗</a>
      {#if cleanup.status.downloaded}
        <button class="secondary-button danger" type="button" disabled={deleting || cleanup.pending || cleanup.status.preparing || settingsStore.current.llm_cleanup_enabled || !!settingsStore.saved?.llm_cleanup_enabled} onclick={() => { confirmDelete = true; }}>Remove cleanup model</button>
        {#if settingsStore.current.llm_cleanup_enabled || settingsStore.saved?.llm_cleanup_enabled}<p class="setting-hint">Turn off cleanup and Save before removing its model.</p>{/if}
      {/if}
    {:else}<p class="setting-hint">{cleanup.loading ? 'Checking cleanup model…' : 'Model status unavailable.'}</p><button class="secondary-button" type="button" onclick={() => cleanup.refresh()}>Retry status</button>{/if}
  </div>
</section>
{#if error}<p class="setting-error" role="alert">{error}</p>{/if}
<ConfirmDialog open={confirmDelete} title="Remove cleanup model?" message="This removes the downloaded cleanup model from this Mac. Turning cleanup on again will download it. Your recordings and history are kept." confirmLabel="Remove model" busy={deleting} onconfirm={removeModel} oncancel={() => { confirmDelete = false; }} />
<style>
  .permission-row { display:flex; align-items:center; flex-wrap:wrap; gap:8px; margin:16px 0; font-size:13px; }
  .permission-row span:first-child { flex:1; font-weight:500; }
  .permission-row span:nth-child(2) { color:var(--text-dim); }
  .model-details { border-top:1px solid var(--border); margin-top:16px; padding-top:16px; }
  .model-details p { font-size:13px; overflow-wrap:anywhere; }
  .model-details a { display:block; font-size:12px; margin:12px 0; color:var(--accent); }
</style>
