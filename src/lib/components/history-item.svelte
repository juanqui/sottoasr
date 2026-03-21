<script lang="ts">
  import { formatRelativeTime, formatDuration, truncateText } from '../utils/format';

  import type { Transcription } from '../utils/tauri';

  interface Props {
    item: Transcription;
    ondelete: (id: string) => void;
    oncopy: (text: string) => void;
  }

  let { item, ondelete, oncopy }: Props = $props();

  let expanded: boolean = $state(false);
  let copyFeedback: boolean = $state(false);

  let relativeTime = $derived(formatRelativeTime(item.created_at));
  let durationText = $derived(formatDuration(item.duration_ms));
  let previewText = $derived(truncateText(item.text, 100));

  function toggleExpand() {
    expanded = !expanded;
  }

  function handleCopy() {
    oncopy(item.text);
    copyFeedback = true;
    setTimeout(() => {
      copyFeedback = false;
    }, 1500);
  }

  function handleDelete() {
    ondelete(item.id);
  }
</script>

<div class="history-item" class:expanded>
  <button class="item-body" onclick={toggleExpand} type="button">
    <p class="text-preview">
      {expanded ? item.text : previewText}
    </p>
  </button>

  <div class="item-meta">
    <span class="timestamp" title={item.created_at}>{relativeTime}</span>
    <span class="separator">&middot;</span>
    <span class="duration">{durationText}</span>
    <span class="separator">&middot;</span>
    <span class="word-count">{item.word_count} {item.word_count === 1 ? 'word' : 'words'}</span>

    <div class="actions">
      <button
        class="action-btn copy-btn"
        class:copied={copyFeedback}
        onclick={handleCopy}
        title="Copy to clipboard"
        type="button"
      >
        {copyFeedback ? 'Copied' : 'Copy'}
      </button>
      <button
        class="action-btn delete-btn"
        onclick={handleDelete}
        title="Delete"
        type="button"
      >
        Delete
      </button>
    </div>
  </div>
</div>

<style>
  .history-item {
    background: var(--card-bg);
    border: 1px solid var(--border);
    border-radius: 10px;
    overflow: hidden;
    transition: border-color 0.15s ease;
  }

  .history-item:hover {
    border-color: var(--border-hover);
  }

  .item-body {
    display: block;
    width: 100%;
    padding: 14px 16px 8px;
    background: none;
    border: none;
    color: inherit;
    font: inherit;
    cursor: pointer;
    text-align: left;
  }

  .text-preview {
    font-size: 14px;
    line-height: 1.5;
    color: var(--text);
    margin: 0;
    word-break: break-word;
    white-space: pre-wrap;
  }

  .expanded .text-preview {
    color: var(--text-bright);
  }

  .item-meta {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 16px 12px;
    font-size: 12px;
    color: var(--text-dim);
    flex-wrap: wrap;
  }

  .separator {
    opacity: 0.4;
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
    font-size: 12px;
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

  .copy-btn.copied {
    color: var(--accent);
    border-color: var(--accent);
  }

  .delete-btn:hover {
    color: #ef4444;
    border-color: #ef4444;
  }
</style>
