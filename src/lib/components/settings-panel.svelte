<script lang="ts">
  import { settingsStore } from '../stores/settings.svelte';
  import {
    checkMicrophonePermission,
    checkAccessibilityPermission,
    requestAccessibilityPermission,
  } from '../utils/tauri';

  // Permission status
  let micPermission: boolean | null = $state(null);
  let accessibilityPermission: boolean | null = $state(null);
  let checkingPermissions: boolean = $state(false);

  // Save feedback
  let saveMessage: string = $state('');

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

  // Load settings and permissions on mount
  $effect(() => {
    settingsStore.load();
    refreshPermissions();
  });

  async function refreshPermissions() {
    checkingPermissions = true;
    try {
      const [mic, acc] = await Promise.all([
        checkMicrophonePermission(),
        checkAccessibilityPermission(),
      ]);
      micPermission = mic;
      accessibilityPermission = acc;
    } catch (err) {
      console.error('Failed to check permissions:', err);
    } finally {
      checkingPermissions = false;
    }
  }

  async function handleRequestAccessibility() {
    await requestAccessibilityPermission();
    // Re-check after a delay to give user time to grant permission
    setTimeout(refreshPermissions, 2000);
  }

  async function handleSave() {
    try {
      await settingsStore.save();
      saveMessage = 'Settings saved';
      setTimeout(() => {
        saveMessage = '';
      }, 2000);
    } catch {
      saveMessage = 'Failed to save';
      setTimeout(() => {
        saveMessage = '';
      }, 3000);
    }
  }
</script>

<div class="settings-window">
  <header class="settings-header">
    <h1>Settings</h1>
  </header>

  <div class="settings-body">
    <!-- Keyboard Shortcuts -->
    <section class="settings-section">
      <h2>Keyboard Shortcuts</h2>
      <div class="field">
        <label for="ptt-shortcut">Push-to-talk</label>
        <input
          id="ptt-shortcut"
          type="text"
          class="text-input"
          bind:value={settingsStore.current.push_to_talk_shortcut}
          placeholder="CommandOrControl+Shift+Space"
        />
        <span class="field-hint">Hold to record, release to transcribe</span>
      </div>
      <div class="field">
        <label for="toggle-shortcut">Toggle recording</label>
        <input
          id="toggle-shortcut"
          type="text"
          class="text-input"
          bind:value={settingsStore.current.toggle_shortcut}
          placeholder="CommandOrControl+Shift+D"
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
        <span class="permission-badge" class:granted={micPermission === true} class:denied={micPermission === false}>
          {#if micPermission === null}
            Checking...
          {:else if micPermission}
            Granted
          {:else}
            Not Granted
          {/if}
        </span>
      </div>
      <div class="permission-row">
        <div class="permission-info">
          <span class="permission-label">Accessibility</span>
          <span class="permission-hint">Required for paste-at-cursor</span>
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
            <button class="grant-btn" onclick={handleRequestAccessibility} type="button">
              Open Settings
            </button>
          {/if}
        </div>
      </div>
      <button
        class="check-btn"
        onclick={refreshPermissions}
        disabled={checkingPermissions}
        type="button"
      >
        {checkingPermissions ? 'Checking...' : 'Check Permissions'}
      </button>
    </section>

    <!-- Save -->
    <div class="save-bar">
      {#if saveMessage}
        <span class="save-message" class:error={saveMessage.includes('Failed')}>
          {saveMessage}
        </span>
      {/if}
      <button
        class="save-btn"
        onclick={handleSave}
        disabled={settingsStore.saving}
        type="button"
      >
        {settingsStore.saving ? 'Saving...' : 'Save'}
      </button>
    </div>
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
    padding: 20px 24px 16px;
    border-bottom: 1px solid var(--border);
  }

  h1 {
    font-size: 22px;
    font-weight: 600;
    margin: 0;
    color: var(--text-bright);
    letter-spacing: -0.3px;
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

  /* Save bar */
  .save-bar {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 12px;
    padding: 18px 0 4px;
  }

  .save-message {
    font-size: 13px;
    color: #22c55e;
  }

  .save-message.error {
    color: #ef4444;
  }

  .save-btn {
    padding: 8px 24px;
    border: none;
    border-radius: 8px;
    background: var(--accent);
    color: white;
    font-size: 14px;
    font-weight: 500;
    font-family: inherit;
    cursor: pointer;
    transition: opacity 0.15s ease;
  }

  .save-btn:hover:not(:disabled) {
    opacity: 0.9;
  }

  .save-btn:disabled {
    opacity: 0.5;
    cursor: default;
  }
</style>
