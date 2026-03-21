<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { onMount, onDestroy } from 'svelte';

  type Step = 'welcome' | 'permissions' | 'model' | 'ready' | 'error';

  let currentStep = $state<Step>('welcome');
  let micPermission = $state(false);
  let axPermission = $state(false);
  let backendName = $state('');
  let progressMessage = $state('');
  let progressPercent = $state(0);
  let errorMessage = $state('');
  let isProcessing = $state(false);

  // Poll interval for permission checks
  let permissionPollInterval: ReturnType<typeof setInterval> | null = null;

  // Derived: both permissions granted
  let allPermissionsGranted = $derived(micPermission && axPermission);

  onMount(async () => {
    try {
      const info = await invoke<{ backend: string; model_available: boolean }>('get_asr_backend');
      backendName = info.backend;

      if (info.model_available) {
        currentStep = 'permissions';
        startPermissionPolling();
      }
    } catch (e) {
      console.error('Failed to get backend info:', e);
    }

    await listen<{ step: string; message: string }>('setup-progress', (event) => {
      progressMessage = event.payload.message;
    });

    await listen<{ progress: number; current_file: string; status: string }>(
      'model-download-progress',
      (event) => {
        progressPercent = Math.round(event.payload.progress * 100);
        if (event.payload.current_file) {
          progressMessage = `Downloading ${event.payload.current_file}...`;
        }
        if (event.payload.status === 'complete') {
          progressMessage = 'Download complete!';
        }
      }
    );

    await listen('asr-init-complete', () => {
      currentStep = 'ready';
      isProcessing = false;
    });

    await listen<{ error: string }>('asr-init-error', (event) => {
      errorMessage = event.payload.error;
      currentStep = 'error';
      isProcessing = false;
    });
  });

  onDestroy(() => {
    stopPermissionPolling();
  });

  function startPermissionPolling() {
    checkPermissions(); // immediate check
    // Poll every 1.5s so the UI updates quickly when the user toggles in System Settings
    permissionPollInterval = setInterval(checkPermissions, 1500);
  }

  function stopPermissionPolling() {
    if (permissionPollInterval) {
      clearInterval(permissionPollInterval);
      permissionPollInterval = null;
    }
  }

  async function checkPermissions() {
    try {
      micPermission = await invoke<boolean>('check_microphone_permission');
      axPermission = await invoke<boolean>('check_accessibility_permission');
    } catch (e) {
      console.error('Permission check failed:', e);
    }
  }

  async function requestAccessibility() {
    await invoke('request_accessibility_permission');
  }

  function goToPermissions() {
    currentStep = 'permissions';
    startPermissionPolling();
  }

  async function startSetup() {
    stopPermissionPolling();
    isProcessing = true;
    currentStep = 'model';
    progressMessage = 'Initializing speech recognition...';
    progressPercent = 0;

    try {
      const result = await invoke<{
        backend: string;
        microphone_permission: boolean;
        accessibility_permission: boolean;
        asr_ready: boolean;
      }>('complete_setup');

      if (result.asr_ready) {
        currentStep = 'ready';
      } else {
        errorMessage = 'ASR engine failed to initialize. Please try again.';
        currentStep = 'error';
      }
    } catch (e: any) {
      errorMessage = e?.toString() || 'Setup failed';
      currentStep = 'error';
    } finally {
      isProcessing = false;
    }
  }

  async function closeOnboarding() {
    stopPermissionPolling();
    const win = getCurrentWindow();
    await win.close();
  }

  function retrySetup() {
    errorMessage = '';
    currentStep = 'welcome';
  }
</script>

<div class="onboarding">
  <!-- Step 1: Welcome -->
  {#if currentStep === 'welcome'}
    <div class="step">
      <div class="icon-large">🎙</div>
      <h1>Welcome to Sotto</h1>
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
      <p class="subtitle">Sotto needs two permissions to work:</p>

      <div class="permission-list">
        <div class="permission-item" class:granted={micPermission}>
          <div class="permission-status">
            {#if micPermission}
              <span class="check">✓</span>
            {:else}
              <span class="pending">○</span>
            {/if}
          </div>
          <div class="permission-info">
            <strong>Microphone</strong>
            <span>Required to capture your speech</span>
          </div>
          {#if !micPermission}
            <span class="permission-note">Will be requested on first recording</span>
          {/if}
        </div>

        <div class="permission-item" class:granted={axPermission}>
          <div class="permission-status">
            {#if axPermission}
              <span class="check">✓</span>
            {:else}
              <span class="pending">○</span>
            {/if}
          </div>
          <div class="permission-info">
            <strong>Accessibility</strong>
            <span>Required to paste text at your cursor</span>
          </div>
          {#if !axPermission}
            <button class="secondary small" onclick={requestAccessibility}>
              Open System Settings
            </button>
          {/if}
        </div>
      </div>

      {#if allPermissionsGranted}
        <p class="note success-note">
          All permissions granted — you're ready to continue!
        </p>
      {:else if !axPermission}
        <p class="note">
          Make sure Sotto is toggled <strong>on</strong> in Accessibility settings.
          Paste-at-cursor won't work without it.
        </p>
      {/if}

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

        {#if progressPercent > 0 && progressPercent < 100}
          <div class="progress-bar-container">
            <div class="progress-bar" style="width: {progressPercent}%"></div>
          </div>
          <p class="progress-detail">{progressPercent}%</p>
        {/if}
      </div>

      <p class="note">
        This downloads the Parakeet TDT v3 speech recognition model (~500 MB).
        <br />First-time setup may take 1-2 minutes depending on your internet speed.
        <br />Models are cached locally for instant startup next time.
      </p>
    </div>

  <!-- Step 4: Ready -->
  {:else if currentStep === 'ready'}
    <div class="step">
      <div class="icon-large success">✓</div>
      <h2>Sotto is Ready!</h2>
      <p class="subtitle">Speech recognition is set up and ready to use.</p>

      <div class="shortcuts-preview">
        <div class="shortcut">
          <kbd>⌘</kbd><kbd>⇧</kbd><kbd>Space</kbd>
          <span>Push-to-talk — hold to record, release to transcribe</span>
        </div>
        <div class="shortcut">
          <kbd>⌘</kbd><kbd>⇧</kbd><kbd>D</kbd>
          <span>Toggle — press to start, press again to stop</span>
        </div>
        <div class="shortcut">
          <kbd>Esc</kbd>
          <span>Cancel current recording</span>
        </div>
      </div>

      <p class="note">
        Sotto lives in your menu bar. Look for the microphone icon at the top of your screen.
      </p>

      <button class="primary" onclick={closeOnboarding}>
        Start Using Sotto
      </button>
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
        <button class="primary" onclick={retrySetup}>
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
    background: var(--bg, #1a1a1a);
    color: var(--text, #e0e0e0);
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', sans-serif;
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
  .icon-large.success { color: #22c55e; }
  .icon-large.error { color: #ef4444; font-weight: bold; }

  h1 {
    font-size: 1.75rem;
    font-weight: 700;
    margin: 0 0 0.5rem;
    color: #fff;
  }
  h2 {
    font-size: 1.4rem;
    font-weight: 600;
    margin: 0 0 0.5rem;
    color: #fff;
  }

  .subtitle {
    color: #999;
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
    background: var(--card-bg, #242424);
    border-radius: 10px;
  }
  .feature-icon {
    font-size: 1.3rem;
    flex-shrink: 0;
  }
  .feature strong {
    display: block;
    font-size: 0.9rem;
    color: #fff;
  }
  .feature span {
    font-size: 0.8rem;
    color: #888;
  }

  .note {
    font-size: 0.8rem;
    color: #666;
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
    background: var(--card-bg, #242424);
    border-radius: 10px;
    border: 1px solid #333;
  }
  .permission-item.granted {
    border-color: #22c55e44;
  }
  .permission-status {
    font-size: 1.2rem;
    flex-shrink: 0;
    width: 1.5rem;
    text-align: center;
  }
  .check { color: #22c55e; }
  .pending { color: #666; }
  .permission-info {
    flex: 1;
  }
  .permission-info strong {
    display: block;
    font-size: 0.9rem;
    color: #fff;
  }
  .permission-info span {
    font-size: 0.8rem;
    color: #888;
  }
  .permission-note {
    font-size: 0.75rem;
    color: #666;
    flex-shrink: 0;
  }

  /* Progress */
  .progress-container {
    margin: 2rem 0;
  }
  .progress-spinner {
    width: 48px;
    height: 48px;
    border: 3px solid #333;
    border-top-color: var(--accent, #3b82f6);
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
    color: #ccc;
    margin: 0.5rem 0;
  }
  .progress-bar-container {
    width: 100%;
    height: 6px;
    background: #333;
    border-radius: 3px;
    margin: 1rem 0 0.5rem;
    overflow: hidden;
  }
  .progress-bar {
    height: 100%;
    background: var(--accent, #3b82f6);
    border-radius: 3px;
    transition: width 0.3s ease;
  }
  .progress-detail {
    font-size: 0.85rem;
    color: #888;
    margin: 0;
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
    background: var(--card-bg, #242424);
    border-radius: 10px;
    flex-wrap: wrap;
  }
  kbd {
    display: inline-block;
    padding: 0.2rem 0.45rem;
    font-size: 0.75rem;
    font-family: -apple-system, monospace;
    background: #333;
    border: 1px solid #444;
    border-radius: 4px;
    color: #ccc;
    min-width: 1.5rem;
    text-align: center;
  }
  .shortcut > span {
    font-size: 0.8rem;
    color: #888;
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
    background: var(--accent, #3b82f6);
    color: #fff;
    width: 100%;
    margin-top: 0.5rem;
  }
  .primary:disabled {
    background: #333;
    color: #666;
    cursor: not-allowed;
    opacity: 0.6;
  }
  .primary:disabled:hover {
    opacity: 0.6;
  }
  .success-note {
    color: #22c55e;
  }
  .skip-link {
    background: none;
    color: #666;
    font-size: 0.8rem;
    text-decoration: underline;
    margin-top: 1rem;
    padding: 0.5rem;
    width: 100%;
  }
  .skip-link:hover {
    color: #999;
  }
  .secondary {
    background: #333;
    color: #ccc;
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
    color: #ef4444;
    font-size: 0.9rem;
    padding: 1rem;
    background: #ef444410;
    border-radius: 8px;
    border: 1px solid #ef444433;
    margin: 1rem 0;
    word-break: break-word;
  }
</style>
