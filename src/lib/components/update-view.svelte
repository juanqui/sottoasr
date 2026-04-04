<script lang="ts">
  import { getVersion } from '@tauri-apps/api/app';
  import { listen } from '@tauri-apps/api/event';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { onMount, onDestroy } from 'svelte';
  import { getUpdateStatus, checkAppUpdate, performAppUpdate } from '../utils/tauri';
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
  let autoCloseTimer: ReturnType<typeof setTimeout> | null = null;
  let autoCloseSeconds = $state(4);
  let checkTimeoutId: ReturnType<typeof setTimeout> | null = null;
  let downloadStallTimer: ReturnType<typeof setInterval> | null = null;
  let lastProgressTime = 0;

  let unlisteners: Array<() => void> = [];

  onMount(async () => {
    currentVersion = await getVersion();

    // Register event listeners before triggering check.
    unlisteners.push(await listen<string>('update-available', async () => {
      const status = await getUpdateStatus();
      availableVersion = status.version ?? '';
      releaseNotes = status.release_notes ?? '';
      step = 'available';
    }));

    unlisteners.push(await listen('update-up-to-date', () => {
      step = 'up_to_date';
      startAutoClose();
    }));

    unlisteners.push(await listen<UpdateDownloadProgress>('update-download-progress', (event) => {
      const p = event.payload;
      lastProgressTime = Date.now();
      downloadProgress = Math.round(p.progress * 100);
      downloadedMB = (p.downloaded_bytes / 1_048_576).toFixed(1);
      if (p.total_bytes) {
        totalMB = (p.total_bytes / 1_048_576).toFixed(1);
      }
    }));

    unlisteners.push(await listen<string>('update-check-error', (event) => {
      errorMessage = event.payload;
      step = 'error';
    }));

    // Determine initial state from existing update status.
    try {
      const status = await getUpdateStatus();
      if (status.restart_pending) {
        step = 'ready';
      } else if (status.downloading) {
        step = 'downloading';
      } else if (status.update_available && status.version) {
        availableVersion = status.version;
        releaseNotes = status.release_notes ?? '';
        step = 'available';
      } else {
        step = 'checking';
        doCheck();
      }
    } catch {
      step = 'checking';
      doCheck();
    }
  });

  onDestroy(() => {
    clearAutoClose();
    clearCheckTimeout();
    clearDownloadStallTimer();
    unlisteners.forEach(fn => fn());
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
      if (version) {
        // Transition directly — don't rely solely on the event listener.
        // Guard: skip if the event listener already transitioned us.
        if (step === 'checking') {
          const status = await getUpdateStatus();
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
    } catch (err: any) {
      errorMessage = err?.toString() || 'Check failed';
      step = 'error';
    } finally {
      clearCheckTimeout();
    }
  }

  async function handleDownload() {
    step = 'downloading';
    downloadProgress = 0;
    downloadedMB = '0';
    totalMB = '';
    errorMessage = '';
    lastProgressTime = Date.now();

    // Detect stalled downloads — if no progress events for 60 s, abort.
    clearDownloadStallTimer();
    downloadStallTimer = setInterval(() => {
      if (step === 'downloading' && Date.now() - lastProgressTime > 60_000) {
        clearDownloadStallTimer();
        errorMessage = 'Download appears to have stalled. Please try again.';
        step = 'error';
      }
    }, 10_000);

    try {
      await performAppUpdate();
      step = 'ready';
    } catch (err: any) {
      errorMessage = err?.toString() || 'Update failed';
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
    try {
      const { relaunch } = await import('@tauri-apps/plugin-process');
      await relaunch();
    } catch {
      // Fallback — should not happen.
    }
  }

  function handleLater() {
    getCurrentWindow().close();
  }

  function startAutoClose() {
    if (autoCloseTimer) return; // Prevent duplicate intervals.
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
      <p class="subtitle">v{availableVersion}</p>

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
      <p class="subtitle">SottoASR v{availableVersion || 'latest'} has been downloaded. Restart to apply.</p>

      <div class="button-row">
        <button class="secondary" onclick={handleLater}>Later</button>
        <button class="primary" onclick={handleRestart}>Restart Now</button>
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
    background: var(--bg, #1a1a1a);
    color: var(--text, #e0e0e0);
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', sans-serif;
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
    color: #fff;
  }

  .subtitle {
    color: #999;
    font-size: 0.85rem;
    line-height: 1.5;
    margin: 0 0 1.25rem;
  }

  .current-hint {
    color: #666;
  }

  .auto-close {
    font-size: 0.75rem;
    color: #555;
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
    background: rgba(34, 197, 94, 0.12);
    color: #22c55e;
  }
  .icon-circle.available {
    background: rgba(59, 130, 246, 0.12);
    color: #3b82f6;
  }
  .icon-circle.error-icon {
    background: rgba(239, 68, 68, 0.12);
    color: #ef4444;
  }

  /* ---- Spinner ---- */
  .spinner {
    width: 48px;
    height: 48px;
    border: 3px solid #333;
    border-top-color: var(--accent, #3b82f6);
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
    background: #333;
    border-radius: 3px;
    overflow: hidden;
  }
  .progress-bar {
    height: 100%;
    background: var(--accent, #3b82f6);
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
    color: #888;
    margin-top: 0.5rem;
  }
  .progress-bytes {
    color: #666;
  }

  /* ---- Release notes ---- */
  .release-notes {
    text-align: left;
    margin: 0 0 1.25rem;
    padding: 0.75rem;
    background: var(--card-bg, #242424);
    border-radius: 8px;
    border: 1px solid #333;
  }
  .release-notes-label {
    font-size: 0.7rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: #666;
    margin: 0 0 0.4rem;
  }
  .release-notes-content {
    font-size: 0.8rem;
    color: #aaa;
    line-height: 1.5;
    max-height: 100px;
    overflow-y: auto;
    white-space: pre-wrap;
  }

  /* ---- Error ---- */
  .error-message {
    color: #ef4444;
    font-size: 0.85rem;
    padding: 0.75rem;
    background: rgba(239, 68, 68, 0.06);
    border-radius: 8px;
    border: 1px solid rgba(239, 68, 68, 0.2);
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
    background: var(--accent, #3b82f6);
    color: #fff;
  }
  .secondary {
    background: #333;
    color: #ccc;
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
