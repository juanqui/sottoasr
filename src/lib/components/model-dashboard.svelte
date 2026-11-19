<script lang="ts">
  import { onMount } from 'svelte';
  import { getModelStatus, initAsr, redownloadSpeechModel } from '../utils/tauri';
  import { createEventScope } from '../utils/event-scope';
  import { cleanupOutcome } from '../utils/cleanup-outcome';
  import ConfirmDialog from './confirm-dialog.svelte';
  import DownloadProgressBar from './download-progress.svelte';
  import type { DownloadProgress, ModelStatus } from '../utils/tauri';
  import type { CleanupSetup } from '../stores/cleanup-setup.svelte';

  let { cleanup, onconfigure }: { cleanup: CleanupSetup; onconfigure: () => void } = $props();
  let asr = $state<ModelStatus | null>(null);
  let asrError = $state('');
  let retrying = $state(false);
  let confirmRedownload = $state(false);
  let redownloading = $state(false);
  let redownloadNotice = $state('');
  let redownloadError = $state('');
  let redownloadProgress = $state<DownloadProgress | null>(null);
  let disposed = false;
  let refreshing = false;
  let lastOutcome = $derived(cleanupOutcome(cleanup.status?.last_cleanup_status));
  let ai = $derived.by(() => {
    const status = cleanup.status;
    if (!status) return { label: cleanup.error ? 'Status unavailable' : 'Checking…', tone: 'neutral', detail: cleanup.error || 'Reading local model status.' };
    if (!status.available) return { label: 'Unavailable', tone: 'warning', detail: status.unavailable_reason || 'Unsupported device.' };
    if (status.preparing || status.downloading || cleanup.pending) return { label: 'Preparing…', tone: 'pending', detail: 'Downloading, loading, and warming the local model.' };
    if (status.enabled === false) return { label: 'Off', tone: 'neutral', detail: status.loaded ? 'Loaded in memory. Cleanup is off in saved settings.' : 'Optional cleanup is off in saved settings.' };
    if (status.setup_error) return { label: 'Needs attention', tone: 'warning', detail: status.setup_error };
    if (status.loaded) return { label: status.busy ? 'Working…' : 'Ready', tone: 'ready', detail: 'Loaded and prewarmed locally with MLX.' };
    return { label: status.downloaded ? 'Not loaded' : 'Not downloaded', tone: 'warning', detail: status.downloaded ? 'The next recording will load it, or prepare it in Dictation.' : 'Enable cleanup in Dictation to prepare the model.' };
  });
  let cleanupNeedsRepair = $derived.by(() => {
    const status = cleanup.status;
    if (!status) return false;
    // Only the runtime-failure states: a `failed` cleanup may be a per-request
    // problem that rebuilding the runtime would not address.
    return Boolean(status.setup_error) || status.last_cleanup_status.kind === 'unavailable';
  });
  let speech = $derived.by(() => {
    if (redownloading && !asr?.initializing) return { label: 'Downloading…', tone: 'pending', detail: 'Replacing the model files that failed the check.' };
    if (retrying || asr?.initializing) return { label: 'Loading…', tone: 'pending', detail: 'Preparing speech recognition.' };
    if (asrError || asr?.error) return { label: 'Needs attention', tone: 'warning', detail: asrError || asr?.error || '' };
    if (!asr) return { label: 'Checking…', tone: 'neutral', detail: 'Reading local model status.' };
    if (asr.loaded) return { label: 'Ready', tone: 'ready', detail: 'Loaded locally and ready to transcribe.' };
    return { label: asr.downloaded ? 'Not loaded' : 'Not downloaded', tone: 'warning', detail: 'Load speech recognition to start dictating.' };
  });

  async function refreshAsr() {
    if (refreshing || disposed) return;
    refreshing = true;
    try {
      const next = await getModelStatus();
      if (!disposed) { asr = next; asrError = ''; }
    } catch (error) {
      if (!disposed) { asr = null; asrError = `Could not read speech recognition status: ${String(error)}`; }
    } finally { refreshing = false; }
  }
  async function retryAsr() {
    if (retrying) return;
    retrying = true;
    try { await initAsr(); }
    catch (error) { if (!disposed) asrError = String(error); }
    finally { if (!disposed) { retrying = false; void refreshAsr(); } }
  }
  async function redownload() {
    if (redownloading) return;
    redownloading = true;
    // Close the confirmation so the card's progress bar is visible.
    confirmRedownload = false;
    redownloadNotice = '';
    redownloadError = '';
    redownloadProgress = null;
    try {
      await redownloadSpeechModel();
      if (!disposed) redownloadNotice = 'Model files replaced and verified.';
    } catch (error) {
      if (!disposed) redownloadError = `Could not replace the model files: ${String(error)}`;
    } finally { if (!disposed) { redownloading = false; redownloadProgress = null; void refreshAsr(); } }
  }
  onMount(() => {
    const scope = createEventScope((error) => { asrError = `Live model status unavailable: ${String(error)}`; });
    // The re-download reloads the engine once its files are verified, so any
    // of these events ends its download phase.
    void Promise.all(['asr-init-started', 'asr-init-complete', 'asr-init-error'].map((event) =>
      scope.listen(event, () => { redownloadProgress = null; void refreshAsr(); }))).then(() => { if (!disposed) void refreshAsr(); });
    void scope.listen<DownloadProgress>('model-download-progress', (event) => { if (redownloading) redownloadProgress = event.payload; });
    void scope.listen('transcription-complete', () => { void cleanup.refresh(); });
    const refresh = () => {
      if (!document.hidden) { void refreshAsr(); void cleanup.refresh(); }
    };
    const timer = setInterval(refresh, 5000);
    document.addEventListener('visibilitychange', refresh);
    window.addEventListener('focus', refresh);
    return () => { disposed = true; clearInterval(timer); scope.dispose(); document.removeEventListener('visibilitychange', refresh); window.removeEventListener('focus', refresh); };
  });
</script>

<section class="model-dashboard" aria-label="Local model status">
  <article class="model-card" aria-label="Speech recognition model">
    <div class="card-heading"><h2>Speech recognition</h2><span class="model-state {speech.tone}"><i></i>{speech.label}</span></div>
    <p class="model-name">{asr?.name || 'Parakeet TDT v3'}</p>
    <p class="model-detail" role="status">{speech.detail}</p>
    {#if redownloading && redownloadProgress}<DownloadProgressBar progress={redownloadProgress} label="Model file download" />{/if}
    {#if asr && !asr.loaded && !asr.initializing}<button type="button" onclick={retryAsr} disabled={retrying || redownloading}>Load speech recognition</button>{/if}
    {#if asr?.integrity_warning && !redownloading}
      <p class="integrity" role="alert">{asr.integrity_warning}</p>
      <button type="button" onclick={() => { confirmRedownload = true; }} disabled={retrying || !!asr.initializing}>Re-download speech model</button>
    {/if}
    {#if redownloadError}<p class="integrity" role="alert">{redownloadError}</p>{/if}
    {#if redownloadNotice}<p class="model-detail" role="status">{redownloadNotice}</p>{/if}
  </article>
  <article class="model-card" aria-label="AI cleanup model">
    <div class="card-heading"><h2>AI cleanup</h2><span class="model-state {ai.tone}"><i></i>{ai.label}</span></div>
    <p class="model-name">{cleanup.status?.model_name || 'MiniCPM5 2B'}</p>
    <p class="model-detail" role="status">{ai.detail}</p>
    {#if cleanup.status && !['idle', 'disabled'].includes(cleanup.status.last_cleanup_status.kind)}
      <details class:attention={lastOutcome.issue}><summary>Last recording: {lastOutcome.label}</summary><p>{lastOutcome.detail}</p></details>
    {/if}
    <div class="actions">
      {#if cleanupNeedsRepair}
        <button type="button" onclick={() => void cleanup.repair()} disabled={cleanup.pending}>Repair cleanup runtime</button>
      {/if}
      <button type="button" onclick={onconfigure}>Manage cleanup</button>
    </div>
  </article>
</section>
<ConfirmDialog open={confirmRedownload} title="Re-download speech model?" message="SottoASR downloads fresh copies of the files that failed the check, verifies them, and reloads speech recognition. This can be several hundred megabytes. Keep SottoASR open until it finishes." confirmLabel="Re-download" busy={redownloading} onconfirm={redownload} oncancel={() => { confirmRedownload = false; }} />

<style>
  .model-dashboard { display:grid; grid-template-columns:1fr 1fr; gap:10px; padding:0 20px 16px; flex:none; }
  .model-card { padding:12px 14px; border:1px solid var(--border); border-radius:12px; background:var(--card-bg); min-width:0; }
  .card-heading { display:flex; align-items:center; gap:8px; justify-content:space-between; flex-wrap:wrap; }
  h2 { margin:0; font-size:12px; font-weight:600; color:var(--text-bright); }
  .model-state { display:inline-flex; align-items:center; gap:5px; font-size:11px; color:var(--text-dim); }
  .model-state i { width:6px; height:6px; border-radius:50%; background:currentColor; }
  .ready { color:var(--success); } .pending { color:var(--accent-text); } .warning, .attention { color:var(--warning); }
  .model-name { color:var(--text); font-size:11px; margin:8px 0 4px; overflow-wrap:anywhere; }
  .model-detail, details { color:var(--text-dim); font-size:11px; line-height:1.45; margin:0; overflow-wrap:anywhere; }
  details { margin-top:8px; } summary { cursor:pointer; } details[open] { max-height:90px; overflow:auto; } details p { margin:6px 0; }
  button { margin:8px 0 0; padding:0; border:0; background:none; color:var(--accent-text); font:inherit; font-size:11px; cursor:pointer; }
  .integrity { color:var(--warning); background:var(--warning-bg); font-size:11px; line-height:1.45; margin:8px 0 0; padding:6px 8px; border-radius:6px; overflow-wrap:anywhere; }
  .actions { display:flex; gap:12px; flex-wrap:wrap; }
  button:disabled { opacity:.5; cursor:default; }
  @media(max-width:460px) { .model-dashboard { grid-template-columns:1fr; } }
</style>
