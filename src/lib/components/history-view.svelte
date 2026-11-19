<script lang="ts">
  import { onMount } from 'svelte';
  import { writeText } from '@tauri-apps/plugin-clipboard-manager';
  import { transcriptionStore } from '../stores/transcriptions.svelte';
  import { exportTranscriptionsCsvFile, revealCsvExport } from '../utils/tauri';
  import { createEventScope } from '../utils/event-scope';
  import { cleanupOutcome } from '../utils/cleanup-outcome';
  import { minuteNow } from '../utils/minute-clock';
  import RecordingRecovery from './recording-recovery.svelte';
  import HistoryItem from './history-item.svelte';
  import ConfirmDialog from './confirm-dialog.svelte';
  import type { Transcription, TranscriptionEvent } from '../utils/tauri';

  const PAGE_SIZE = 50;
  let searchQuery = $state('');
  let outcomeFilter = $state<'all' | 'cleaned' | 'issues'>('all');
  let page = $state(0);
  let rowViews = $state<Record<string, { expanded: boolean; viewMode: 'cleaned' | 'raw' | 'diff' }>>({});
  let error = $state('');
  let exportedPath = $state<string | null>(null);
  let deleting = $state(false);
  let exporting = $state(false);
  let deleteTarget = $state<string | null>(null);
  let disposed = false;
  let historyList: HTMLDivElement;
  let query = $derived(searchQuery.trim().toLocaleLowerCase());
  // Entries are never changed in place, so each one is lower-cased once rather than on every keystroke.
  const searchText = new WeakMap<Transcription, string>();
  function searchable(item: Transcription) {
    let text = searchText.get(item);
    if (text === undefined) {
      // A search field value cannot contain a line break, so a query never spans both fields.
      text = `${item.text}\n${item.raw_text ?? ''}`.toLocaleLowerCase();
      searchText.set(item, text);
    }
    return text;
  }
  let filteredItems = $derived(transcriptionStore.items.filter((item) =>
    (!query || searchable(item).includes(query)) &&
    (outcomeFilter === 'all' || (outcomeFilter === 'cleaned' ? item.llm_applied : !item.cancelled && cleanupOutcome(item.llm_cleanup_status).issue))));
  function dayKey(value: string) { return new Date(value).toLocaleDateString(); }
  function dayLabel(value: string) {
    const day = new Date(value);
    const today = new Date(minuteNow());
    const yesterday = new Date(today); yesterday.setDate(today.getDate() - 1);
    if (dayKey(value) === today.toLocaleDateString()) return 'Today';
    if (dayKey(value) === yesterday.toLocaleDateString()) return 'Yesterday';
    return day.toLocaleDateString(undefined, { weekday:'long', month:'short', day:'numeric', year:'numeric' });
  }
  let pageCount = $derived(Math.max(1, Math.ceil(filteredItems.length / PAGE_SIZE)));
  let currentPage = $derived(Math.min(page, pageCount - 1));
  let visibleItems = $derived(filteredItems.slice(currentPage * PAGE_SIZE, (currentPage + 1) * PAGE_SIZE));

  $effect(() => { query; outcomeFilter; currentPage; if (historyList) historyList.scrollTop = 0; });

  async function handleCopy(text: string) {
    error = '';
    try { await writeText(text); }
    catch (err) { error = `Could not copy: ${String(err)}`; throw err; }
  }

  async function confirmDelete() {
    if (!deleteTarget || deleting) return;
    deleting = true;
    error = '';
    try {
      if (deleteTarget === 'all') await transcriptionStore.clear();
      else await transcriptionStore.delete(deleteTarget);
      deleteTarget = null;
    } catch (err) { error = `Could not delete history: ${String(err)}`; deleteTarget = null; }
    finally { deleting = false; }
  }

  async function handleExport() {
    if (exporting) return;
    exporting = true;
    error = '';
    try {
      const path = await exportTranscriptionsCsvFile();
      if (disposed) return;
      exportedPath = path;
    } catch (err) { if (!disposed) error = `Could not export history: ${String(err)}`; }
    finally { if (!disposed) exporting = false; }
  }

  async function showExport(path: string) {
    error = '';
    try { await revealCsvExport(path); }
    catch (err) { if (!disposed) error = `Finder did not open: ${String(err)}`; }
  }

  onMount(() => {
    const scope = createEventScope((err) => { error = `Live history unavailable: ${String(err)}`; });
    void Promise.all([
      scope.listen<TranscriptionEvent>('transcription-complete', (event) => transcriptionStore.add(event.payload)),
      scope.listen<string>('history-error', (event) => { error = event.payload; }),
      // Retention removed entries in the background; they are already gone from disk.
      scope.listen<{ removed_ids: string[] }>('history-changed', (event) => transcriptionStore.remove(event.payload.removed_ids)),
    ]).then(() => { if (!disposed) void transcriptionStore.load(); });
    return () => { disposed = true; scope.dispose(); transcriptionStore.invalidateLoad(); };
  });
</script>

<div class="history-window">
  <header class="history-header">
    <div class="history-title"><h1>History</h1><span>{transcriptionStore.items.length} saved narrations</span></div>
    <div class="header-actions">
      <div class="search-wrapper"><input class="search-input" type="search" placeholder="Search all history…" aria-label="Search transcriptions" value={searchQuery} oninput={(event) => { searchQuery = event.currentTarget.value; page = 0; }} /></div>
      <button class="export-btn" type="button" onclick={handleExport} disabled={!transcriptionStore.loaded || !transcriptionStore.items.length || exporting}>{exporting ? 'Exporting…' : 'Export CSV'}</button>
      <button class="clear-all-btn" type="button" onclick={() => { deleteTarget = 'all'; }} disabled={!transcriptionStore.loaded || !transcriptionStore.items.length || deleting}>Clear All</button>
    </div>
    <nav class="history-filters" aria-label="Filter cleanup outcomes">
      {#each [{id:'all',label:'All'}, {id:'cleaned',label:'AI cleaned'}, {id:'issues',label:'Cleanup issues'}] as filter}
        <button type="button" aria-pressed={outcomeFilter === filter.id} onclick={() => { outcomeFilter = filter.id as typeof outcomeFilter; page = 0; }}>{filter.label}</button>
      {/each}
      {#if query || outcomeFilter !== 'all'}<button class="reset-filter" type="button" onclick={() => { searchQuery = ''; outcomeFilter = 'all'; page = 0; }}>Clear filters</button>{/if}
    </nav>
  </header>
  {#if error || transcriptionStore.error}
    <div class="history-error" role="alert">{error || transcriptionStore.error}
      {#if transcriptionStore.error}<button type="button" onclick={() => transcriptionStore.load()}>Retry</button>{:else}<button type="button" onclick={() => { error = ''; }}>Dismiss</button>{/if}
    </div>
  {/if}
  {#if exportedPath}<p class="history-feedback" role="status">CSV saved: {exportedPath}<button type="button" onclick={() => showExport(exportedPath!)}>Show in Finder</button></p>{/if}
  <div class="history-list" bind:this={historyList}>
    <RecordingRecovery />
    {#if transcriptionStore.loading}<div class="empty-state" role="status"><p>Loading history…</p></div>
    {:else if !transcriptionStore.loaded}<div class="empty-state"><p class="empty-title">History is unavailable</p><p class="empty-subtitle">Your saved entries have not been changed.</p></div>
    {:else if !transcriptionStore.items.length}<div class="empty-state"><p class="empty-title">No transcriptions yet</p><p class="empty-subtitle">Press your hotkey to start recording. Transcriptions will appear here.</p></div>
    {:else if !filteredItems.length}<div class="empty-state"><p class="empty-title">No results</p><p class="empty-subtitle">Try another search or cleanup filter.</p></div>
    {:else}
      {#each visibleItems as item, index (item.id)}
        {#if index === 0 || dayKey(item.created_at) !== dayKey(visibleItems[index - 1].created_at)}<h2 class="date-group">{dayLabel(item.created_at)}</h2>{/if}
        <HistoryItem {item} expanded={rowViews[item.id]?.expanded ?? false} viewMode={rowViews[item.id]?.viewMode ?? 'cleaned'} onviewchange={(state) => { rowViews[item.id] = state; }} ondelete={(id) => { deleteTarget = id; }} oncopy={handleCopy} />{/each}
    {/if}
  </div>
  {#if transcriptionStore.loaded && filteredItems.length}
    <nav class="history-pagination" aria-label="History pages">
      <span>{currentPage * PAGE_SIZE + 1}–{Math.min((currentPage + 1) * PAGE_SIZE, filteredItems.length)} of {filteredItems.length}{query || outcomeFilter !== 'all' ? ' matches' : ' entries'}</span>
      <button type="button" disabled={currentPage === 0} onclick={() => { page = currentPage - 1; }}>Previous</button>
      <button type="button" disabled={currentPage + 1 >= pageCount} onclick={() => { page = currentPage + 1; }}>Next</button>
    </nav>
  {/if}
</div>
<ConfirmDialog open={deleteTarget !== null} title={deleteTarget === 'all' ? 'Clear all history?' : 'Delete this transcription?'} message={deleteTarget === 'all' ? 'This permanently removes every saved transcription and the audio kept with it, including entries hidden by your search. Export a CSV first if you want a copy of the text.' : 'This permanently removes the selected transcription from history and deletes the audio kept with it.'} confirmLabel={deleteTarget === 'all' ? 'Clear all history' : 'Delete'} busy={deleting} onconfirm={confirmDelete} oncancel={() => { deleteTarget = null; }} />

<style>
  .history-title { display:flex; align-items:baseline; gap:12px; margin-bottom:14px; }
  .history-title span { color:var(--text-dim); font-size:12px; }
  .history-filters { display:flex; gap:6px; margin-top:12px; flex-wrap:wrap; }
  .history-filters button { border:1px solid var(--border); border-radius:7px; background:transparent; color:var(--text-dim); padding:6px 10px; font:inherit; font-size:11px; cursor:pointer; }
  .history-filters button[aria-pressed="true"] { background:var(--accent-bg); color:var(--accent-text); border-color:var(--accent); }
  .history-filters .reset-filter { margin-left:auto; border-color:transparent; }
  .date-group { margin:12px 0 2px; color:var(--text-dim); font-size:11px; font-weight:600; letter-spacing:.3px; }

  .history-window {
    display: flex;
    flex-direction: column;
    height: 100vh;
    overflow: hidden;
  }

  .history-header {
    flex-shrink: 0;
    padding: 20px 20px 16px;
    border-bottom: 1px solid var(--border);
  }

  h1 {
    font-size: 22px;
    font-weight: 600;
    margin: 0;
    color: var(--text-bright);
    letter-spacing: -0.3px;
  }

  .header-actions {
    display: flex;
    gap: 10px;
    align-items: center;
  }

  .search-wrapper {
    flex: 1;
    position: relative;
    display: flex;
    align-items: center;
  }

  .search-input {
    width: 100%;
    padding: 8px 12px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--input-bg);
    color: var(--text);
    font-size: 13px;
    font-family: inherit;
    outline: none;
    transition: border-color 0.15s ease;
    box-sizing: border-box;
  }

  .search-input::placeholder {
    color: var(--text-dim);
  }

  .search-input:focus {
    border-color: var(--accent);
  }

  .export-btn {
    flex-shrink: 0;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: none;
    color: var(--text-dim);
    font-size: 13px;
    font-family: inherit;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .export-btn:hover:not(:disabled) {
    color: var(--accent-text);
    border-color: var(--accent);
    background: var(--accent-bg);
  }

  .export-btn:disabled {
    opacity: 0.35;
    cursor: default;
  }

  .clear-all-btn {
    flex-shrink: 0;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: none;
    color: var(--text-dim);
    font-size: 13px;
    font-family: inherit;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .clear-all-btn:hover:not(:disabled) {
    color: var(--danger);
    border-color: var(--danger);
    background: var(--danger-bg);
  }

  .clear-all-btn:disabled {
    opacity: 0.35;
    cursor: default;
  }

  .history-list {
    flex: 1;
    overflow-y: auto;
    padding: 12px 20px 20px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    text-align: center;
    padding: 48px 20px;
    flex: 1;
  }

  .empty-title {
    font-size: 15px;
    font-weight: 500;
    color: var(--text);
    margin: 0 0 6px;
  }

  .empty-subtitle {
    font-size: 13px;
    color: var(--text-dim);
    margin: 0;
    max-width: 260px;
    line-height: 1.5;
  }
  .history-pagination { display:flex; align-items:center; gap:8px; padding:12px 20px; border-top:1px solid var(--border); font-size:12px; flex:none; }
  .history-pagination span { flex:1; color:var(--text-dim); }
  .history-pagination button, .history-error button, .history-feedback button { padding:6px 9px; background:var(--card-bg); color:var(--text); border:1px solid var(--border-hover); border-radius:6px; font:inherit; font-size:12px; cursor:pointer; }
  .history-pagination button:disabled { opacity:.4; cursor:default; }
  .history-error { margin:12px 20px 0; padding:10px; border-radius:8px; background:var(--danger-bg); color:var(--danger); font-size:12px; overflow-wrap:anywhere; }
  .history-error button, .history-feedback button { margin-left:8px; }
  .history-feedback { overflow-wrap:anywhere; color:var(--text-dim); font-size:12px; margin:10px 20px 0; }
</style>
