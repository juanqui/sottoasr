<script lang="ts">
  import { getVersion } from '@tauri-apps/api/app';
  import { createEventScope } from '../utils/event-scope';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { onMount, onDestroy } from 'svelte';
  import { getUpdateStatus, checkAppUpdate, performAppUpdate, restartApp } from '../utils/tauri';
  import type { UpdateDownloadProgress } from '../utils/tauri';

  type UpdateStep = 'checking' | 'up_to_date' | 'available' | 'downloading' | 'ready' | 'error';

  let step = $state<UpdateStep>('checking');
  let currentVersion = $state('');
  let availableVersion = $state('');
  let releaseNotes = $state('');
  let downloadProgress = $state(0);
  let downloadedMB = $state('0');
  let totalMB = $state('');
  let errorMessage = $state('');
  let restarting = $state(false);
  let autoCloseTimer: ReturnType<typeof setTimeout> | null = null;
  let autoCloseSeconds = $state(4);
  let checkTimeoutId: ReturnType<typeof setTimeout> | null = null;
  let downloadStallTimer: ReturnType<typeof setInterval> | null = null;
  let lastProgressTime = 0;

  let disposed = false;
  const scope = createEventScope((error) => { errorMessage = String(error); step = 'error'; });

  onMount(() => { void initialize().catch((error) => { if (!disposed) { errorMessage = String(error); step = 'error'; } }); });

  async function initialize() {
    currentVersion = await getVersion();
    if (disposed) return;

    // Register event listeners before triggering check. Background checks
    // emit the same three check events, so they only answer a check this
    // window is waiting for.
    await scope.listen<string>('update-available', async () => {
      if (step !== 'checking') return;
      const status = await getUpdateStatus();
      if (disposed || step !== 'checking') return;
      availableVersion = status.version ?? '';
      releaseNotes = status.release_notes ?? '';
      step = 'available';
    });

    await scope.listen('update-up-to-date', () => {
      if (step !== 'checking') return;
      step = 'up_to_date';
      startAutoClose();
    });

    await scope.listen<string>('update-check-error', (event) => {
      if (step !== 'checking') return;
      errorMessage = event.payload;
      step = 'error';
    });

    await scope.listen<UpdateDownloadProgress>('update-download-progress', (event) => {
      const p = event.payload;
      lastProgressTime = Date.now();
      downloadProgress = Math.round(p.progress * 100);
      downloadedMB = (p.downloaded_bytes / 1_048_576).toFixed(1);
      if (p.total_bytes) {
        totalMB = (p.total_bytes / 1_048_576).toFixed(1);
      }
    });

    // An install started from this window or elsewhere has finished.
    await scope.listen<{ version: string }>('update-installed', (event) => {
      clearDownloadStallTimer();
      clearAutoClose();
      if (event.payload.version) availableVersion = event.payload.version;
      errorMessage = '';
      step = 'ready';
    });

    // Determine initial state from existing update status.
    try {
      const status = await getUpdateStatus();
      if (disposed) return;
      if (status.restart_pending) {
        availableVersion = status.version ?? '';
        step = 'ready';
      } else if (status.downloading) {
        availableVersion = status.version ?? '';
        watchDownload();
      } else if (status.update_available && status.version) {
        availableVersion = status.version;
        releaseNotes = status.release_notes ?? '';
        step = 'available';
      } else {
        doCheck();
      }
    } catch {
      if (disposed) return;
      doCheck();
    }
  }

  onDestroy(() => {
    disposed = true;
    clearAutoClose();
    clearCheckTimeout();
    clearDownloadStallTimer();
    scope.dispose();
  });

  async function doCheck() {
    step = 'checking';
    errorMessage = '';

    // Safety-net: if the Rust side hangs beyond its own 30 s timeout,
    // the frontend still recovers after 35 s.  Tracked at component scope
    // so onDestroy can clean it up if the window closes mid-check.
    clearCheckTimeout();
    checkTimeoutId = setTimeout(() => {
      if (step === 'checking') {
        errorMessage = 'Update check timed out. Please try again.';
        step = 'error';
      }
    }, 35_000);

    try {
      const version = await checkAppUpdate();
      if (disposed) return;
      if (version) {
        // Transition directly — don't rely solely on the event listener.
        // Guard: skip if the event listener already transitioned us.
        if (step === 'checking') {
          const status = await getUpdateStatus();
          if (disposed || step !== 'checking') return;
          availableVersion = status.version ?? version;
          releaseNotes = status.release_notes ?? '';
          step = 'available';
        }
      } else {
        if (step === 'checking') {
          step = 'up_to_date';
          startAutoClose();
        }
      }
    } catch (err) {
      if (disposed || step !== 'checking') return;
      errorMessage = String(err) || 'Check failed';
      step = 'error';
    } finally {
      clearCheckTimeout();
    }
  }

  /** Show download progress and fail the step if no progress arrives for 60 s. */
  function watchDownload() {
    step = 'downloading';
    downloadProgress = 0;
    downloadedMB = '0';
    totalMB = '';
    errorMessage = '';
    lastProgressTime = Date.now();
    clearDownloadStallTimer();
    downloadStallTimer = setInterval(() => {
      if (step === 'downloading' && Date.now() - lastProgressTime > 60_000) {
        clearDownloadStallTimer();
        errorMessage = 'Download appears to have stalled. Please try again.';
        step = 'error';
      }
    }, 10_000);
  }

  async function handleDownload() {
    watchDownload();
    try {
      const version = await performAppUpdate();
      if (disposed) return;
      if (version) availableVersion = version;
      step = 'ready';
    } catch (err) {
      if (disposed) return;
      errorMessage = String(err) || 'Update failed';
      step = 'error';
    } finally {
      clearDownloadStallTimer();
    }
  }

  function clearCheckTimeout() {
    if (checkTimeoutId) {
      clearTimeout(checkTimeoutId);
      checkTimeoutId = null;
    }
  }

  function clearDownloadStallTimer() {
    if (downloadStallTimer) {
      clearInterval(downloadStallTimer);
      downloadStallTimer = null;
    }
  }

  async function handleRestart() {
    if (restarting) return;
    restarting = true;
    errorMessage = '';
    try {
      await restartApp();
    } catch (error) { if (!disposed) errorMessage = String(error); }
    finally { if (!disposed) restarting = false; }
  }

  function handleLater() {
    getCurrentWindow().close();
  }

  function startAutoClose() {
    if (disposed || autoCloseTimer) return; // Never schedule work for a closed window.
    autoCloseSeconds = 4;
    autoCloseTimer = setInterval(() => {
      autoCloseSeconds -= 1;
      if (autoCloseSeconds <= 0) {
        clearAutoClose();
        getCurrentWindow().close();
      }
    }, 1000);
  }

  function clearAutoClose() {
    if (autoCloseTimer) {
      clearInterval(autoCloseTimer);
      autoCloseTimer = null;
    }
  }
</script>

<div class="update-window">
  <!-- Checking -->
  {#if step === 'checking'}
    <div class="step">
      <div class="spinner"></div>
      <h2>Checking for Updates</h2>
      <p class="subtitle">Contacting update server...</p>
    </div>

  <!-- Up to date -->
  {:else if step === 'up_to_date'}
    <div class="step">
      <div class="icon-circle success">
        <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
          <polyline points="20 6 9 17 4 12"></polyline>
        </svg>
      </div>
      <h2>SottoASR is Up to Date</h2>
      <p class="subtitle">Version {currentVersion}</p>
      <p class="auto-close">Closing in {autoCloseSeconds}s</p>
    </div>

  <!-- Update available -->
  {:else if step === 'available'}
    <div class="step">
      <div class="icon-circle available">
        <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
          <line x1="12" y1="5" x2="12" y2="19"></line>
          <polyline points="19 12 12 19 5 12"></polyline>
        </svg>
      </div>
      <h2>Update Available</h2>
      <p class="subtitle">
        Version {availableVersion} is ready to download.
        <span class="current-hint">You have v{currentVersion}.</span>
      </p>

      {#if releaseNotes}
        <div class="release-notes">
          <p class="release-notes-label">What's new</p>
          <div class="release-notes-content">{releaseNotes}</div>
        </div>
      {/if}

      <div class="button-row">
        <button class="secondary" onclick={handleLater}>Later</button>
        <button class="primary" onclick={handleDownload}>Download & Install</button>
      </div>
    </div>

  <!-- Downloading -->
  {:else if step === 'downloading'}
    <div class="step">
      <h2>Downloading Update</h2>
      {#if availableVersion}<p class="subtitle">v{availableVersion}</p>{/if}

      <div class="progress-section">
        <div class="progress-bar-container">
          {#if downloadProgress > 0}
            <div class="progress-bar" style="width: {downloadProgress}%"></div>
          {:else}
            <div class="progress-bar indeterminate"></div>
          {/if}
        </div>
        <div class="progress-info">
          {#if downloadProgress > 0}
            <span>{downloadProgress}%</span>
          {/if}
          {#if totalMB}
            <span class="progress-bytes">{downloadedMB} / {totalMB} MB</span>
          {:else if Number(downloadedMB) > 0}
            <span class="progress-bytes">{downloadedMB} MB</span>
          {/if}
        </div>
      </div>
    </div>

  <!-- Ready to restart -->
  {:else if step === 'ready'}
    <div class="step">
      <div class="icon-circle success">
        <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
          <polyline points="20 6 9 17 4 12"></polyline>
        </svg>
      </div>
      <h2>Update Ready</h2>
      <p class="subtitle">{availableVersion ? `SottoASR v${availableVersion}` : 'The update'} has been downloaded. Restart to apply.</p>
      {#if errorMessage}<p class="error-message" role="alert">{errorMessage}</p>{/if}

      <div class="button-row">
        <button class="secondary" onclick={handleLater}>Later</button>
        <button class="primary" disabled={restarting} onclick={handleRestart}>Restart Now</button>
      </div>
    </div>

  <!-- Error -->
  {:else if step === 'error'}
    <div class="step">
      <div class="icon-circle error-icon">
        <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
          <line x1="18" y1="6" x2="6" y2="18"></line>
          <line x1="6" y1="6" x2="18" y2="18"></line>
        </svg>
      </div>
      <h2>Update Failed</h2>
      <p class="error-message">{errorMessage}</p>

      <div class="button-row">
        <button class="secondary" onclick={handleLater}>Close</button>
        <button class="primary" onclick={doCheck}>Try Again</button>
      </div>
    </div>
  {/if}
</div>

<style>
  .update-window {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    min-height: 100vh;
    padding: 2rem;
    background: var(--bg);
    color: var(--text);
    font-family: var(--sans);
    user-select: none;
  }

  .step {
    max-width: 360px;
    width: 100%;
    text-align: center;
  }

  h2 {
    font-size: 1.25rem;
    font-weight: 600;
    margin: 0 0 0.4rem;
    color: var(--text-bright);
  }

  .subtitle {
    color: var(--text-dim);
    font-size: 0.85rem;
    line-height: 1.5;
    margin: 0 0 1.25rem;
  }

  .current-hint {
    color: var(--text-dim);
  }

  .auto-close {
    font-size: 0.75rem;
    color: var(--text-dim);
    margin: 0;
  }

  /* ---- Icons ---- */
  .icon-circle {
    width: 56px;
    height: 56px;
    border-radius: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
    margin: 0 auto 1rem;
  }
  .icon-circle.success {
    background: var(--success-bg);
    color: var(--success);
  }
  .icon-circle.available {
    background: var(--accent-bg);
    color: var(--accent);
  }
  .icon-circle.error-icon {
    background: var(--danger-bg);
    color: var(--danger);
  }

  /* ---- Spinner ---- */
  .spinner {
    width: 48px;
    height: 48px;
    border: 3px solid var(--border);
    border-top-color: var(--accent);
    border-radius: 50%;
    margin: 0 auto 1rem;
    animation: spin 1s linear infinite;
  }
  @keyframes spin {
    to { transform: rotate(360deg); }
  }

  /* ---- Progress bar ---- */
  .progress-section {
    margin: 1.5rem 0;
  }
  .progress-bar-container {
    width: 100%;
    height: 6px;
    background: var(--border);
    border-radius: 3px;
    overflow: hidden;
  }
  .progress-bar {
    height: 100%;
    background: var(--accent);
    border-radius: 3px;
    transition: width 0.3s ease;
  }
  .progress-bar.indeterminate {
    width: 40%;
    animation: indeterminate 1.4s ease-in-out infinite;
  }
  @keyframes indeterminate {
    0% { transform: translateX(-100%); }
    100% { transform: translateX(350%); }
  }
  .progress-info {
    display: flex;
    justify-content: space-between;
    font-size: 0.8rem;
    color: var(--text-dim);
    margin-top: 0.5rem;
  }
  .progress-bytes {
    color: var(--text-dim);
  }

  /* ---- Release notes ---- */
  .release-notes {
    text-align: left;
    margin: 0 0 1.25rem;
    padding: 0.75rem;
    background: var(--card-bg);
    border-radius: 8px;
    border: 1px solid var(--border);
  }
  .release-notes-label {
    font-size: 0.7rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-dim);
    margin: 0 0 0.4rem;
  }
  .release-notes-content {
    font-size: 0.8rem;
    color: var(--text);
    line-height: 1.5;
    max-height: 100px;
    overflow-y: auto;
    white-space: pre-wrap;
  }

  /* ---- Error ---- */
  .error-message {
    color: var(--danger);
    font-size: 0.85rem;
    padding: 0.75rem;
    background: var(--danger-bg);
    border-radius: 8px;
    margin: 0 0 1.25rem;
    word-break: break-word;
    text-align: left;
  }

  /* ---- Buttons ---- */
  button {
    cursor: pointer;
    border: none;
    border-radius: 8px;
    font-size: 0.9rem;
    font-weight: 500;
    font-family: inherit;
    padding: 0.6rem 1.25rem;
    transition: background 0.2s, opacity 0.2s;
  }
  button:hover { opacity: 0.9; }
  button:active { opacity: 0.8; }

  .primary {
    background: var(--accent);
    color: var(--on-accent);
  }
  .secondary {
    background: var(--card-bg);
    color: var(--text);
    box-shadow: inset 0 0 0 1px var(--border-hover);
  }

  .button-row {
    display: flex;
    gap: 0.75rem;
    margin-top: 0.5rem;
  }
  .button-row .primary {
    flex: 1;
  }
</style>
