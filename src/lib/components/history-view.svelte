<script lang="ts">
  import { listen } from '@tauri-apps/api/event';
  import { writeText } from '@tauri-apps/plugin-clipboard-manager';

  import { transcriptionStore } from '../stores/transcriptions.svelte';
  import { exportTranscriptionsCsv } from '../utils/tauri';
  import HistoryItem from './history-item.svelte';

  import type { Transcription } from '../utils/tauri';

  let searchQuery: string = $state('');

  let filteredItems = $derived(
    searchQuery.trim()
      ? transcriptionStore.items.filter((item) => {
          const q = searchQuery.toLowerCase();
          return item.text.toLowerCase().includes(q)
            || (item.raw_text?.toLowerCase().includes(q) ?? false);
        })
      : transcriptionStore.items
  );

  let isEmpty = $derived(transcriptionStore.items.length === 0);

  async function handleCopy(text: string) {
    try {
      await writeText(text);
    } catch (err) {
      console.error('Failed to copy text:', err);
    }
  }

  async function handleDelete(id: string) {
    await transcriptionStore.delete(id);
  }

  async function handleClearAll() {
    if (transcriptionStore.items.length === 0) return;
    await transcriptionStore.clear();
  }

  let exportFeedback = $state('');

  async function handleExport() {
    try {
      const csv = await exportTranscriptionsCsv();
      // Create a blob and trigger download via data URI
      const blob = new Blob([csv], { type: 'text/csv' });
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = `sottoasr-transcriptions-${new Date().toISOString().slice(0, 10)}.csv`;
      a.click();
      URL.revokeObjectURL(url);
      exportFeedback = 'Exported!';
      setTimeout(() => { exportFeedback = ''; }, 2000);
    } catch (err) {
      console.error('Export failed:', err);
      // Fallback: copy CSV to clipboard
      try {
        const csv = await exportTranscriptionsCsv();
        await writeText(csv);
        exportFeedback = 'Copied to clipboard';
        setTimeout(() => { exportFeedback = ''; }, 2000);
      } catch {
        exportFeedback = 'Export failed';
        setTimeout(() => { exportFeedback = ''; }, 2000);
      }
    }
  }

  // Load transcriptions and listen for new ones
  $effect(() => {
    transcriptionStore.load();

    const unlisteners: Array<() => void> = [];

    listen<Transcription>('transcription-complete', (event) => {
      transcriptionStore.add(event.payload);
    }).then((unlisten) => unlisteners.push(unlisten));

    return () => {
      unlisteners.forEach((fn) => fn());
    };
  });
</script>

<div class="history-window">
  <header class="history-header">
    <h1>History</h1>
    <div class="header-actions">
      <div class="search-wrapper">
        <svg class="search-icon" viewBox="0 0 16 16" fill="none" aria-hidden="true">
          <circle cx="7" cy="7" r="5.5" stroke="currentColor" stroke-width="1.5" />
          <path d="M11 11l3.5 3.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
        </svg>
        <input
          type="text"
          class="search-input"
          placeholder="Search transcriptions..."
          aria-label="Search transcriptions"
          bind:value={searchQuery}
        />
      </div>
      <button
        class="export-btn"
        onclick={handleExport}
        disabled={isEmpty}
        type="button"
      >
        {exportFeedback || 'Export CSV'}
      </button>
      <button
        class="clear-all-btn"
        onclick={handleClearAll}
        disabled={isEmpty}
        type="button"
      >
        Clear All
      </button>
    </div>
  </header>

  <div class="history-list">
    {#if isEmpty}
      <div class="empty-state">
        <div class="empty-icon" aria-hidden="true">
          <svg viewBox="0 0 48 48" fill="none">
            <rect x="8" y="12" width="32" height="28" rx="4" stroke="currentColor" stroke-width="2" />
            <path d="M16 22h16M16 28h10" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
            <path d="M20 8v8M28 8v8" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
          </svg>
        </div>
        <p class="empty-title">No transcriptions yet</p>
        <p class="empty-subtitle">
          Press your hotkey to start recording. Transcriptions will appear here.
        </p>
      </div>
    {:else if filteredItems.length === 0}
      <div class="empty-state">
        <p class="empty-title">No results</p>
        <p class="empty-subtitle">
          No transcriptions match "{searchQuery}"
        </p>
      </div>
    {:else}
      {#each filteredItems as item (item.id)}
        <HistoryItem
          {item}
          ondelete={handleDelete}
          oncopy={handleCopy}
        />
      {/each}
    {/if}
  </div>
</div>

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

  .search-icon {
    position: absolute;
    left: 10px;
    width: 14px;
    height: 14px;
    color: var(--text-dim);
    pointer-events: none;
  }

  .search-input {
    width: 100%;
    padding: 8px 12px 8px 32px;
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
    padding: 8px 14px;
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
    padding: 8px 14px;
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

  .empty-icon {
    width: 48px;
    height: 48px;
    color: var(--text-dim);
    opacity: 0.4;
    margin-bottom: 16px;
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
</style>
