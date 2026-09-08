<script lang="ts">
  import { onDestroy } from 'svelte';
  import { diffWords } from 'diff';

  let { text, suggestion, oncopy }: {
    text: string;
    suggestion: string;
    oncopy: (text: string) => void | Promise<void>;
  } = $props();
  let parts = $derived(diffWords(text, suggestion));
  let copied = $state(false);
  let copying = $state(false);
  let disposed = false;
  let feedbackTimer: ReturnType<typeof setTimeout> | null = null;

  async function copySuggestion() {
    if (copying) return;
    copying = true;
    copied = false;
    try {
      await oncopy(suggestion);
      if (disposed) return;
      copied = true;
      if (feedbackTimer) clearTimeout(feedbackTimer);
      feedbackTimer = setTimeout(() => { copied = false; feedbackTimer = null; }, 1500);
    } catch {
      // The History window owns the actionable clipboard error message.
    } finally {
      if (!disposed) copying = false;
    }
  }

  onDestroy(() => { disposed = true; if (feedbackTimer) clearTimeout(feedbackTimer); });
</script>

<section class="suggestion" aria-label="Experimental cleanup suggestion">
  <h3>Experimental suggestion</h3>
  <p class="warning">Review carefully: deletions can remove meaningful words. Your transcript is unchanged.</p>
  <div class="suggestion-diff">
    {#each parts as part}
      {#if part.removed}<del>{part.value}</del>
      {:else if part.added}<ins>{part.value}</ins>
      {:else}<span>{part.value}</span>{/if}
    {/each}
  </div>
  <p class="legend">Struck-through words would be removed.</p>
  <button type="button" disabled={copying} onclick={copySuggestion}>{copying ? 'Copying suggestion…' : 'Copy suggestion'}</button>
  <span class="feedback" role="status">{copied ? 'Suggestion copied' : ''}</span>
</section>

<style>
  .suggestion { margin:0 16px 14px; padding:12px; border:1px solid #fbbf2438; border-radius:8px; background:#fbbf2408; }
  h3 { margin:0 0 5px; font-size:12px; font-weight:600; color:#fcd34d; }
  .warning, .legend { margin:0 0 10px; font-size:11px; line-height:1.5; color:var(--text-dim); }
  .suggestion-diff { font-size:14px; line-height:1.6; color:var(--text-bright); white-space:pre-wrap; overflow-wrap:anywhere; }
  del { color:#fca5a5; background:#ef44442e; text-decoration:line-through; }
  ins { color:#86efac; background:#22c55e2e; }
  .legend { margin:7px 0 10px; }
  button { padding:5px 9px; border:1px solid var(--border); border-radius:6px; font:inherit; font-size:11px; background:var(--card-bg); color:var(--text-bright); cursor:pointer; }
  button:disabled { opacity:.6; cursor:default; }
  button:hover:not(:disabled) { border-color:var(--border-hover); }
  .feedback { margin-left:8px; font-size:11px; color:var(--accent); }
</style>
