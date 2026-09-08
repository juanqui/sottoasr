<script lang="ts">
  import { settingsStore } from '../stores/settings.svelte';
  import SettingsToggle from './settings-toggle.svelte';
  import type { CleanupSetup } from '../stores/cleanup-setup.svelte';
  let { cleanup }: { cleanup: CleanupSetup } = $props();

  function toggleCleanup(enabled: boolean) {
    if (enabled) void cleanup.enable(() => settingsStore.update('llm_cleanup_enabled', true));
    else { cleanup.cancel(); settingsStore.update('llm_cleanup_enabled', false); }
  }
</script>

<section class="setting-card" aria-busy={cleanup.pending}>
  <h3>Transcript cleanup</h3>
  <SettingsToggle label="AI transcript cleanup" hint="Remove clear fillers and accidental repeats. Preserve wording, facts, and order."
    checked={settingsStore.current.llm_cleanup_enabled || cleanup.pending}
    disabled={cleanup.status?.available === false}
    onchange={toggleCleanup} />
  <p class="setting-hint">Accepted cleanup is applied automatically. The original remains in History. Runs locally with MiniCPM5 2B; off by default.</p>
  {#if cleanup.pending}
    <div class="setting-status" role="status">
      <span>{cleanup.status?.downloading ? 'Downloading cleanup model…' : 'Preparing local cleanup…'}</span>
      <button class="secondary-button" type="button" onclick={() => cleanup.cancel()}>Cancel setup</button>
    </div>
    <p class="setting-hint">This may download the model and prepare its runtime. Your other settings remain available.</p>
  {:else if cleanup.status?.available === false}
    <p class="setting-hint">{cleanup.status.unavailable_reason ?? 'Cleanup is unavailable on this device.'}</p>
  {:else if !cleanup.status?.downloaded}
    <p class="setting-hint">Turning this on handles setup automatically{cleanup.status ? `, including a one-time ${cleanup.status.download_size_mb} MB download` : ''}.</p>
  {/if}
  {#if settingsStore.saved?.llm_cleanup_enabled && !cleanup.pending && cleanup.status?.downloaded && !cleanup.status.loaded && !cleanup.error}
    <button class="secondary-button" type="button" onclick={() => cleanup.prepare()}>Prepare cleanup now</button>
  {/if}
  {#if cleanup.error}
    <div class="setting-error" role="alert">
      <p>{cleanup.error}</p>
      <button class="secondary-button" type="button" onclick={() => toggleCleanup(true)}>Retry setup</button>
    </div>
  {/if}
  {#if cleanup.notice}<p class="setting-hint" role="status">{cleanup.notice}</p>{/if}
</section>

<section class="setting-card">
  <h3>After recording</h3>
  <SettingsToggle label="Paste automatically" hint="Insert the transcript at your cursor. Turn off to copy it instead."
    checked={settingsStore.current.auto_paste} onchange={(value) => settingsStore.update('auto_paste', value)} />
  <SettingsToggle label="Restore clipboard text" hint="Restore previous text after pasting. Images and files cannot be restored."
    checked={settingsStore.current.restore_clipboard} onchange={(value) => settingsStore.update('restore_clipboard', value)} />
  <SettingsToggle label="Return to the original app" hint="Restore focus if SottoASR took it while recording."
    checked={settingsStore.current.restore_focus_before_paste} onchange={(value) => settingsStore.update('restore_focus_before_paste', value)} />
  <SettingsToggle label="Show recording overlay" hint="Show the waveform, timer, and recording controls."
    checked={settingsStore.current.show_overlay} onchange={(value) => settingsStore.update('show_overlay', value)} />
</section>
