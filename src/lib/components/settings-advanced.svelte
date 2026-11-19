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

  const RETENTION_DAYS = [0, 1, 7, 30, 90, 365];
  const retention = [
    { key: 'history_retention_days', label: 'Keep transcripts', hint: 'Older transcripts are removed from History, together with the audio kept with them.',
      warning: (period: string) => `Saving deletes transcripts older than ${period} and the audio kept with them. This cannot be undone.` },
    { key: 'audio_retention_days', label: 'Keep recordings', hint: 'Older recordings are deleted, including recordings waiting for recovery.',
      warning: (period: string) => `Saving deletes recordings older than ${period}, including recordings waiting for recovery. This cannot be undone.` },
  ] as const;
  const period = (days: number) => days === 1 ? '1 day' : `${days} days`;

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
  <h3>History and recordings</h3>
  <p class="setting-hint">Choose how long SottoASR keeps what you dictate. Nothing leaves your Mac.</p>
  {#each retention as item (item.key)}
    {@const days = settingsStore.current[item.key]}
    {@const savedDays = settingsStore.saved?.[item.key] ?? 0}
    {@const choices = RETENTION_DAYS.includes(days) ? RETENTION_DAYS : [...RETENTION_DAYS, days].sort((a, b) => a - b)}
    <label class="setting-field">
      <span class="field-label">{item.label}</span>
      <select class="setting-select" value={days} onchange={(event) => settingsStore.update(item.key, Number(event.currentTarget.value))}>
        {#each choices as choice (choice)}
          <option value={choice}>{choice === 0 ? 'Keep until I delete' : period(choice)}</option>
        {/each}
      </select>
    </label>
    <p class="setting-hint">{item.hint}</p>
    {#if days > 0 && (savedDays === 0 || days < savedDays)}
      <p class="setting-warning" role="status">{item.warning(period(days))}</p>
    {/if}
  {/each}
</section>
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
  <p class="setting-hint">Speech recognition uses Parakeet Ultra. Recordings and transcripts stay on your Mac.</p>
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
<section class="setting-card">
  <h3>AI cleanup strategy</h3>
  <p class="setting-hint">How the cleanup model rewrites your dictation. Applies after Save.</p>
  <div class="mode-options" role="radiogroup" aria-label="AI cleanup strategy">
    <label class="mode-option"><input type="radio" name="cleanup-mode" value="retype"
      checked={settingsStore.current.llm_cleanup_mode === 'retype'}
      onchange={() => settingsStore.update('llm_cleanup_mode', 'retype')} />
      <span><strong>Retype</strong><span class="mode-hint">The model rewrites the whole passage. Best correction quality.</span></span></label>
    <label class="mode-option"><input type="radio" name="cleanup-mode" value="replace"
      checked={settingsStore.current.llm_cleanup_mode === 'replace'}
      onchange={() => settingsStore.update('llm_cleanup_mode', 'replace')} />
      <span><strong>Replace (experimental)</strong><span class="mode-hint">The model only marks words to remove or replace. Noticeably faster on short recordings; cleans fewer issues and sometimes none. Your original text is never altered unless the safety check passes.</span></span></label>
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
  .model-details a { display:block; font-size:12px; margin:12px 0; color:var(--accent-text); }
  .mode-options { display:grid; gap:10px; margin-top:14px; }
  .mode-option { display:flex; gap:12px; align-items:flex-start; padding:12px 14px; border:1px solid var(--border); border-radius:10px; cursor:pointer; }
  .mode-option:has(input:checked) { border-color:var(--accent); }
  .mode-option input { margin-top:2px; accent-color:var(--accent); }
  .mode-option strong { display:block; color:var(--text-bright); font-size:13px; font-weight:500; }
  .mode-option > span { flex:1; display:grid; gap:4px; }
  .mode-option .mode-hint { color:var(--text-dim); font-size:12px; line-height:1.5; }
</style>
