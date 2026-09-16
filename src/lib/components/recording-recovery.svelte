<script lang="ts">
  import { onDestroy } from 'svelte';
  import { transcriptionStore } from '../stores/transcriptions.svelte';
  import { createEventScope } from '../utils/event-scope';
  import { formatDuration } from '../utils/format';
  import { getRecoverableRecordings, getRecoveryNotice, recoverRecording, revealRecoveryRecording } from '../utils/tauri';
  import type { RecoveryRecording } from '../utils/tauri';

  let items = $state<RecoveryRecording[]>([]);
  let notice = $state('');
  let noticeError = $state('');
  let loading = $state(true);
  let listError = $state('');
  let reprocessErrors = $state<Record<string, string>>({});
  let revealErrors = $state<Record<string, string>>({});
  let processingId = $state<string | null>(null);
  let status = $state('');
  let disposed = false;
  let refreshGeneration = 0;
  let noticeGeneration = 0;

  // The section must never hide a failed check or a completed action: any
  // error, notice, pending item, or status keeps it visible. An error is not
  // mistaken for "nothing pending"; a status stays after the last item goes.
  let visible = $derived(items.length > 0 || notice !== '' || noticeError !== '' || listError !== '' || status !== '');

  function createdAt(item: RecoveryRecording) {
    const date = new Date(item.created_at);
    return isNaN(date.getTime()) ? item.created_at : date.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
  }

  // Only the newest read owns the list. A response from an older read is stale:
  // it predates any reprocess that removed an item, and must not resurrect it.
  async function refresh(showErrors: boolean) {
    const generation = ++refreshGeneration;
    try {
      const fetched = await getRecoverableRecordings();
      if (disposed || generation !== refreshGeneration) return;
      items = fetched;
      listError = '';
    } catch (err) {
      if (disposed || generation !== refreshGeneration) return;
      if (showErrors) listError = `Could not check for recoverable recordings: ${String(err)}`;
    } finally {
      if (!disposed && generation === refreshGeneration) loading = false;
    }
  }

  // The notice read is independent of the list read: each failing side must
  // not hide the other side's result. Only the newest read owns each state.
  async function loadNotice() {
    const generation = ++noticeGeneration;
    try {
      const value = await getRecoveryNotice();
      if (disposed || generation !== noticeGeneration) return;
      notice = value ?? '';
      noticeError = '';
    } catch (err) {
      if (disposed || generation !== noticeGeneration) return;
      noticeError = `Could not check the last session: ${String(err)}`;
    }
  }

  async function reprocess(item: RecoveryRecording) {
    if (processingId !== null) return;
    processingId = item.id;
    status = '';
    const remaining = { ...reprocessErrors };
    delete remaining[item.id];
    reprocessErrors = remaining;
    try {
      const transcription = await recoverRecording(item.id);
      if (disposed) return;
      // The native command saves history before it returns, so the returned
      // record is durable. The store dedupes if its event arrives too.
      transcriptionStore.add(transcription);
      // Retire in-flight snapshots before removing the acknowledged recording.
      ++refreshGeneration;
      items = items.filter((entry) => entry.id !== item.id);
      status = `Transcription saved to history. Audio retained at ${item.audio_path}`;
      void refresh(true);
    } catch (err) {
      // Retryable: the audio and the list entry both stay in place.
      if (!disposed) reprocessErrors = { ...reprocessErrors, [item.id]: `Could not reprocess: ${String(err)}` };
    } finally {
      if (!disposed) processingId = null;
    }
  }

  async function showInFinder(item: RecoveryRecording) {
    const remaining = { ...revealErrors };
    delete remaining[item.id];
    revealErrors = remaining;
    try {
      await revealRecoveryRecording(item.id);
    } catch (err) {
      if (!disposed) revealErrors = { ...revealErrors, [item.id]: `Finder did not open: ${String(err)}` };
    }
  }

  const scope = createEventScope((err) => { listError = `Recovery update failed: ${String(err)}`; });
  // Register the listener before the first read. A native startup event that
  // fires during mount still reaches this window, so nothing goes missed.
  void scope.listen('recovery-recordings-changed', () => refresh(true)).then(() => refresh(true));
  void loadNotice();

  onDestroy(() => { disposed = true; scope.dispose(); });
</script>

{#if visible}
  <section class="recovery" aria-label="Recoverable recordings">
    <h2>Recoverable recordings</h2>
    {#if items.length}
      <p class="recovery-lead">These recordings need recovery after an interrupted operation. The last audio checkpoint can omit the ending. Reprocess saves to History without pasting.</p>
    {/if}
    {#if notice}<p class="recovery-notice" role="status">{notice}</p>{/if}
    {#if noticeError}
      <p class="recovery-error" role="alert">{noticeError}
        <button type="button" onclick={() => loadNotice()}>Retry</button>
      </p>
    {/if}
    {#if listError}
      <p class="recovery-error" role="alert">{listError}
        <button type="button" disabled={loading} onclick={() => refresh(true)}>Retry</button>
      </p>
    {/if}
    {#if status}<p class="recovery-status" role="status">{status}</p>{/if}
    {#each items as item (item.id)}
      <div class="recovery-item">
        <div class="recovery-meta">
          <span class="recovery-created" title={item.created_at}>{createdAt(item)}</span>
          {#if item.duration_ms !== null}<span class="recovery-sep">&middot;</span><span class="recovery-duration">{formatDuration(item.duration_ms)}</span>{/if}
        </div>
        <p class="recovery-path" title="Audio file location">{item.audio_path}</p>
        {#if item.error}<p class="recovery-item-error">{item.error}</p>{/if}
        {#if reprocessErrors[item.id]}<p class="recovery-item-error" role="alert">{reprocessErrors[item.id]}</p>{/if}
        {#if revealErrors[item.id]}<p class="recovery-item-error" role="alert">{revealErrors[item.id]}</p>{/if}
        <div class="recovery-actions">
          <button type="button" class="reprocess" disabled={processingId !== null || !!item.error} onclick={() => reprocess(item)}>{processingId === item.id ? 'Reprocessing…' : 'Reprocess'}</button>
          <button type="button" class="reveal" onclick={() => showInFinder(item)}>Show in Finder</button>
        </div>
      </div>
    {/each}
  </section>
{/if}

<style>
  .recovery {
    margin: 0;
    padding: 12px 14px;
    border: 1px solid #fbbf2438;
    border-radius: 10px;
    background: #fbbf2408;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  h2 { margin: 0; font-size: 13px; font-weight: 600; color: #fcd34d; }
  .recovery-lead, .recovery-notice { margin: 0; font-size: 12px; line-height: 1.5; color: var(--text-dim); }
  .recovery-notice { color: var(--text); }
  .recovery-error { margin: 0; padding: 8px; border-radius: 8px; background: #ef444410; color: #fca5a5; font-size: 12px; overflow-wrap: anywhere; }
  .recovery-error button { margin-left: 8px; padding: 4px 8px; background: var(--card-bg); color: var(--text); border: 1px solid var(--border-hover); border-radius: 6px; font: inherit; font-size: 12px; cursor: pointer; }
  .recovery-status { margin: 0; font-size: 12px; color: var(--accent); overflow-wrap: anywhere; user-select: text; }
  .recovery-item { display: flex; flex-direction: column; gap: 4px; padding: 10px 12px; border: 1px solid var(--border); border-radius: 8px; background: var(--card-bg); }
  .recovery-meta { display: flex; gap: 6px; flex-wrap: wrap; align-items: baseline; font-size: 12px; color: var(--text-dim); }
  .recovery-created { color: var(--text); }
  .recovery-path { margin: 0; font-family: var(--mono); font-size: 11px; color: var(--text-dim); user-select: text; cursor: text; white-space: normal; overflow-wrap: anywhere; }
  .recovery-item-error { margin: 0; font-size: 11px; color: #fca5a5; line-height: 1.5; overflow-wrap: anywhere; user-select: text; }
  .recovery-actions { display: flex; gap: 8px; flex-wrap: wrap; margin-top: 2px; }
  .recovery-actions button { padding: 5px 9px; border: 1px solid var(--border); border-radius: 6px; font: inherit; font-size: 11px; background: var(--input-bg); color: var(--text-bright); cursor: pointer; transition: border-color 0.15s ease; }
  .recovery-actions button:hover:not(:disabled) { border-color: var(--border-hover); }
  .recovery-actions .reprocess:not(:disabled):hover { color: var(--accent); border-color: var(--accent); }
  .recovery-actions button:disabled { opacity: 0.5; cursor: default; }
</style>
