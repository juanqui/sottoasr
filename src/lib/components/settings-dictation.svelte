<script lang="ts">
  import { onMount } from 'svelte';
  import { settingsStore } from '../stores/settings.svelte';
  import { listInputDevices } from '../utils/tauri';
  import SettingsToggle from './settings-toggle.svelte';
  import SettingsSpeechEngine from './settings-speech-engine.svelte';
  import DownloadProgressBar from './download-progress.svelte';
  import type { InputDevice } from '../utils/tauri';
  import type { CleanupSetup } from '../stores/cleanup-setup.svelte';
  let { cleanup }: { cleanup: CleanupSetup } = $props();
  let devices = $state<InputDevice[] | null>(null);
  let devicesError = $state('');
  let deviceRequest = 0;
  let selectedDevice = $derived(settingsStore.current.input_device ?? '');
  let systemDefault = $derived(devices?.find((device) => device.is_default)?.name);
  let connected = $derived(devices !== null && devices.some((device) => device.name === selectedDevice));

  function toggleCleanup(enabled: boolean) {
    if (enabled) void cleanup.enable(() => settingsStore.update('llm_cleanup_enabled', true));
    else { cleanup.cancel(); settingsStore.update('llm_cleanup_enabled', false); }
  }

  async function refreshDevices() {
    const request = ++deviceRequest;
    try {
      const listed = await listInputDevices();
      if (request === deviceRequest) { devices = listed; devicesError = ''; }
    } catch (error) {
      if (request === deviceRequest) devicesError = String(error);
    }
  }

  onMount(() => {
    const refresh = () => { void refreshDevices(); };
    refresh();
    window.addEventListener('focus', refresh);
    return () => { ++deviceRequest; window.removeEventListener('focus', refresh); };
  });
</script>

<SettingsSpeechEngine />

<section class="setting-card">
  <h3>Recording</h3>
  <label class="setting-field">
    <span class="field-label">Microphone</span>
    <select class="setting-select" value={selectedDevice} onchange={(event) => settingsStore.update('input_device', event.currentTarget.value || null)}>
      <option value="">{systemDefault ? `System default (${systemDefault})` : 'System default'}</option>
      {#each devices ?? [] as device (device.name)}
        <option value={device.name}>{device.name}</option>
      {/each}
      {#if selectedDevice && !connected}
        <option value={selectedDevice}>{devices ? `${selectedDevice} (not connected)` : selectedDevice}</option>
      {/if}
    </select>
  </label>
  {#if selectedDevice && devices && !connected}
    <p class="setting-warning" role="status">This microphone is not connected. Recording will not start until you connect it or choose another microphone.</p>
  {:else}
    <p class="setting-hint">System default follows the input you choose in macOS Sound settings.</p>
  {/if}
  {#if devicesError}
    <div class="setting-error" role="alert">
      <p>{devicesError}</p>
      <button class="secondary-button" type="button" onclick={() => refreshDevices()}>Try again</button>
    </div>
  {/if}
  <SettingsToggle label="Sound cues" hint="Play a short sound when recording starts, stops, or is cancelled, and when something goes wrong."
    checked={settingsStore.current.sound_cues} onchange={(value) => settingsStore.update('sound_cues', value)} />
</section>

<section class="setting-card" aria-busy={cleanup.pending}>
  <h3>Transcript cleanup</h3>
  <SettingsToggle label="AI transcript cleanup" hint="Remove clear fillers and accidental repeats. Preserve wording, facts, and order."
    checked={settingsStore.current.llm_cleanup_enabled || cleanup.pending}
    disabled={cleanup.status?.available === false}
    onchange={toggleCleanup} />
  <p class="setting-hint">Accepted cleanup is applied automatically. The original remains in History. Runs locally with MiniCPM5 2B; off by default.</p>
  {#if cleanup.pending}
    <div class="setting-status" role="status">
      <span>{cleanup.progress || cleanup.status?.downloading ? 'Downloading cleanup model…' : 'Preparing local cleanup…'}</span>
      <button class="secondary-button" type="button" onclick={() => cleanup.cancel()}>Cancel setup</button>
    </div>
    {#if cleanup.progress}<DownloadProgressBar progress={cleanup.progress} label="Cleanup model download" />{/if}
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
  <SettingsToggle label="Restore clipboard" hint="Put back what you had copied after pasting, including images and files. Items larger than 64 MB are not restored."
    checked={settingsStore.current.restore_clipboard} onchange={(value) => settingsStore.update('restore_clipboard', value)} />
  <SettingsToggle label="Return to the original app" hint="Restore focus if SottoASR took it while recording."
    checked={settingsStore.current.restore_focus_before_paste} onchange={(value) => settingsStore.update('restore_focus_before_paste', value)} />
  <SettingsToggle label="Show recording overlay" hint="Show the waveform, timer, and recording controls."
    checked={settingsStore.current.show_overlay} onchange={(value) => settingsStore.update('show_overlay', value)} />
</section>
