<script lang="ts">
  import type { DictionaryEntry } from '../utils/tauri';

  interface Props {
    entries: DictionaryEntry[];
    onchange: (entries: DictionaryEntry[]) => void;
  }

  let { entries, onchange }: Props = $props();

  function updateEntry(index: number, field: keyof DictionaryEntry, value: string) {
    onchange(entries.map((entry, row) => row === index ? { ...entry, [field]: value } : entry));
  }
</script>

<div class="dictionary-editor">
  <p class="hint">
    Correct words SottoASR mishears, even with AI cleanup off. For example, replace
    <strong>Quen</strong> with <strong>Qwen</strong>. Only the aliases you add are changed.
  </p>
  {#if entries.length === 0}
    <p class="empty">No exact replacements yet.</p>
  {/if}
  {#each entries as entry, index}
    <div class="dictionary-row">
      <label>
        <span>Heard</span>
        <input
          aria-label={`Heard alias ${index + 1}`}
          value={entry.heard}
          placeholder="Quen"
          maxlength="120"
          oninput={(event) => updateEntry(index, 'heard', event.currentTarget.value)}
        />
      </label>
      <label>
        <span>Write instead</span>
        <input
          aria-label={`Replacement ${index + 1}`}
          value={entry.replacement}
          placeholder="Qwen"
          maxlength="120"
          oninput={(event) => updateEntry(index, 'replacement', event.currentTarget.value)}
        />
      </label>
      <button
        class="remove"
        aria-label={`Remove dictionary row ${index + 1}`}
        onclick={() => onchange(entries.filter((_, row) => row !== index))}
        type="button"
      >Remove</button>
    </div>
  {/each}
  <button
    class="add"
    disabled={entries.length >= 200}
    onclick={() => onchange([...entries, { heard: '', replacement: '' }])}
    type="button"
  >Add replacement</button>
  <p class="hint footnote">
    Save to apply. Matches whole words or exact phrases, ignoring English letter case.
    For a name with a version number, add its full spelling. URLs, email addresses, and
    backtick code are left alone.
  </p>
</div>

<style>
  .hint, .empty {
    color: var(--text-dim);
    font-size: 12px;
    line-height: 1.5;
    margin: 0 0 12px;
  }

  .hint strong { color: var(--text); font-weight: 500; }
  .empty { font-style: italic; }
  .footnote { margin: 10px 0 0; }

  .dictionary-row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1fr) auto;
    gap: 8px;
    align-items: end;
    margin-bottom: 10px;
  }

  label { min-width: 0; }
  label span { display: block; font-size: 12px; margin-bottom: 5px; }
  input {
    width: 100%;
    box-sizing: border-box;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--input-bg);
    color: var(--text-bright);
    font: inherit;
    font-size: 13px;
  }

  input:focus { outline: 1px solid var(--accent); }
  button {
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--card-bg);
    color: var(--text);
    font: inherit;
    font-size: 12px;
    cursor: pointer;
  }

  button:hover { border-color: var(--accent); }
  button:disabled { opacity: 0.5; cursor: default; }
</style>
