<script lang="ts">
  import { settingsStore } from '../stores/settings.svelte';
  import DictionarySettings from './dictionary-settings.svelte';
  import type { VocabularySetup } from '../stores/vocabulary-setup.svelte';
  let { vocabulary }: { vocabulary: VocabularySetup } = $props();
  let newTerm = $state('');
  let inputError = $state('');
  let termInput: HTMLInputElement;

  function addTerm() {
    const term = newTerm.trim();
    inputError = '';
    if (!term) return;
    if (settingsStore.current.vocabulary.some((entry) => entry.toLocaleLowerCase() === term.toLocaleLowerCase())) {
      inputError = 'That word is already in your vocabulary.';
      return;
    }
    if (settingsStore.current.vocabulary.length >= 100) {
      inputError = 'You can save up to 100 vocabulary words.';
      return;
    }
    settingsStore.update('vocabulary', [...settingsStore.current.vocabulary, term]);
    newTerm = '';
    termInput?.focus();
  }
</script>

<section class="setting-card">
  <h3>Words you use</h3>
  <p class="setting-hint">Add names and technical words in their correct spelling. SottoASR uses the recording to help recognize them, with AI cleanup off or on.</p>
  <p class="setting-hint">For a model name, add <strong>Qwen</strong>. Version numbers and phrases with punctuation currently need an exact replacement below.</p>
  <form class="term-form" onsubmit={(event) => { event.preventDefault(); addTerm(); }}>
    <label class="term-input">
      <span class="field-label">Correct spelling</span>
      <input bind:this={termInput} bind:value={newTerm} maxlength="120" placeholder="Qwen, Kubernetes, Juanqui…" aria-describedby="term-help" />
    </label>
    <button class="secondary-button" type="submit" disabled={!newTerm.trim() || settingsStore.current.vocabulary.length >= 100}>Add</button>
  </form>
  <p class="setting-hint" id="term-help">{settingsStore.current.vocabulary.length} of 100 words · Save to apply</p>
  {#if inputError}<p class="setting-error" role="alert">{inputError}</p>{/if}
  {#if settingsStore.current.vocabulary.length}
    <ul class="term-list" aria-label="Vocabulary words">
      {#each settingsStore.current.vocabulary as term, index}
        <li><span>{term}</span><button type="button" aria-label={`Remove ${term}`} onclick={() => settingsStore.update('vocabulary', settingsStore.current.vocabulary.filter((_, row) => row !== index))}>×</button></li>
      {/each}
    </ul>
  {:else}
    <p class="setting-hint">No vocabulary words yet. Add a word above to get started.</p>
  {/if}
  {#if vocabulary.status?.supported === false}
    <p class="setting-hint">Audio-assisted vocabulary is unavailable with this speech engine. Your words are saved; exact replacements below still work.</p>
  {:else if vocabulary.pending || vocabulary.status?.preparing}
    <p class="setting-hint" role="status">Preparing vocabulary support in the background. Dictation remains available.</p>
  {:else if vocabulary.status?.loaded}
    <p class="setting-success" role="status">Vocabulary support is ready.</p>
  {:else if !vocabulary.error}
    <p class="setting-hint">Saving your first words prepares local vocabulary support{vocabulary.status && !vocabulary.status.downloaded ? ` (about ${vocabulary.status.download_size_mb} MB)` : ''}. Dictation continues while it downloads.</p>
  {/if}
  {#if vocabulary.error}
    <div class="setting-error" role="alert"><p>Vocabulary support: {vocabulary.error}</p>
      <button class="secondary-button" type="button" disabled={vocabulary.pending} onclick={() => vocabulary.status && settingsStore.saved?.vocabulary.length ? vocabulary.retry() : vocabulary.refresh()}>Retry</button>
    </div>
  {/if}
</section>

<section class="setting-card">
  <h3>Exact replacements</h3>
  <p class="setting-hint">For a predictable correction, tell SottoASR exactly what to replace. These work without an additional model.</p>
  <DictionarySettings entries={settingsStore.current.dictionary} onchange={(entries) => settingsStore.update('dictionary', entries)} />
</section>

<style>
  .term-form { display:flex; align-items:end; gap:8px; margin-top:14px; }
  .term-input { flex:1; min-width:0; }
  .term-input input { display:block; margin-top:6px; width:100%; box-sizing:border-box; border:1px solid var(--border); background:var(--input-bg); color:var(--text-bright); padding:9px 10px; border-radius:8px; font:inherit; font-size:13px; }
  .term-list { padding:0; margin:12px 0; display:flex; flex-wrap:wrap; gap:7px; list-style:none; }
  .term-list li { display:flex; align-items:center; gap:7px; max-width:100%; border:1px solid var(--border); background:var(--input-bg); border-radius:8px; padding:4px 6px 4px 9px; font-size:13px; }
  .term-list span { overflow-wrap:anywhere; }
  .term-list button { flex:none; border:0; border-radius:4px; background:transparent; color:var(--text-dim); cursor:pointer; width:24px; height:24px; font-size:18px; }
  .term-list button:hover { color:var(--text-bright); background:var(--border); }
</style>
