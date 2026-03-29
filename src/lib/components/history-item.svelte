<script lang="ts">
  import { onDestroy } from 'svelte';
  import { diffWords } from 'diff';
  import { formatRelativeTime, formatDuration, truncateText } from '../utils/format';
  import type { Transcription } from '../utils/tauri';

  interface Props {
    item: Transcription;
    ondelete: (id: string) => void;
    oncopy: (text: string) => void;
  }

  let { item, ondelete, oncopy }: Props = $props();

  type ViewMode = 'cleaned' | 'raw' | 'diff';

  let expanded: boolean = $state(false);
  let copyFeedback: boolean = $state(false);
  let viewMode: ViewMode = $state('cleaned');

  let relativeTime = $derived(formatRelativeTime(item.created_at));
  let durationText = $derived(formatDuration(item.duration_ms));

  let displayText = $derived(
    viewMode === 'raw' && item.raw_text ? item.raw_text : item.text
  );
  let previewText = $derived(truncateText(displayText, 120));

  // Compute word-level diff parts using the `diff` library
  let diffParts = $derived.by(() => {
    if (!item.raw_text || !item.llm_applied) return [];
    return diffWords(item.raw_text, item.text);
  });

  let hasLlm = $derived(item.llm_applied && !!item.raw_text);

  let copyTimeoutId: ReturnType<typeof setTimeout> | null = null;

  function setView(mode: ViewMode) {
    if (viewMode === mode) {
      viewMode = 'cleaned';
    } else {
      viewMode = mode;
    }
  }

  function toggleExpand() {
    expanded = !expanded;
  }

  function handleCopy() {
    oncopy(displayText);
    copyFeedback = true;
    // Clear any existing timeout to avoid stale callbacks
    if (copyTimeoutId) clearTimeout(copyTimeoutId);
    copyTimeoutId = setTimeout(() => {
      copyFeedback = false;
      copyTimeoutId = null;
    }, 1500);
  }

  function handleDelete() {
    ondelete(item.id);
  }

  // Clean up timeout on component destroy to prevent memory leaks
  onDestroy(() => {
    if (copyTimeoutId) {
      clearTimeout(copyTimeoutId);
      copyTimeoutId = null;
    }
  });
</script>

<div class="history-item" class:expanded>
  <!-- Main text area -->
  <button class="item-body" onclick={toggleExpand} type="button" aria-expanded={expanded}>
    {#if item.cancelled}
      <span class="cancelled-badge">Cancelled</span>
    {:else if hasLlm}
      <span class="llm-badge">AI Cleaned</span>
    {/if}

    {#if !item.text && item.cancelled}
      <p class="text-preview empty">No transcription (recording was cancelled)</p>
    {:else if viewMode === 'diff' && expanded && hasLlm}
      <div class="diff-inline">
        {#each diffParts as part}
          {#if part.added}
            <span class="diff-added">{part.value}</span>
          {:else if part.removed}
            <span class="diff-removed">{part.value}</span>
          {:else}
            <span>{part.value}</span>
          {/if}
        {/each}
      </div>
    {:else}
      <p class="text-preview">
        {expanded ? displayText : previewText}
      </p>
    {/if}
  </button>

  <!-- Metadata row -->
  <div class="item-meta">
    <span class="timestamp" title={item.created_at}>{relativeTime}</span>
    <span class="sep">&middot;</span>
    <span class="duration">{durationText}</span>
    <span class="sep">&middot;</span>
    <span class="words">{item.word_count} {item.word_count === 1 ? 'word' : 'words'}</span>

    <div class="actions">
      {#if hasLlm}
        <button
          class="action-btn"
          class:active={viewMode === 'raw'}
          onclick={() => setView('raw')}
          type="button"
        >Raw</button>
        <button
          class="action-btn"
          class:active={viewMode === 'diff'}
          onclick={() => { if (!expanded) expanded = true; setView('diff'); }}
          type="button"
        >Diff</button>
      {/if}
      <button
        class="action-btn"
        class:copied={copyFeedback}
        onclick={handleCopy}
        type="button"
      >{copyFeedback ? 'Copied' : 'Copy'}</button>
      <button
        class="action-btn delete"
        onclick={handleDelete}
        type="button"
      >Delete</button>
    </div>
  </div>

  <!-- Diff legend (only when diff view active) -->
  {#if viewMode === 'diff' && expanded && hasLlm}
    <div class="diff-legend">
      <span class="legend-item"><span class="legend-swatch removed"></span> Removed</span>
      <span class="legend-item"><span class="legend-swatch added"></span> Added</span>
    </div>
  {/if}
</div>

<style>
  .history-item {
    background: var(--card-bg);
    border: 1px solid var(--border);
    border-radius: 10px;
    overflow: hidden;
    transition: border-color 0.15s ease;
    flex-shrink: 0;
  }

  .history-item:hover {
    border-color: var(--border-hover);
  }

  /* Body */
  .item-body {
    display: block;
    width: 100%;
    padding: 12px 16px 8px;
    background: none;
    border: none;
    color: inherit;
    font: inherit;
    cursor: pointer;
    text-align: left;
  }

  .cancelled-badge {
    display: inline-block;
    font-size: 9px;
    font-weight: 600;
    padding: 2px 7px;
    border-radius: 10px;
    background: rgba(239, 68, 68, 0.12);
    color: rgba(248, 113, 113, 0.9);
    letter-spacing: 0.4px;
    text-transform: uppercase;
    margin-bottom: 6px;
  }

  .text-preview.empty {
    color: var(--text-dim);
    font-style: italic;
  }

  .llm-badge {
    display: inline-block;
    font-size: 9px;
    font-weight: 600;
    padding: 2px 7px;
    border-radius: 10px;
    background: rgba(99, 102, 241, 0.15);
    color: rgb(129, 140, 248);
    letter-spacing: 0.4px;
    text-transform: uppercase;
    margin-bottom: 6px;
  }

  .text-preview {
    font-size: 14px;
    line-height: 1.55;
    color: var(--text);
    margin: 0;
    word-break: break-word;
    white-space: pre-wrap;
  }

  .expanded .text-preview {
    color: var(--text-bright);
  }

  /* ---- Inline diff ---- */
  .diff-inline {
    font-size: 14px;
    line-height: 1.6;
    color: var(--text-bright);
    word-break: break-word;
    white-space: pre-wrap;
  }

  .diff-removed {
    background: rgba(239, 68, 68, 0.18);
    color: rgb(252, 165, 165);
    text-decoration: line-through;
    text-decoration-color: rgba(252, 165, 165, 0.5);
    border-radius: 2px;
    padding: 0 1px;
  }

  .diff-added {
    background: rgba(34, 197, 94, 0.18);
    color: rgb(134, 239, 172);
    border-radius: 2px;
    padding: 0 1px;
  }

  .diff-legend {
    display: flex;
    gap: 14px;
    padding: 0 16px 10px;
    font-size: 10px;
    color: var(--text-dim);
  }

  .legend-item {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .legend-swatch {
    display: inline-block;
    width: 10px;
    height: 10px;
    border-radius: 2px;
  }

  .legend-swatch.removed {
    background: rgba(239, 68, 68, 0.25);
    border: 1px solid rgba(252, 165, 165, 0.4);
  }

  .legend-swatch.added {
    background: rgba(34, 197, 94, 0.25);
    border: 1px solid rgba(134, 239, 172, 0.4);
  }

  /* ---- Meta row ---- */
  .item-meta {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 16px 10px;
    font-size: 12px;
    color: var(--text-dim);
    flex-wrap: wrap;
  }

  .sep {
    opacity: 0.35;
  }

  .actions {
    display: flex;
    gap: 4px;
    margin-left: auto;
  }

  .action-btn {
    background: none;
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 3px 10px;
    font-size: 11px;
    cursor: pointer;
    transition: all 0.15s ease;
    color: var(--text-dim);
    font-family: inherit;
  }

  .action-btn:hover {
    background: var(--hover-bg);
    color: var(--text);
    border-color: var(--border-hover);
  }

  .action-btn.active {
    color: rgb(129, 140, 248);
    border-color: rgba(129, 140, 248, 0.5);
    background: rgba(99, 102, 241, 0.08);
  }

  .action-btn.copied {
    color: var(--accent);
    border-color: var(--accent);
  }

  .action-btn.delete:hover {
    color: #ef4444;
    border-color: #ef4444;
  }
</style>
