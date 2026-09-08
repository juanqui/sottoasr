<script lang="ts">
  import { onMount } from 'svelte';
  import { getModelStatus, initAsr } from '../utils/tauri';
  import { createEventScope } from '../utils/event-scope';
  import { cleanupOutcome } from '../utils/cleanup-outcome';
  import type { ModelStatus } from '../utils/tauri';
  import type { CleanupSetup } from '../stores/cleanup-setup.svelte';

  let { cleanup, onconfigure }: { cleanup: CleanupSetup; onconfigure: () => void } = $props();
  let asr = $state<ModelStatus | null>(null);
  let asrError = $state('');
  let retrying = $state(false);
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
  let speech = $derived.by(() => {
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
  onMount(() => {
    const scope = createEventScope((error) => { asrError = `Live model status unavailable: ${String(error)}`; });
    void Promise.all(['asr-init-started', 'asr-init-complete', 'asr-init-error'].map((event) =>
      scope.listen(event, () => { void refreshAsr(); }))).then(() => { if (!disposed) void refreshAsr(); });
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
    {#if asr && !asr.loaded && !asr.initializing}<button type="button" onclick={retryAsr} disabled={retrying}>Load speech recognition</button>{/if}
  </article>
  <article class="model-card" aria-label="AI cleanup model">
    <div class="card-heading"><h2>AI cleanup</h2><span class="model-state {ai.tone}"><i></i>{ai.label}</span></div>
    <p class="model-name">{cleanup.status?.model_name || 'MiniCPM5 2B'}</p>
    <p class="model-detail" role="status">{ai.detail}</p>
    {#if cleanup.status && !['idle', 'disabled'].includes(cleanup.status.last_cleanup_status.kind)}
      <details class:attention={lastOutcome.issue}><summary>Last recording: {lastOutcome.label}</summary><p>{lastOutcome.detail}</p></details>
    {/if}
    <button type="button" onclick={onconfigure}>Manage cleanup</button>
  </article>
</section>

<style>
  .model-dashboard { display:grid; grid-template-columns:1fr 1fr; gap:10px; padding:0 20px 16px; flex:none; }
  .model-card { padding:12px 14px; border:1px solid var(--border); border-radius:12px; background:var(--card-bg); min-width:0; }
  .card-heading { display:flex; align-items:center; gap:8px; justify-content:space-between; flex-wrap:wrap; }
  h2 { margin:0; font-size:12px; font-weight:600; color:var(--text-bright); }
  .model-state { display:inline-flex; align-items:center; gap:5px; font-size:11px; color:var(--text-dim); }
  .model-state i { width:6px; height:6px; border-radius:50%; background:currentColor; }
  .ready { color:#6ee7b7; } .pending { color:#93c5fd; } .warning, .attention { color:#fcd34d; }
  .model-name { color:var(--text); font-size:11px; margin:8px 0 4px; overflow-wrap:anywhere; }
  .model-detail, details { color:var(--text-dim); font-size:11px; line-height:1.45; margin:0; overflow-wrap:anywhere; }
  details { margin-top:8px; } summary { cursor:pointer; } details[open] { max-height:90px; overflow:auto; } details p { margin:6px 0; }
  button { margin:8px 0 0; padding:0; border:0; background:none; color:#93c5fd; font:inherit; font-size:11px; cursor:pointer; }
  button:disabled { opacity:.5; cursor:default; }
  @media(max-width:460px) { .model-dashboard { grid-template-columns:1fr; } }
</style>
