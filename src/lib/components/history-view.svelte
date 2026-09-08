<script lang="ts">
  import { onMount } from 'svelte';
  import { writeText } from '@tauri-apps/plugin-clipboard-manager';
  import { transcriptionStore } from '../stores/transcriptions.svelte';
  import { exportTranscriptionsCsvFile } from '../utils/tauri';
  import { createEventScope } from '../utils/event-scope';
  import HistoryItem from './history-item.svelte';
  import ConfirmDialog from './confirm-dialog.svelte';
  import type { TranscriptionEvent } from '../utils/tauri';

  const PAGE_SIZE = 50;
  let searchQuery = $state('');
  let page = $state(0);
  let rowViews = $state<Record<string, { expanded: boolean; viewMode: 'cleaned' | 'raw' | 'diff' }>>({});
  let error = $state('');
  let feedback = $state('');
  let deleting = $state(false);
  let exporting = $state(false);
  let deleteTarget = $state<string | null>(null);
  let disposed = false;
  let historyList: HTMLDivElement;
  let query = $derived(searchQuery.trim().toLocaleLowerCase());
  let filteredItems = $derived(query ? transcriptionStore.items.filter((item) =>
    item.text.toLocaleLowerCase().includes(query) || (item.raw_text?.toLocaleLowerCase().includes(query) ?? false)) : transcriptionStore.items);
  let pageCount = $derived(Math.max(1, Math.ceil(filteredItems.length / PAGE_SIZE)));
  let currentPage = $derived(Math.min(page, pageCount - 1));
  let visibleItems = $derived(filteredItems.slice(currentPage * PAGE_SIZE, (currentPage + 1) * PAGE_SIZE));

  $effect(() => { query; currentPage; if (historyList) historyList.scrollTop = 0; });

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
      feedback = `CSV saved: ${path}`;
    } catch (err) { if (!disposed) error = `Could not export history: ${String(err)}`; }
    finally { if (!disposed) exporting = false; }
  }

  onMount(() => {
    const scope = createEventScope((err) => { error = `Live history unavailable: ${String(err)}`; });
    void Promise.all([
      scope.listen<TranscriptionEvent>('transcription-complete', (event) => transcriptionStore.add(event.payload)),
      scope.listen<string>('history-error', (event) => { error = event.payload; }),
    ]).then(() => { if (!disposed) void transcriptionStore.load(); });
    return () => { disposed = true; scope.dispose(); transcriptionStore.invalidateLoad(); };
  });
</script>

<div class="history-window">
  <header class="history-header">
    <h1>History</h1>
    <div class="header-actions">
      <div class="search-wrapper"><input class="search-input" type="search" placeholder="Search all history…" aria-label="Search transcriptions" value={searchQuery} oninput={(event) => { searchQuery = event.currentTarget.value; page = 0; }} /></div>
      <button class="export-btn" type="button" onclick={handleExport} disabled={!transcriptionStore.loaded || !transcriptionStore.items.length || exporting}>{exporting ? 'Exporting…' : 'Export CSV'}</button>
      <button class="clear-all-btn" type="button" onclick={() => { deleteTarget = 'all'; }} disabled={!transcriptionStore.loaded || !transcriptionStore.items.length || deleting}>Clear All</button>
    </div>
  </header>
  {#if error || transcriptionStore.error}
    <div class="history-error" role="alert">{error || transcriptionStore.error}
      {#if transcriptionStore.error}<button type="button" onclick={() => transcriptionStore.load()}>Retry</button>{:else}<button type="button" onclick={() => { error = ''; }}>Dismiss</button>{/if}
    </div>
  {/if}
  {#if feedback}<p class="history-feedback" role="status">{feedback}</p>{/if}
  <div class="history-list" bind:this={historyList}>
    {#if transcriptionStore.loading}<div class="empty-state" role="status"><p>Loading history…</p></div>
    {:else if !transcriptionStore.loaded}<div class="empty-state"><p class="empty-title">History is unavailable</p><p class="empty-subtitle">Your saved entries have not been changed.</p></div>
    {:else if !transcriptionStore.items.length}<div class="empty-state"><p class="empty-title">No transcriptions yet</p><p class="empty-subtitle">Press your hotkey to start recording. Transcriptions will appear here.</p></div>
    {:else if !filteredItems.length}<div class="empty-state"><p class="empty-title">No results</p><p class="empty-subtitle">No transcriptions match “{searchQuery}”</p></div>
    {:else}
      {#each visibleItems as item (item.id)}<HistoryItem {item} expanded={rowViews[item.id]?.expanded ?? false} viewMode={rowViews[item.id]?.viewMode ?? 'cleaned'} onviewchange={(state) => { rowViews[item.id] = state; }} ondelete={(id) => { deleteTarget = id; }} oncopy={handleCopy} />{/each}
    {/if}
  </div>
  {#if transcriptionStore.loaded && filteredItems.length}
    <nav class="history-pagination" aria-label="History pages">
      <span>{currentPage * PAGE_SIZE + 1}–{Math.min((currentPage + 1) * PAGE_SIZE, filteredItems.length)} of {filteredItems.length}{query ? ' matches' : ' entries'}</span>
      <button type="button" disabled={currentPage === 0} onclick={() => { page = currentPage - 1; }}>Previous</button>
      <button type="button" disabled={currentPage + 1 >= pageCount} onclick={() => { page = currentPage + 1; }}>Next</button>
    </nav>
  {/if}
</div>
<ConfirmDialog open={deleteTarget !== null} title={deleteTarget === 'all' ? 'Clear all history?' : 'Delete this transcription?'} message={deleteTarget === 'all' ? 'This permanently removes every saved transcription, including entries hidden by your search. Export a CSV first if you want a copy.' : 'This permanently removes the selected transcription from history.'} confirmLabel={deleteTarget === 'all' ? 'Clear all history' : 'Delete'} busy={deleting} onconfirm={confirmDelete} oncancel={() => { deleteTarget = null; }} />

<style>
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
    margin: 0 0 14px;
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
    color: var(--accent);
    border-color: var(--accent);
    background: rgba(99, 102, 241, 0.08);
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
    color: #ef4444;
    border-color: #ef4444;
    background: rgba(239, 68, 68, 0.08);
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
  .history-pagination button, .history-error button { padding:6px 9px; background:var(--card-bg); color:var(--text); border:1px solid var(--border-hover); border-radius:6px; font:inherit; font-size:12px; cursor:pointer; }
  .history-pagination button:disabled { opacity:.4; cursor:default; }
  .history-error { margin:12px 20px 0; padding:10px; border-radius:8px; background:#ef444410; color:#fca5a5; font-size:12px; overflow-wrap:anywhere; }
  .history-error button { margin-left:8px; }
  .history-feedback { overflow-wrap:anywhere; color:var(--text-dim); font-size:12px; margin:10px 20px 0; }
</style>
