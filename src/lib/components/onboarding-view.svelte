<script lang="ts">
  import { createEventScope } from '../utils/event-scope';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { onMount, onDestroy } from 'svelte';
  import {
    checkAllPermissions, completeSetup, downloadModel, getAsrBackend, getSettings,
    openAccessibilitySettings, openMicrophoneSettings, requestAccessibilityPermission,
    requestMicrophonePermission, restartApp,
  } from '../utils/tauri';
  import type { DownloadProgress } from '../utils/tauri';
  import DownloadProgressBar from './download-progress.svelte';
  import { shortcutKeys } from './shortcut-format';

  type Step = 'welcome' | 'permissions' | 'model' | 'ready' | 'restart' | 'error';

  let currentStep = $state<Step>('welcome');
  let micPermission = $state<string>('not_determined'); // "authorized", "denied", "not_determined", "restricted"
  let axPermission = $state(false);
  let axFunctional = $state(false);
  let axRequested = $state(false);
  let needsRestart = $state(false);
  let backendName = $state('');
  let progressMessage = $state('');
  let download = $state<DownloadProgress | null>(null);
  let errorMessage = $state('');
  let actionError = $state('');
  let isProcessing = $state(false);
  let restarting = $state(false);
  let restartError = $state('');
  let shortcuts = $state<{ keys: string[]; description: string }[]>([]);
  let shortcutsUnavailable = $state(false);

  // Poll interval for permission checks
  let permissionPollInterval: ReturnType<typeof setInterval> | null = null;
  let permissionCheckTimeout: ReturnType<typeof setTimeout> | null = null;

  // Event listener cleanup
  const scope = createEventScope();
  let disposed = false;
  let checkingPermissions = false;

  // Derived: both permissions granted and functional
  let micGranted = $derived(micPermission === 'authorized');
  let allPermissionsGranted = $derived(micGranted && axPermission && axFunctional);

  onMount(async () => {
    await Promise.all([
      scope.listen<{ step: string; message: string }>('setup-progress', (event) => {
        progressMessage = event.payload.message;
      }),
      scope.listen<DownloadProgress>('model-download-progress', (event) => {
        if (isProcessing) download = event.payload;
      }),
    ]);
    if (disposed) return;
    try {
      const info = await getAsrBackend();
      if (disposed) return;
      backendName = info.backend;
      if (info.model_available && currentStep === 'welcome') goToPermissions();
    } catch (e) {
      console.error('Failed to get backend info:', e);
    }
  });

  onDestroy(() => {
    stopPermissionPolling();
    disposed = true;
    scope.dispose();
  });

  function startPermissionPolling() {
    stopPermissionPolling();
    checkPermissions(); // immediate check
    // Poll every 1.5s so the UI updates quickly when the user toggles in System Settings
    permissionPollInterval = setInterval(checkPermissions, 1500);
  }

  function stopPermissionPolling() {
    if (permissionPollInterval) {
      clearInterval(permissionPollInterval);
      permissionPollInterval = null;
    }
    if (permissionCheckTimeout) {
      clearTimeout(permissionCheckTimeout);
      permissionCheckTimeout = null;
    }
  }

  async function checkPermissions() {
    if (disposed || checkingPermissions) return;
    checkingPermissions = true;
    try {
      const status = await checkAllPermissions();
      if (disposed) return;
      micPermission = status.microphone;
      axPermission = status.accessibility_api;
      axFunctional = status.accessibility_functional;
      needsRestart = status.needs_restart;
    } catch (e) {
      console.error('Permission check failed:', e);
    } finally { checkingPermissions = false; }
  }

  async function requestMicrophone() {
    actionError = '';
    try {
      await requestMicrophonePermission();
      // Recheck after prompt
      if (!disposed) permissionCheckTimeout = setTimeout(checkPermissions, 500);
    } catch (e) {
      if (!disposed) actionError = String(e);
    }
  }

  /** The first press shows the macOS prompt, which also lists SottoASR in System Settings. */
  async function requestAccessibility() {
    actionError = '';
    try {
      if (axRequested) await openAccessibilitySettings();
      else await requestAccessibilityPermission();
      if (!disposed) axRequested = true;
    } catch (e) {
      if (!disposed) actionError = String(e);
    }
  }

  async function showMicrophoneSettings() {
    actionError = '';
    try { await openMicrophoneSettings(); }
    catch (e) { if (!disposed) actionError = String(e); }
  }

  async function restartNow() {
    if (restarting) return;
    restarting = true;
    restartError = '';
    try {
      await restartApp();
    } catch (error) { if (!disposed) restartError = String(error); }
    finally { if (!disposed) restarting = false; }
  }

  function goToPermissions() {
    currentStep = 'permissions';
    startPermissionPolling();
  }

  /** Runs from Permissions, and again in place from the error step. */
  async function startSetup() {
    stopPermissionPolling();

    // If accessibility needs restart, go to restart step instead
    if (needsRestart) {
      currentStep = 'restart';
      return;
    }
    if (isProcessing) return;

    isProcessing = true;
    currentStep = 'model';
    errorMessage = '';
    download = null;
    progressMessage = 'Preparing speech recognition…';

    try {
      // The parakeet build needs its files before setup can load them; the
      // FluidAudio build downloads during setup and returns at once here.
      const backend = await getAsrBackend();
      if (disposed) return;
      if (!backend.model_available) {
        progressMessage = 'Downloading the speech model…';
        await downloadModel();
        if (disposed) return;
      }
      const result = await completeSetup();
      if (disposed) return;
      if (result.asr_ready) {
        currentStep = 'ready';
        void loadShortcuts();
      } else {
        errorMessage = result.error || 'Speech recognition could not start. Try again.';
        currentStep = 'error';
      }
    } catch (e) {
      if (disposed) return;
      errorMessage = String(e) || 'Setup failed. Try again.';
      currentStep = 'error';
    } finally {
      if (!disposed) isProcessing = false;
    }
  }

  async function loadShortcuts() {
    try {
      const settings = await getSettings();
      if (disposed) return;
      shortcuts = [
        { keys: shortcutKeys(settings.push_to_talk_shortcut), description: 'Push-to-talk — hold to record, release to transcribe' },
        { keys: shortcutKeys(settings.toggle_shortcut), description: 'Toggle — press to start, press again to stop' },
        { keys: shortcutKeys(settings.cancel_shortcut), description: 'Cancel current recording' },
      ];
    } catch {
      if (!disposed) shortcutsUnavailable = true;
    }
  }

  async function closeOnboarding() {
    stopPermissionPolling();
    const win = getCurrentWindow();
    await win.close();
  }
</script>

<div class="onboarding">
  <!-- Step 1: Welcome -->
  {#if currentStep === 'welcome'}
    <div class="step">
      <div class="icon-large">🎙</div>
      <h1>Welcome to SottoASR</h1>
      <p class="subtitle">
        Local, privacy-first speech-to-text for macOS.
        Press a hotkey, speak, and text appears at your cursor.
      </p>

      <div class="features">
        <div class="feature">
          <span class="feature-icon">🔒</span>
          <div>
            <strong>100% Local</strong>
            <span>Audio never leaves your Mac</span>
          </div>
        </div>
        <div class="feature">
          <span class="feature-icon">⚡</span>
          <div>
            <strong>Lightning Fast</strong>
            <span>Powered by Apple Neural Engine</span>
          </div>
        </div>
        <div class="feature">
          <span class="feature-icon">🌍</span>
          <div>
            <strong>25 Languages</strong>
            <span>Auto-detected from your speech</span>
          </div>
        </div>
      </div>

      <p class="note">
        ASR Backend: <strong>{backendName || 'Loading...'}</strong>
      </p>

      <button class="primary" onclick={goToPermissions}>
        Get Started
      </button>
    </div>

  <!-- Step 2: Permissions -->
  {:else if currentStep === 'permissions'}
    <div class="step">
      <h2>Permissions</h2>
      <p class="subtitle">SottoASR needs two permissions to work:</p>

      <div class="permission-list">
        <div class="permission-item" class:granted={micGranted}>
          <div class="permission-status">
            {#if micGranted}
              <span class="check">✓</span>
            {:else if micPermission === 'denied'}
              <span class="denied">✗</span>
            {:else}
              <span class="pending">○</span>
            {/if}
          </div>
          <div class="permission-info">
            <strong>Microphone</strong>
            <span>Required to capture your speech</span>
          </div>
          {#if micPermission === 'not_determined'}
            <button class="secondary small" onclick={requestMicrophone}>
              Grant Access
            </button>
          {:else if micPermission === 'denied'}
            <button class="secondary small" onclick={showMicrophoneSettings}>
              Open Settings
            </button>
          {/if}
        </div>

        <div class="permission-item" class:granted={axPermission && axFunctional}>
          <div class="permission-status">
            {#if axPermission && axFunctional}
              <span class="check">✓</span>
            {:else if needsRestart}
              <span class="warning-icon">!</span>
            {:else}
              <span class="pending">○</span>
            {/if}
          </div>
          <div class="permission-info">
            <strong>Accessibility</strong>
            <span>Required to paste text at your cursor</span>
          </div>
          {#if needsRestart}
            <span class="permission-note restart-note">Restart required</span>
          {:else if !axPermission}
            <button class="secondary small" onclick={requestAccessibility}>
              {axRequested ? 'Open System Settings' : 'Grant Access'}
            </button>
          {/if}
        </div>
      </div>

      {#if allPermissionsGranted}
        <p class="note success-note">
          All permissions granted — you're ready to continue!
        </p>
      {:else if needsRestart}
        <p class="note warning-note">
          Accessibility permission is granted but requires a <strong>restart</strong> to take effect.
          You can continue setup and restart afterwards.
        </p>
      {:else if micPermission === 'denied'}
        <p class="note">
          Microphone access was denied. Open <strong>System Settings</strong> to grant it.
        </p>
      {:else if !axPermission}
        <p class="note">
          Toggle SottoASR <strong>on</strong> in System Settings &gt; Privacy &amp; Security &gt; Accessibility.
        </p>
      {/if}
      {#if actionError}<p class="error-message" role="alert">{actionError}</p>{/if}

      <div class="button-row">
        <button class="secondary" onclick={() => { stopPermissionPolling(); currentStep = 'welcome'; }}>
          Back
        </button>
        <button class="primary" onclick={startSetup}>
          Continue
        </button>
      </div>
    </div>

  <!-- Step 3: Model Download / Init -->
  {:else if currentStep === 'model'}
    <div class="step">
      <h2>Setting Up Speech Recognition</h2>

      <div class="progress-container">
        <div class="progress-spinner" class:active={isProcessing}></div>

        <p class="progress-message">{progressMessage || 'Preparing...'}</p>

        {#if download}
          <DownloadProgressBar progress={download} label="Speech model download" />
        {/if}
      </div>

      <p class="note">
        This downloads the Parakeet Ultra speech recognition model (about 630 MB).
        <br />First-time setup may take 1-2 minutes depending on your internet speed.
        <br />Models are cached locally for instant startup next time.
      </p>
    </div>

  <!-- Step 4: Ready -->
  {:else if currentStep === 'ready'}
    <div class="step">
      <div class="icon-large success">✓</div>
      <h2>SottoASR is Ready!</h2>
      <p class="subtitle">Speech recognition is set up and ready to use.</p>

      {#if shortcuts.length}
        <div class="shortcuts-preview">
          {#each shortcuts as shortcut (shortcut.description)}
            <div class="shortcut">
              {#each shortcut.keys as key}<kbd>{key}</kbd>{/each}
              <span>{shortcut.description}</span>
            </div>
          {/each}
        </div>
      {:else if shortcutsUnavailable}
        <p class="note">Your shortcuts are listed in Settings under General.</p>
      {/if}

      <p class="note">
        SottoASR lives in your menu bar. Look for the microphone icon at the top of your screen.
      </p>

      <button class="primary" onclick={closeOnboarding}>
        Start Using SottoASR
      </button>
    </div>

  <!-- Step: Restart Required -->
  {:else if currentStep === 'restart'}
    <div class="step">
      <div class="icon-large warning-icon-large">!</div>
      <h2>Restart Required</h2>
      <p class="subtitle">
        Accessibility permission has been granted, but macOS requires
        an app restart for it to take full effect.
      </p>
      <p class="note">
        Without restarting, SottoASR won't be able to paste transcribed text
        at your cursor. Your settings and model data will be preserved.
      </p>
      {#if restartError}<p class="error-message" role="alert">{restartError}</p>{/if}

      <div class="button-row">
        <button class="secondary" onclick={closeOnboarding}>
          Later
        </button>
        <button class="primary" disabled={restarting} onclick={restartNow}>
          Restart Now
        </button>
      </div>
    </div>

  <!-- Error State -->
  {:else if currentStep === 'error'}
    <div class="step">
      <div class="icon-large error">!</div>
      <h2>Setup Failed</h2>
      <p class="error-message">{errorMessage}</p>

      <div class="button-row">
        <button class="secondary" onclick={closeOnboarding}>
          Skip for Now
        </button>
        <button class="primary" disabled={isProcessing} onclick={startSetup}>
          Try Again
        </button>
      </div>
    </div>
  {/if}
</div>

<style>
  .onboarding {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    min-height: 100vh;
    padding: 2rem;
    background: var(--bg);
    color: var(--text);
    font-family: var(--sans);
  }

  .step {
    max-width: 440px;
    width: 100%;
    text-align: center;
  }

  .icon-large {
    font-size: 3rem;
    margin-bottom: 1rem;
  }
  .icon-large.success { color: var(--success); }
  .icon-large.error { color: var(--danger); font-weight: bold; }

  h1 {
    font-size: 1.75rem;
    font-weight: 700;
    margin: 0 0 0.5rem;
    color: var(--text-bright);
  }
  h2 {
    font-size: 1.4rem;
    font-weight: 600;
    margin: 0 0 0.5rem;
    color: var(--text-bright);
  }

  .subtitle {
    color: var(--text-dim);
    font-size: 0.95rem;
    line-height: 1.5;
    margin: 0 0 1.5rem;
  }

  .features {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    margin-bottom: 1.5rem;
    text-align: left;
  }
  .feature {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 0.75rem 1rem;
    background: var(--card-bg);
    border: 1px solid var(--border);
    border-radius: 10px;
  }
  .feature-icon {
    font-size: 1.3rem;
    flex-shrink: 0;
  }
  .feature strong {
    display: block;
    font-size: 0.9rem;
    color: var(--text-bright);
  }
  .feature span {
    font-size: 0.8rem;
    color: var(--text-dim);
  }

  .note {
    font-size: 0.8rem;
    color: var(--text-dim);
    margin: 1rem 0;
    line-height: 1.5;
  }

  /* Permissions */
  .permission-list {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    margin: 1.5rem 0;
    text-align: left;
  }
  .permission-item {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 1rem;
    background: var(--card-bg);
    border-radius: 10px;
    border: 1px solid var(--border);
  }
  .permission-item.granted {
    border-color: var(--success);
  }
  .permission-status {
    font-size: 1.2rem;
    flex-shrink: 0;
    width: 1.5rem;
    text-align: center;
  }
  .check { color: var(--success); }
  .pending { color: var(--text-dim); }
  .denied { color: var(--danger); }
  .warning-icon { color: var(--warning); font-weight: bold; }
  .warning-icon-large { color: var(--warning); }
  .warning-note { color: var(--warning); }
  .restart-note { color: var(--warning); font-weight: 500; }
  .permission-info {
    flex: 1;
  }
  .permission-info strong {
    display: block;
    font-size: 0.9rem;
    color: var(--text-bright);
  }
  .permission-info span {
    font-size: 0.8rem;
    color: var(--text-dim);
  }
  .permission-note {
    font-size: 0.75rem;
    color: var(--text-dim);
    flex-shrink: 0;
  }

  /* Progress */
  .progress-container {
    margin: 2rem 0;
  }
  .progress-spinner {
    width: 48px;
    height: 48px;
    border: 3px solid var(--border);
    border-top-color: var(--accent);
    border-radius: 50%;
    margin: 0 auto 1rem;
  }
  .progress-spinner.active {
    animation: spin 1s linear infinite;
  }
  @keyframes spin {
    to { transform: rotate(360deg); }
  }
  .progress-message {
    font-size: 0.95rem;
    color: var(--text);
    margin: 0.5rem 0;
  }

  /* Shortcuts */
  .shortcuts-preview {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    margin: 1.5rem 0;
    text-align: left;
  }
  .shortcut {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.75rem 1rem;
    background: var(--card-bg);
    border: 1px solid var(--border);
    border-radius: 10px;
    flex-wrap: wrap;
  }
  kbd {
    display: inline-block;
    padding: 0.2rem 0.45rem;
    font-size: 0.75rem;
    font-family: var(--sans);
    background: var(--code-bg);
    border: 1px solid var(--border-hover);
    border-radius: 4px;
    color: var(--text);
    min-width: 1.5rem;
    text-align: center;
  }
  .shortcut > span {
    font-size: 0.8rem;
    color: var(--text-dim);
    margin-left: 0.25rem;
  }

  /* Buttons */
  button {
    cursor: pointer;
    border: none;
    border-radius: 8px;
    font-size: 0.95rem;
    font-weight: 500;
    padding: 0.7rem 1.5rem;
    transition: background 0.2s, opacity 0.2s;
  }
  button:hover { opacity: 0.9; }
  button:active { opacity: 0.8; }

  .primary {
    background: var(--accent);
    color: var(--on-accent);
    width: 100%;
    margin-top: 0.5rem;
  }
  .primary:disabled {
    cursor: not-allowed;
    opacity: 0.5;
  }
  .primary:disabled:hover {
    opacity: 0.5;
  }
  .success-note {
    color: var(--success);
  }
  .secondary {
    background: var(--card-bg);
    color: var(--text);
    box-shadow: inset 0 0 0 1px var(--border-hover);
  }
  .small {
    font-size: 0.8rem;
    padding: 0.4rem 0.8rem;
  }

  .button-row {
    display: flex;
    gap: 0.75rem;
    margin-top: 1rem;
  }
  .button-row .primary {
    flex: 1;
    width: auto;
  }

  .error-message {
    color: var(--danger);
    font-size: 0.9rem;
    padding: 1rem;
    background: var(--danger-bg);
    border-radius: 8px;
    margin: 1rem 0;
    word-break: break-word;
  }
</style>
