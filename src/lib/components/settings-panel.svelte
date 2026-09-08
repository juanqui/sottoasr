<script lang="ts">
  import { onMount } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { settingsStore } from '../stores/settings.svelte';
  import { CleanupSetup } from '../stores/cleanup-setup.svelte';
  import { VocabularySetup } from '../stores/vocabulary-setup.svelte';
  import { createEventScope } from '../utils/event-scope';
  import ModelDashboard from './model-dashboard.svelte';
  import SettingsGeneral from './settings-general.svelte';
  import SettingsDictation from './settings-dictation.svelte';
  import SettingsVocabulary from './settings-vocabulary.svelte';
  import SettingsAdvanced from './settings-advanced.svelte';
  import ConfirmDialog from './confirm-dialog.svelte';
  import './settings-panels.css';

  const sections = [
    { id:'general', label:'General', hint:'Shortcuts, startup, and updates.' },
    { id:'dictation', label:'Dictation', hint:'Choose how your words reach the page.' },
    { id:'vocabulary', label:'Vocabulary', hint:'Help SottoASR recognize the words you use.' },
    { id:'advanced', label:'Advanced', hint:'Permissions and local model details.' },
  ] as const;
  let selected = $state(0);
  let feedback = $state('');
  let closeConfirm = $state(false);
  let closeError = $state('');
  let allowClose = false;
  let disposed = false;
  const cleanup = new CleanupSetup();
  const vocabulary = new VocabularySetup();
  let section = $derived(sections[selected]);

  async function save(closeAfter = false) {
    feedback = '';
    try {
      const result = await settingsStore.save();
      if (disposed) return;
      cleanup.acknowledgeSaved(result.settings.llm_cleanup_enabled);
      feedback = settingsStore.dirty ? 'Saved. Your newer edits are still unsaved.' : 'Settings saved.';
      void vocabulary.refresh();
      void cleanup.refresh();
      if (closeAfter && !settingsStore.dirty) await closeWindow();
      else closeConfirm = false;
    } catch { closeConfirm = false; }
  }

  function discard() {
    cleanup.cancel();
    settingsStore.discard();
    feedback = 'Changes discarded.';
  }

  async function closeWindow() {
    try { allowClose = true; await getCurrentWindow().close(); }
    catch (error) { allowClose = false; closeError = String(error); }
  }

  function navigate(event: KeyboardEvent, index: number) {
    let next = index;
    if (event.key === 'ArrowRight') next = (index + 1) % sections.length;
    else if (event.key === 'ArrowLeft') next = (index + sections.length - 1) % sections.length;
    else if (event.key === 'Home') next = 0;
    else if (event.key === 'End') next = sections.length - 1;
    else return;
    event.preventDefault();
    selected = next;
    document.getElementById(`settings-tab-${sections[next].id}`)?.focus();
  }

  onMount(() => {
    const scope = createEventScope((error) => { feedback = `Live status unavailable: ${String(error)}`; });
    void settingsStore.load();
    void scope.listen('llm-preparation-changed', () => { void cleanup.refresh(); })
      .then(() => { if (!disposed) void cleanup.refresh(); });
    void scope.listen('vocabulary-status', () => { void vocabulary.refresh(); })
      .then(() => { if (!disposed) void vocabulary.refresh(); });
    void getCurrentWindow().onCloseRequested((event) => {
      if (!allowClose && (settingsStore.dirty || settingsStore.saving || cleanup.pending)) {
        event.preventDefault();
        closeConfirm = true;
      }
    }).then((unlisten) => scope.add(unlisten)).catch((error) => { if (!disposed) closeError = String(error); });
    return () => { disposed = true; cleanup.dispose(); vocabulary.dispose(); settingsStore.invalidateLoad(); scope.dispose(); };
  });
</script>

<div class="settings-window">
  <header class="settings-header"><h1>Settings</h1><p>Make SottoASR work your way.</p></header>
  <ModelDashboard {cleanup} onconfigure={() => { selected = 1; }} />
  <div class="settings-nav" role="tablist" aria-label="Settings sections">
    {#each sections as item, index}
      <button type="button" role="tab" id={`settings-tab-${item.id}`} aria-controls="settings-section" aria-selected={selected === index} tabindex={selected === index ? 0 : -1} class:active={selected === index} onclick={() => { selected = index; }} onkeydown={(event) => navigate(event, index)}>{item.label}</button>
    {/each}
  </div>
  <div class="settings-content" id="settings-section" role="tabpanel" aria-labelledby={`settings-tab-${section.id}`} tabindex="0">
    {#if settingsStore.loading}
      <p class="load-state" role="status">Loading settings…</p>
    {:else if !settingsStore.loaded}
      <div class="setting-error" role="alert"><p>{settingsStore.error || 'Settings are unavailable.'}</p><button class="secondary-button" type="button" onclick={() => settingsStore.load()}>Retry</button></div>
    {:else}
      <div class="section-heading"><h2>{section.label}</h2><p>{section.hint}</p></div>
      {#if selected === 0}<SettingsGeneral />
      {:else if selected === 1}<SettingsDictation {cleanup} />
      {:else if selected === 2}<SettingsVocabulary {vocabulary} />
      {:else}<SettingsAdvanced {cleanup} />{/if}
    {/if}
  </div>
  <footer class="settings-footer">
    <div class="footer-feedback" aria-live="polite">
      {#if settingsStore.loading}<span>Loading settings…</span>
      {:else if !settingsStore.loaded}<span class="error">Settings unavailable</span>
      {:else if settingsStore.error}<span class="error">{settingsStore.error}</span>
      {:else if closeError}<span class="error">{closeError}</span>
      {:else if settingsStore.warnings.length}<span class="warning">Saved. {settingsStore.warnings.join(' ')}</span>
      {:else if settingsStore.saving}<span>Saving…</span>
      {:else if cleanup.pending}<span>Setup is running. Other changes can be saved.</span>
      {:else if settingsStore.dirty}<span>Unsaved changes</span>
      {:else}<span>{feedback || 'All changes saved'}</span>{/if}
    </div>
    <div class="footer-actions">
      <button type="button" class="cancel-button" onclick={discard} disabled={settingsStore.saving || (!settingsStore.dirty && !cleanup.pending)}>Cancel</button>
      <button type="button" class="save-button" onclick={() => save()} disabled={!settingsStore.loaded || settingsStore.loading || settingsStore.saving || !settingsStore.dirty}>{settingsStore.saving ? 'Saving…' : 'Save'}</button>
    </div>
  </footer>
</div>
<ConfirmDialog open={closeConfirm} title="Save your changes?" message={cleanup.pending ? 'Cleanup setup may finish in the background. Discarding changes keeps your saved preferences.' : 'You have unsaved settings. Save them before closing, or discard this draft.'} confirmLabel="Save and close" cancelLabel="Keep editing" secondaryLabel="Discard" busy={settingsStore.saving} confirmDisabled={cleanup.pending} onconfirm={() => save(true)} oncancel={() => { closeConfirm = false; }} onsecondary={() => { discard(); void closeWindow(); }} />

<style>
  .settings-window { height:100vh; display:flex; flex-direction:column; overflow:hidden; }
  .settings-header { padding:22px 24px 17px; flex:none; }
  h1 { font-size:23px; line-height:1.2; font-weight:650; color:var(--text-bright); margin:0; letter-spacing:-.4px; }
  .settings-header p { margin:5px 0 0; font-size:12px; color:var(--text-dim); }
  .settings-nav { display:flex; gap:4px; padding:0 20px 12px; border-bottom:1px solid var(--border); flex:none; }
  .settings-nav button { flex:1; padding:9px 5px; border:1px solid transparent; border-radius:8px; background:transparent; color:var(--text-dim); font:inherit; font-size:12px; font-weight:500; cursor:pointer; }
  .settings-nav button.active { background:var(--accent-bg); color:#93c5fd; border-color:#3b82f630; }
  .settings-nav button:hover { color:var(--text-bright); }
  .settings-content { flex:1; min-height:0; min-width:0; overflow:auto; padding:20px; }
  .section-heading { margin:0 0 16px; }
  .section-heading h2 { font-size:17px; font-weight:600; color:var(--text-bright); margin:0; }
  .section-heading p { font-size:12px; color:var(--text-dim); margin:4px 0 0; }
  .settings-footer { flex:none; display:flex; align-items:center; justify-content:space-between; gap:16px; padding:14px 20px; border-top:1px solid var(--border); background:var(--bg); }
  .footer-feedback { min-width:0; font-size:11px; color:var(--text-dim); overflow-wrap:anywhere; max-height:64px; overflow:auto; }
  .footer-feedback .error { color:#fca5a5; }
  .footer-feedback .warning { color:#fcd34d; }
  .footer-actions { display:flex; flex:none; gap:8px; }
  .footer-actions button { padding:8px 18px; font:inherit; font-size:12px; border-radius:8px; border:1px solid var(--border-hover); cursor:pointer; }
  .cancel-button { background:var(--card-bg); color:var(--text); }
  .save-button { background:var(--accent); border-color:var(--accent)!important; color:white; }
  .footer-actions button:disabled { opacity:.4; cursor:default; }
  .load-state { font-size:13px; color:var(--text-dim); text-align:center; padding:32px; }
</style>
