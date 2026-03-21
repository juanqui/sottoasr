<script lang="ts">
  import { settingsStore } from '../stores/settings.svelte';
  import {
    checkMicrophonePermission,
    checkAccessibilityPermission,
    requestAccessibilityPermission,
    getLlmStatus,
    downloadLlmModel,
    deleteLlmModel,
  } from '../utils/tauri';
  import type { Settings, LlmStatus } from '../utils/tauri';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { onMount } from 'svelte';
  import ShortcutRecorder from './shortcut-recorder.svelte';

  // Permission status (using structured check)
  let micPermission: string = $state('checking');
  let accessibilityPermission: boolean | null = $state(null);
  let accessibilityFunctional: boolean | null = $state(null);
  let checkingPermissions: boolean = $state(false);
  let fixingAccessibility: boolean = $state(false);

  // Save feedback
  let saveMessage: string = $state('');

  // Track which shortcut recorder is active (mutual exclusion)
  let activeRecorder: string | null = $state(null);

  // LLM status
  let llmStatus: LlmStatus | null = $state(null);
  let llmDownloading = $state(false);
  let llmError = $state('');
  let llmDeleteConfirm = $state(false);

  // Snapshot of settings at load time for dirty detection
  let savedSnapshot: string = $state('');

  // Dirty detection: compare current settings JSON to saved snapshot
  let isDirty = $derived(
    settingsStore.loaded && JSON.stringify(settingsStore.current) !== savedSnapshot
  );


  // Available languages
  const languages = [
    { value: 'auto', label: 'Auto-detect' },
    { value: 'en', label: 'English' },
    { value: 'es', label: 'Spanish' },
    { value: 'fr', label: 'French' },
    { value: 'de', label: 'German' },
    { value: 'it', label: 'Italian' },
    { value: 'pt', label: 'Portuguese' },
    { value: 'nl', label: 'Dutch' },
    { value: 'ja', label: 'Japanese' },
    { value: 'ko', label: 'Korean' },
    { value: 'zh', label: 'Chinese' },
    { value: 'ru', label: 'Russian' },
    { value: 'ar', label: 'Arabic' },
    { value: 'hi', label: 'Hindi' },
  ];

  onMount(() => {
    const cleanups: Array<() => void> = [];

    // Load settings and permissions
    settingsStore.load().then(() => {
      savedSnapshot = JSON.stringify(settingsStore.current);
    });
    refreshPermissions();
    refreshLlmStatus();

    // Listen for LLM download events
    listen('llm-download-complete', () => {
      llmDownloading = false;
      refreshLlmStatus();
    }).then((u) => cleanups.push(u));

    listen<{ message: string }>('llm-download-error', (event) => {
      llmDownloading = false;
      llmError = event.payload.message;
      refreshLlmStatus();
    }).then((u) => cleanups.push(u));

    return () => {
      cleanups.forEach((fn) => fn());
    };
  });

  async function refreshPermissions() {
    checkingPermissions = true;
    try {
      const status = await invoke<{
        microphone: string;
        accessibility_api: boolean;
        accessibility_functional: boolean;
        needs_restart: boolean;
      }>('check_all_permissions');
      micPermission = status.microphone;
      accessibilityPermission = status.accessibility_api;
      accessibilityFunctional = status.accessibility_functional;
    } catch (err) {
      console.error('Failed to check permissions:', err);
    } finally {
      checkingPermissions = false;
    }
  }

  async function handleRequestAccessibility() {
    await requestAccessibilityPermission();
    setTimeout(refreshPermissions, 2000);
  }

  async function handleFixAccessibility() {
    fixingAccessibility = true;
    try {
      await invoke('fix_accessibility_permission');
      // Wait for user to grant in System Settings, then re-check
      setTimeout(refreshPermissions, 3000);
    } catch (err) {
      console.error('Fix accessibility failed:', err);
    } finally {
      fixingAccessibility = false;
    }
  }

  async function handleSave() {
    try {
      await settingsStore.save();
      savedSnapshot = JSON.stringify(settingsStore.current);
      saveMessage = 'Settings saved';

      // Re-register shortcuts with the new values (non-blocking)
      try {
        await invoke('apply_shortcuts');
        saveMessage = 'Settings saved & shortcuts applied';
      } catch (e) {
        console.error('Shortcut registration failed:', e);
        saveMessage = 'Saved (shortcuts may need restart)';
      }

      setTimeout(() => { saveMessage = ''; }, 2500);
    } catch (e) {
      console.error('Save failed:', e);
      saveMessage = `Save failed: ${e}`;
      setTimeout(() => { saveMessage = ''; }, 4000);
    }
  }

  function handleDiscard() {
    settingsStore.current = JSON.parse(savedSnapshot);
    saveMessage = '';
  }

  async function refreshLlmStatus() {
    try {
      llmStatus = await getLlmStatus();
    } catch (err) {
      console.error('Failed to get LLM status:', err);
    }
  }

  async function handleLlmDownload() {
    llmDownloading = true;
    llmError = '';
    try {
      await downloadLlmModel();
    } catch (err: any) {
      llmError = err?.toString() || 'Download failed';
      llmDownloading = false;
    }
  }

  async function handleLlmDelete() {
    if (!llmDeleteConfirm) {
      llmDeleteConfirm = true;
      return;
    }
    try {
      settingsStore.update('llm_cleanup_enabled', false);
      settingsStore.update('llm_markdown_mode', false);
      await deleteLlmModel();
      llmDeleteConfirm = false;
      refreshLlmStatus();
    } catch (err: any) {
      llmError = err?.toString() || 'Delete failed';
      llmDeleteConfirm = false;
    }
  }
</script>

<div class="settings-window">
  <header class="settings-header">
    <h1>Settings</h1>
    <div class="header-actions">
      {#if saveMessage}
        <span class="save-message" class:error={saveMessage.includes('Failed')}>
          {saveMessage}
        </span>
      {/if}
      <button class="discard-btn" onclick={handleDiscard} disabled={!isDirty} type="button">
        Cancel
      </button>
      <button
        class="save-btn"
        onclick={handleSave}
        disabled={settingsStore.saving || !isDirty}
        type="button"
      >
        {settingsStore.saving ? 'Saving...' : 'Save'}
      </button>
    </div>
  </header>

  <div class="settings-body">
    <!-- Keyboard Shortcuts -->
    <section class="settings-section">
      <h2>Keyboard Shortcuts</h2>
      <div class="field">
        <label>Push-to-talk</label>
        <ShortcutRecorder
          value={settingsStore.current.push_to_talk_shortcut}
          onchange={(v) => settingsStore.update('push_to_talk_shortcut', v)}
          disabled={activeRecorder !== null && activeRecorder !== 'ptt'}
          onrecordstart={() => { activeRecorder = 'ptt'; }}
          onrecordend={() => { activeRecorder = null; }}
        />
        <span class="field-hint">Hold to record, release to transcribe</span>
      </div>
      <div class="field">
        <label>Toggle recording</label>
        <ShortcutRecorder
          value={settingsStore.current.toggle_shortcut}
          onchange={(v) => settingsStore.update('toggle_shortcut', v)}
          disabled={activeRecorder !== null && activeRecorder !== 'toggle'}
          onrecordstart={() => { activeRecorder = 'toggle'; }}
          onrecordend={() => { activeRecorder = null; }}
        />
        <span class="field-hint">Press to start, press again to stop</span>
      </div>
    </section>

    <!-- Behavior -->
    <section class="settings-section">
      <h2>Behavior</h2>
      <div class="toggle-field">
        <div class="toggle-info">
          <span class="toggle-label">Show overlay</span>
          <span class="toggle-hint">Display recording pill during capture</span>
        </div>
        <label class="switch">
          <input type="checkbox" bind:checked={settingsStore.current.show_overlay} />
          <span class="slider"></span>
        </label>
      </div>
      <div class="toggle-field">
        <div class="toggle-info">
          <span class="toggle-label">Auto-paste</span>
          <span class="toggle-hint">Paste transcribed text at cursor position</span>
        </div>
        <label class="switch">
          <input type="checkbox" bind:checked={settingsStore.current.auto_paste} />
          <span class="slider"></span>
        </label>
      </div>
      <div class="toggle-field">
        <div class="toggle-info">
          <span class="toggle-label">Restore clipboard</span>
          <span class="toggle-hint">Restore previous clipboard after pasting</span>
        </div>
        <label class="switch">
          <input type="checkbox" bind:checked={settingsStore.current.restore_clipboard} />
          <span class="slider"></span>
        </label>
      </div>
      <div class="toggle-field">
        <div class="toggle-info">
          <span class="toggle-label">Launch at login</span>
          <span class="toggle-hint">Start Sotto when you log in</span>
        </div>
        <label class="switch">
          <input type="checkbox" bind:checked={settingsStore.current.launch_at_login} />
          <span class="slider"></span>
        </label>
      </div>
    </section>

    <!-- AI Transcript Cleanup -->
    {#if llmStatus?.available}
    <section class="settings-section">
      <h2>AI Transcript Cleanup</h2>
      <div class="toggle-field">
        <div class="toggle-info">
          <span class="toggle-label">Clean up transcriptions with AI</span>
          <span class="toggle-hint">Uses Qwen3.5-0.8B (~570 MB) running locally via MLX on Metal GPU</span>
        </div>
        <label class="switch">
          <input
            type="checkbox"
            checked={settingsStore.current.llm_cleanup_enabled}
            disabled={llmDownloading}
            onchange={(e) => {
              const enabled = (e.target as HTMLInputElement).checked;
              if (enabled && !llmStatus?.downloaded) {
                // Need to download model first
                (e.target as HTMLInputElement).checked = false;
                handleLlmDownload().then(() => {
                  settingsStore.update('llm_cleanup_enabled', true);
                });
              } else {
                settingsStore.update('llm_cleanup_enabled', enabled);
                if (!enabled) {
                  // Unload model on disable (async, non-blocking)
                  import('../utils/tauri').then(({ unloadLlmModel }) => unloadLlmModel().catch(() => {}));
                }
              }
            }}
          />
          <span class="slider"></span>
        </label>
      </div>

      <!-- Model status -->
      <div class="llm-status">
        {#if llmDownloading}
          <div class="llm-downloading">
            <div class="spinner-small"></div>
            <span>Downloading model...</span>
          </div>
        {:else if llmStatus?.downloaded}
          <span class="llm-badge ready">Model Ready</span>
        {:else}
          <button class="download-btn" onclick={handleLlmDownload} type="button">
            Download Model (~600 MB)
          </button>
        {/if}
      </div>

      {#if llmError}
        <div class="llm-error">{llmError}</div>
      {/if}

      {#if settingsStore.current.llm_cleanup_enabled}
        <div class="toggle-field">
          <div class="toggle-info">
            <span class="toggle-label">Format as Markdown</span>
            <span class="toggle-hint">Structures longer dictations with headings and lists (experimental)</span>
          </div>
          <label class="switch">
            <input type="checkbox" bind:checked={settingsStore.current.llm_markdown_mode} />
            <span class="slider"></span>
          </label>
        </div>
      {/if}

      {#if llmStatus?.downloaded}
        <button
          class="delete-btn"
          onclick={handleLlmDelete}
          type="button"
        >
          {llmDeleteConfirm ? 'Are you sure? Click again to confirm' : 'Delete Model (~600 MB)'}
        </button>
      {/if}
    </section>
    {/if}

    <!-- Language & History -->
    <section class="settings-section">
      <h2>Language & History</h2>
      <div class="field">
        <label for="language">Transcription language</label>
        <select
          id="language"
          class="select-input"
          bind:value={settingsStore.current.language}
        >
          {#each languages as lang}
            <option value={lang.value}>{lang.label}</option>
          {/each}
        </select>
      </div>
      <div class="field">
        <label for="max-history">Maximum history entries</label>
        <input
          id="max-history"
          type="number"
          class="text-input short"
          bind:value={settingsStore.current.max_history}
          min="10"
          max="10000"
          step="10"
        />
      </div>
    </section>

    <!-- Permissions -->
    <section class="settings-section">
      <h2>Permissions</h2>
      <div class="permission-row">
        <div class="permission-info">
          <span class="permission-label">Microphone</span>
          <span class="permission-hint">Required for audio capture</span>
        </div>
        <div class="permission-action">
          <span class="permission-badge" class:granted={micPermission === 'authorized'} class:denied={micPermission === 'denied'}>
            {#if micPermission === 'checking'}
              Checking...
            {:else if micPermission === 'authorized'}
              Granted
            {:else if micPermission === 'denied'}
              Denied
            {:else}
              Not Set
            {/if}
          </span>
          {#if micPermission !== 'authorized' && micPermission !== 'checking'}
            <button class="grant-btn" onclick={() => invoke('open_microphone_settings')} type="button">
              Open Settings
            </button>
          {/if}
        </div>
      </div>
      <div class="permission-row">
        <div class="permission-info">
          <span class="permission-label">Accessibility</span>
          <span class="permission-hint">Required for paste-at-cursor, hotkeys, and key detection</span>
        </div>
        <div class="permission-action">
          <span class="permission-badge" class:granted={accessibilityPermission === true} class:denied={accessibilityPermission === false}>
            {#if accessibilityPermission === null}
              Checking...
            {:else if accessibilityPermission}
              Granted
            {:else}
              Not Granted
            {/if}
          </span>
          {#if accessibilityPermission === false}
            <button
              class="grant-btn"
              onclick={handleFixAccessibility}
              disabled={fixingAccessibility}
              type="button"
            >
              {fixingAccessibility ? 'Fixing...' : 'Fix Permission'}
            </button>
          {/if}
        </div>
      </div>
      {#if accessibilityPermission === false}
        <p class="permission-explain">
          Sotto appears enabled in System Settings but the app was updated since then.
          Click "Fix Permission" to re-register, then toggle Sotto ON in the System Settings
          window that opens. You may need to restart Sotto afterwards.
        </p>
      {/if}
      <button
        class="check-btn"
        onclick={refreshPermissions}
        disabled={checkingPermissions}
        type="button"
      >
        {checkingPermissions ? 'Checking...' : 'Check Permissions'}
      </button>
    </section>
  </div>
</div>

<style>
  .settings-window {
    display: flex;
    flex-direction: column;
    height: 100vh;
    overflow: hidden;
  }

  .settings-header {
    flex-shrink: 0;
    padding: 16px 24px;
    border-bottom: 1px solid var(--border);
    display: flex;
    align-items: center;
    justify-content: space-between;
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
    align-items: center;
    gap: 8px;
  }

  .save-message {
    font-size: 12px;
    color: #22c55e;
  }

  .save-message.error {
    color: #ef4444;
  }

  .discard-btn {
    padding: 6px 14px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: none;
    color: var(--text-dim);
    font-size: 13px;
    font-family: inherit;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .discard-btn:hover {
    border-color: var(--border-hover);
    color: var(--text);
  }

  .save-btn {
    padding: 6px 16px;
    border: none;
    border-radius: 6px;
    background: var(--accent);
    color: white;
    font-size: 13px;
    font-weight: 500;
    font-family: inherit;
    cursor: pointer;
    transition: opacity 0.15s ease;
  }

  .save-btn:hover:not(:disabled) {
    opacity: 0.9;
  }

  .save-btn:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .settings-body {
    flex: 1;
    overflow-y: auto;
    padding: 8px 24px 24px;
  }

  .settings-section {
    padding: 18px 0;
    border-bottom: 1px solid var(--border);
  }

  .settings-section:last-of-type {
    border-bottom: none;
  }

  h2 {
    font-size: 13px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-dim);
    margin: 0 0 14px;
  }

  /* Text & select fields */
  .field {
    margin-bottom: 14px;
  }

  .field:last-child {
    margin-bottom: 0;
  }

  .field label {
    display: block;
    font-size: 13px;
    font-weight: 500;
    color: var(--text);
    margin-bottom: 6px;
  }

  .text-input,
  .select-input {
    width: 100%;
    padding: 8px 12px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--input-bg);
    color: var(--text-bright);
    font-size: 13px;
    font-family: inherit;
    outline: none;
    transition: border-color 0.15s ease;
    box-sizing: border-box;
  }

  .text-input:focus,
  .select-input:focus {
    border-color: var(--accent);
  }

  .text-input.short {
    width: 120px;
  }

  .select-input {
    cursor: pointer;
    -webkit-appearance: none;
    appearance: none;
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 12 12'%3E%3Cpath d='M3 5l3 3 3-3' stroke='%239ca3af' stroke-width='1.5' fill='none' stroke-linecap='round'/%3E%3C/svg%3E");
    background-repeat: no-repeat;
    background-position: right 10px center;
    padding-right: 30px;
  }

  .select-input option {
    background: var(--card-bg);
    color: var(--text);
  }

  .field-hint {
    display: block;
    font-size: 11px;
    color: var(--text-dim);
    margin-top: 4px;
  }

  /* Toggle fields */
  .toggle-field {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 0;
  }

  .toggle-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .toggle-label {
    font-size: 13px;
    font-weight: 500;
    color: var(--text);
  }

  .toggle-hint {
    font-size: 11px;
    color: var(--text-dim);
  }

  /* Switch toggle */
  .switch {
    position: relative;
    display: inline-block;
    width: 40px;
    height: 22px;
    flex-shrink: 0;
  }

  .switch input {
    opacity: 0;
    width: 0;
    height: 0;
  }

  .slider {
    position: absolute;
    inset: 0;
    cursor: pointer;
    background: var(--border);
    border-radius: 11px;
    transition: background 0.2s ease;
  }

  .slider::before {
    content: '';
    position: absolute;
    width: 16px;
    height: 16px;
    left: 3px;
    bottom: 3px;
    background: white;
    border-radius: 50%;
    transition: transform 0.2s ease;
  }

  .switch input:checked + .slider {
    background: var(--accent);
  }

  .switch input:checked + .slider::before {
    transform: translateX(18px);
  }

  /* Permissions */
  .permission-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 0;
  }

  .permission-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .permission-label {
    font-size: 13px;
    font-weight: 500;
    color: var(--text);
  }

  .permission-hint {
    font-size: 11px;
    color: var(--text-dim);
  }

  .permission-action {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .permission-badge {
    font-size: 12px;
    padding: 3px 10px;
    border-radius: 6px;
    background: var(--hover-bg);
    color: var(--text-dim);
  }

  .permission-badge.granted {
    background: rgba(34, 197, 94, 0.12);
    color: #22c55e;
  }

  .permission-badge.denied {
    background: rgba(239, 68, 68, 0.12);
    color: #ef4444;
  }

  .grant-btn {
    padding: 4px 10px;
    border: 1px solid var(--accent);
    border-radius: 6px;
    background: none;
    color: var(--accent);
    font-size: 12px;
    font-family: inherit;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .grant-btn:hover {
    background: rgba(59, 130, 246, 0.1);
  }

  .permission-explain {
    font-size: 12px;
    color: var(--text-dim);
    margin: 6px 0 0;
    line-height: 1.5;
    padding: 8px 12px;
    background: rgba(59, 130, 246, 0.06);
    border-radius: 6px;
    border: 1px solid rgba(59, 130, 246, 0.15);
  }

  .check-btn {
    margin-top: 12px;
    padding: 7px 14px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: none;
    color: var(--text-dim);
    font-size: 13px;
    font-family: inherit;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .check-btn:hover:not(:disabled) {
    border-color: var(--border-hover);
    color: var(--text);
  }

  .check-btn:disabled {
    opacity: 0.5;
    cursor: default;
  }

  /* LLM section */
  .llm-status {
    margin-top: 8px;
    margin-bottom: 4px;
  }

  .llm-downloading {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    color: var(--text-muted);
  }

  .spinner-small {
    width: 14px;
    height: 14px;
    border-radius: 50%;
    border: 2px solid var(--border);
    border-top-color: var(--accent);
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to { transform: rotate(360deg); }
  }

  .llm-badge {
    display: inline-block;
    font-size: 12px;
    padding: 3px 10px;
    border-radius: 10px;
    font-weight: 500;
  }

  .llm-badge.ready {
    background: rgba(34, 197, 94, 0.15);
    color: rgb(34, 197, 94);
  }

  .download-btn {
    padding: 6px 14px;
    border-radius: 8px;
    border: 1px solid var(--border);
    background: var(--bg-secondary);
    color: var(--text);
    font-size: 13px;
    cursor: pointer;
  }

  .download-btn:hover {
    background: var(--bg-hover);
  }

  .delete-btn {
    margin-top: 8px;
    padding: 4px 10px;
    border-radius: 6px;
    border: 1px solid rgba(239, 68, 68, 0.3);
    background: transparent;
    color: rgba(239, 68, 68, 0.8);
    font-size: 12px;
    cursor: pointer;
  }

  .delete-btn:hover {
    background: rgba(239, 68, 68, 0.1);
    color: rgb(239, 68, 68);
  }

  .llm-setup-notice {
    padding: 12px 16px;
    border-radius: 8px;
    background: rgba(59, 130, 246, 0.08);
    border: 1px solid rgba(59, 130, 246, 0.2);
    font-size: 13px;
    color: var(--text-muted);
    line-height: 1.5;
  }

  .llm-setup-notice p {
    margin: 4px 0;
  }

  .llm-install-cmd {
    display: block;
    margin: 8px 0;
    padding: 8px 12px;
    border-radius: 6px;
    background: rgba(0, 0, 0, 0.3);
    color: rgb(129, 199, 132);
    font-family: 'SF Mono', Menlo, Monaco, monospace;
    font-size: 12px;
    user-select: all;
  }

  .llm-setup-hint {
    font-size: 12px;
    opacity: 0.7;
  }

  .llm-error {
    margin-top: 6px;
    font-size: 12px;
    color: rgb(239, 68, 68);
  }
</style>
