<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { createEventScope } from '../utils/event-scope';
  import { onMount, tick } from 'svelte';
  import { MediaQuery } from 'svelte/reactivity';
  import { fade } from 'svelte/transition';
  import Waveform from './waveform.svelte';
  import RecordingTimer from './recording-timer.svelte';
  import { getOverlaySnapshot } from '../utils/tauri';
  import type { DictationError, LlmCleanupStatus, OverlaySnapshot } from '../utils/tauri';
  const reducedMotion = new MediaQuery('(prefers-reduced-motion: reduce)');

  async function handleStop() {
    try {
      await invoke('stop_recording');
    } catch (e) {
      console.error('Stop failed:', e);
    }
  }

  async function handleCancel() {
    try {
      await invoke('cancel_recording');
    } catch (e) {
      console.error('Cancel failed:', e);
    }
  }

  // The overlay is an NSPanel with can_become_key_window: false, so wry's
  // `-webkit-app-region: drag` heuristic does not fire on it. Instead we
  // trigger a native drag from a mousedown on the pill background.
  function handlePillMouseDown(e: MouseEvent) {
    if (e.button !== 0) return;
    // Buttons stopPropagation their own mousedown — so if we got here, the
    // user clicked the pill background or a non-interactive child.
    invoke('overlay_start_drag').catch((err) => {
      console.error('overlay_start_drag failed:', err);
    });
  }

  function stopMouseDown(e: MouseEvent) {
    // Prevent the pill-level drag handler from firing when the user
    // clicks an interactive control (Stop / Cancel).
    e.stopPropagation();
  }

  // Initialize as false so the timer doesn't start at precreation time.
  // The state-changed:Recording event will flip this to true when recording actually starts,
  // triggering the RecordingTimer's $effect to capture the correct start time.
  let isRecording = $state(false);
  let isTranscribing = $state(false);
  let isCleaningUp = $state(false);
  let isPasting = $state(false);
  let showSlowMessage = $state(false);
  let cleanupTimer: ReturnType<typeof setTimeout> | null = null;
  let startTime = $state<number>(0);
  // Cleanup outcome — set when the llm-cleanup-status event arrives, then
  // displayed as a badge for the brief window before the overlay hides.
  // Reset on the next state-changed:Recording so we don't carry stale state.
  let cleanupStatus = $state<LlmCleanupStatus | null>(null);

  // Keep only the latest sample; the waveform owns its fixed-size history.
  let audioLevel = $state(0);
  let audioSequence = $state(0);

  // Duration cap and warning. Mirrors MAX_RECORDING_SECS in Rust
  // (src-tauri/src/hotkeys/manager.rs and src-tauri/src/pipeline.rs).
  const MAX_DURATION_MS = 20 * 60 * 1000;
  let showWarning = $state(false);
  let remainingSeconds = $state(60);
  let countdownInterval: ReturnType<typeof setInterval> | null = null;

  let countdownDisplay = $derived(
    `${Math.floor(remainingSeconds / 60)}:${(remainingSeconds % 60).toString().padStart(2, '0')}`
  );

  function startCountdown() {
    showWarning = true;
    updateRemaining();
    if (countdownInterval) clearInterval(countdownInterval);
    countdownInterval = setInterval(updateRemaining, 250);
  }

  function updateRemaining() {
    const elapsed = Date.now() - startTime;
    remainingSeconds = Math.max(0, Math.ceil((MAX_DURATION_MS - elapsed) / 1000));
  }

  function clearWarning() {
    showWarning = false;
    remainingSeconds = 60;
    if (countdownInterval) {
      clearInterval(countdownInterval);
      countdownInterval = null;
    }
  }

  type FailureKind = 'recording' | 'transcription' | 'paste' | 'history';
  const failureTitles: Record<FailureKind, string> = {
    recording: 'Recording unavailable', transcription: 'Transcription failed',
    paste: 'Paste failed', history: 'History not saved',
  };
  let failure = $state<(DictationError & { kind: FailureKind }) | null>(null);
  let errorCard = $state<HTMLDivElement>();
  let isActive = $derived(isRecording || isTranscribing || isCleaningUp || isPasting || failure !== null);
  const canDismissFailure = $derived(!isRecording && !isTranscribing && !isCleaningUp && !isPasting);
  let recordingGeneration: number | null = null;
  let overlayRevision = -1;
  let errorAction = $state('');
  let dismissing = $state(false);
  const failureTitle = $derived(failure ? failureTitles[failure.kind] : '');

  function receiveFailure(kind: FailureKind, payload: DictationError) {
    if (payload.generation !== undefined && recordingGeneration !== null && payload.generation !== recordingGeneration) return;
    failure = { ...payload, kind };
    errorAction = '';
    if (cleanupTimer) { clearTimeout(cleanupTimer); cleanupTimer = null; }
    clearWarning();
  }

  async function errorCommand(command: string, args?: Record<string, unknown>) {
    const currentFailure = failure;
    errorAction = '';
    try { await invoke(command, args); }
    catch (error) { if (failure === currentFailure) errorAction = String(error); }
  }

  async function dismissError() {
    if (dismissing) return;
    const currentFailure = failure;
    dismissing = true;
    errorAction = '';
    try { await invoke('dismiss_overlay_error', { revision: overlayRevision }); if (failure === currentFailure) failure = null; }
    catch (error) { if (failure === currentFailure) errorAction = String(error); }
    finally { dismissing = false; }
  }

  $effect(() => {
    if (!failure || !canDismissFailure) return;
    const currentFailure = failure;
    void tick().then(() => {
      if (failure === currentFailure) errorCard?.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus();
    });
  });

  let waveformRef = $state<Waveform>();

  function applySnapshot(snapshot: OverlaySnapshot) {
    if (snapshot.revision <= overlayRevision) return;
    overlayRevision = snapshot.revision;
    const newRecording = snapshot.state === 'Recording' &&
      (recordingGeneration !== snapshot.generation || !isRecording);
    recordingGeneration = snapshot.generation;
    isPasting = snapshot.state === 'Pasting';

    if (snapshot.error) {
      const kinds: Record<string, FailureKind> = {
        'recording-error': 'recording', 'transcription-error': 'transcription',
        'paste-error': 'paste', 'history-save-error': 'history',
      };
      receiveFailure(kinds[snapshot.error.event] ?? 'transcription', snapshot.error.payload);
    } else { failure = null; errorAction = ''; }

    if (snapshot.state === 'Recording') {
      isRecording = true; isTranscribing = false; isCleaningUp = false;
      startTime = snapshot.started_at_ms ?? Date.now();
      if (newRecording) {
        showSlowMessage = false; cleanupStatus = null;
        if (cleanupTimer) { clearTimeout(cleanupTimer); cleanupTimer = null; }
        audioLevel = 0; audioSequence = 0; waveformRef?.reset(); clearWarning();
        if (Date.now() - startTime >= MAX_DURATION_MS - 60_000) startCountdown();
      }
    } else if (snapshot.state === 'CleaningUp') {
      const startedCleanup = !isCleaningUp;
      isRecording = false; isTranscribing = false; isCleaningUp = true;
      clearWarning();
      if (startedCleanup) {
        showSlowMessage = false; cleanupStatus = null;
        cleanupTimer = setTimeout(() => { showSlowMessage = true; }, 5000);
      }
    } else {
      isRecording = false; isTranscribing = snapshot.state === 'Transcribing'; isCleaningUp = false;
      showSlowMessage = false;
      if (cleanupTimer) { clearTimeout(cleanupTimer); cleanupTimer = null; }
      clearWarning();
    }
  }

  /// Compute label and visual variant for the current cleanup status badge.
  /// Returns null when no badge should be shown (Disabled, SkippedTooShort, Idle).
  function badgeFor(status: LlmCleanupStatus | null): { label: string; variant: 'success' | 'warn' } | null {
    if (!status) return null;
    switch (status.kind) {
      case 'applied':
        return { label: 'Cleaned', variant: 'success' };
      case 'suggested':
        return { label: 'Suggestion in History', variant: 'success' };
      case 'unavailable':
        return { label: 'Cleanup unavailable', variant: 'warn' };
      case 'failed':
        return { label: 'Cleanup failed', variant: 'warn' };
      case 'timed_out':
        return { label: 'Cleanup timed out', variant: 'warn' };
      // SkippedTooShort, Disabled, Idle — no badge
      default:
        return null;
    }
  }
  let badge = $derived(badgeFor(cleanupStatus));

  onMount(() => {
    let disposed = false;
    const scope = createEventScope();
    const dismissWithEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && failure && canDismissFailure) {
        event.preventDefault();
        void dismissError();
      }
    };
    window.addEventListener('keydown', dismissWithEscape);

    void scope.listen<OverlaySnapshot>('overlay-state', (event) => applySnapshot(event.payload))
      .then(async () => {
        if (disposed) return;
        try { const snapshot = await getOverlaySnapshot(); if (!disposed) applySnapshot(snapshot); }
        catch (error) { if (!disposed) console.error('Could not load overlay state:', error); }
      });

    // Audio level events from Rust: { level: f32 } emitted ~30 times/sec
    scope.listen<{ level: number }>('audio-level', (event) => {
      if (!isRecording) return;
      const level = event.payload.level;
      audioLevel = Number.isFinite(level) ? Math.max(0, level) : 0;
      audioSequence += 1;
    });

    scope.listen('recording-time-warning', () => { if (isRecording) startCountdown(); });

    // Cleanup outcome arrives from Rust right after run_cleanup() finishes,
    // BEFORE the overlay hides. We replace the "Cleaning up..." spinner with
    // a brief badge. Rust holds the overlay open for a short window
    // (badge_dwell_ms) before it tells us to hide.
    scope.listen<LlmCleanupStatus>('llm-cleanup-status', (event) => {
      cleanupStatus = event.payload;
      // Once a status arrives, the slow-message timer is no longer relevant.
      if (cleanupTimer) { clearTimeout(cleanupTimer); cleanupTimer = null; }
      showSlowMessage = false;
    });

    return () => {
      disposed = true;
      scope.dispose();
      window.removeEventListener('keydown', dismissWithEscape);
      if (cleanupTimer) clearTimeout(cleanupTimer);
      clearWarning();
    };
  });
</script>

<div class="pill-container" class:visible={isActive}>
  {#if failure}
    <div bind:this={errorCard} class="error-card" role="alert">
      <strong>{failureTitle}</strong>
      <p title={errorAction || failure.error}>{errorAction || failure.error}</p>
      <div class="error-actions">
        {#if failure.audio_path}<button type="button" onclick={() => errorCommand('reveal_recording_audio', {path: failure!.audio_path})}>Show audio</button>
        {:else if failure.kind === 'paste'}<button type="button" onclick={() => errorCommand('open_transcription_history')}>History</button>{/if}
        {#if failure.clipboard_available}<span>Text is on the clipboard</span>{/if}
        <button class="dismiss-error" type="button" disabled={dismissing || !canDismissFailure} onclick={dismissError}>Dismiss</button>
      </div>
    </div>
  {:else}
  {#if showWarning}
    <div class="warning-banner" in:fade={{ duration: reducedMotion.current ? 0 : 200 }}>
      <span class="warning-text">Recording stops in {countdownDisplay}</span>
    </div>
  {/if}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="pill"
    class:transcribing={isTranscribing || isCleaningUp}
    class:warning={showWarning}
    onmousedown={handlePillMouseDown}
  >
    <!-- Recording indicator dot -->
    <div class="indicator">
      {#if isRecording}
        <div class="dot recording-dot"></div>
      {:else if isTranscribing || isCleaningUp}
        <div class="spinner"></div>
      {/if}
    </div>

    {#if isCleaningUp}
      <!-- Cleaning up label / cleanup result badge -->
      <div class="status-label">
        {#if badge}
          <span class="status-text badge-text" class:badge-success={badge.variant === 'success'} class:badge-warn={badge.variant === 'warn'}>
            {#if badge.variant === 'success'}
              <svg class="badge-icon" width="12" height="12" viewBox="0 0 12 12" fill="none">
                <path d="M2 6.5 L5 9.5 L10 3" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/>
              </svg>
            {:else}
              <svg class="badge-icon" width="12" height="12" viewBox="0 0 12 12" fill="none">
                <path d="M6 2 L11 10.5 H1 Z" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round"/>
                <path d="M6 5 V7.5" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/>
                <circle cx="6" cy="9" r="0.6" fill="currentColor"/>
              </svg>
            {/if}
            {badge.label}
          </span>
        {:else if showSlowMessage}
          <span class="status-text">Taking a bit longer<br/>than usual, please wait</span>
        {:else}
          <span class="status-text">Preparing suggestion...</span>
        {/if}
      </div>
    {:else if isTranscribing || isPasting}
      <div class="status-label"><span class="status-text">{isPasting ? 'Pasting...' : 'Transcribing...'}</span></div>
    {:else}
      <!-- Waveform bars -->
      <div class="waveform-area">
        <Waveform bind:this={waveformRef} level={audioLevel} sampleId={audioSequence} active={isRecording} />
      </div>

      <!-- Timer -->
      <RecordingTimer running={isRecording} startedAt={startTime} />
    {/if}

    <!-- Stop (transcribe) and Cancel buttons -->
    {#if isRecording}
      <button
        class="stop-btn"
        onclick={handleStop}
        onmousedown={stopMouseDown}
        type="button"
        aria-label="Stop and transcribe"
      >
        <svg width="10" height="8" viewBox="0 0 10 8" fill="none">
          <path d="M1 4L3.5 6.5L9 1" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
        </svg>
      </button>
      <button
        class="cancel-btn"
        onclick={handleCancel}
        onmousedown={stopMouseDown}
        type="button"
        aria-label="Cancel recording"
      >
        ×
      </button>
    {/if}
  </div>
  {/if}
</div>

<style>
  :global(html),
  :global(body) {
    background: transparent !important;
    margin: 0;
    padding: 0;
    overflow: hidden;
  }

  .pill-container {
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
    align-items: center;
    width: 100%;
    height: 100%;
    gap: 8px;
    padding-bottom: 4px;
    box-sizing: border-box;
    opacity: 0;
    transform: translateY(8px) scale(0.95);
    transition: opacity 0.2s ease-out, transform 0.2s ease-out;
  }

  .pill-container.visible {
    opacity: 1;
    transform: translateY(0) scale(1);
  }

  .error-card { width:300px; box-sizing:border-box; padding:9px 12px; border:1px solid #f8717180; border-radius:14px; background:rgba(28,24,24,.98); color:#f0f0f0; font-family:var(--sans); }
  .error-card strong { font-size:12px; font-weight:600; }
  .error-card p { margin:3px 0 7px; font-size:11px; line-height:1.35; color:#ddd; overflow-wrap:anywhere; display:-webkit-box; line-clamp:2; -webkit-line-clamp:2; -webkit-box-orient:vertical; overflow:hidden; }
  .error-actions { display:flex; align-items:center; gap:8px; }
  .error-actions button { flex:none; font:inherit; font-size:11px; border:1px solid #ffffff30; border-radius:5px; background:#ffffff12; color:white; padding:3px 7px; cursor:pointer; }
  .error-actions button:disabled { opacity:.5; cursor:default; }
  .error-actions span { font-size:10px; color:#c8c8c8; }
  .error-actions .dismiss-error { margin-left:auto; }

  .pill {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 300px;
    height: 44px;
    padding: 0 14px;
    border-radius: 22px;
    background: rgba(20, 20, 22, 0.95);
    border: 1px solid rgba(255, 255, 255, 0.1);
    box-sizing: border-box;
    user-select: none;
    /* NOTE: `-webkit-app-region: drag` does NOT work here because the
       panel is a non-activating NSPanel (can_become_key_window: false).
       Drag is handled by the `overlay_start_drag` Tauri command invoked
       from onmousedown on this element. */
    cursor: grab;
  }

  .pill:active {
    cursor: grabbing;
  }

  .pill.transcribing {
    background: rgba(20, 20, 22, 0.85);
  }

  .pill.warning {
    border-color: rgba(239, 68, 68, 0.5);
  }

  .warning-banner {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 7px 16px;
    border-radius: 14px;
    background: rgba(220, 38, 38, 0.95);
    border: 1px solid rgba(255, 120, 120, 0.3);
    animation: warningPulse 2s ease-in-out infinite;
  }

  .warning-text {
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', system-ui, sans-serif;
    font-size: 13px;
    font-weight: 600;
    color: white;
    letter-spacing: 0.2px;
    white-space: nowrap;
  }

  @keyframes warningPulse {
    0%, 100% {
      opacity: 1;
    }
    50% {
      opacity: 0.85;
    }
  }

  .indicator {
    width: 10px;
    height: 10px;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .recording-dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: #ef4444;
    animation: pulse 1.5s ease-in-out infinite;
  }

  @keyframes pulse {
    0%, 100% {
      opacity: 1;
      box-shadow: 0 0 0 0 rgba(239, 68, 68, 0.5);
    }
    50% {
      opacity: 0.7;
      box-shadow: 0 0 0 4px rgba(239, 68, 68, 0);
    }
  }

  .spinner {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    border: 2px solid rgba(255, 255, 255, 0.15);
    border-top-color: rgba(255, 255, 255, 0.7);
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to { transform: rotate(360deg); }
  }

  .status-label {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .status-text {
    font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text', system-ui, sans-serif;
    font-size: 12px;
    font-weight: 500;
    color: rgba(255, 255, 255, 0.7);
    letter-spacing: 0.2px;
    text-align: center;
    line-height: 1.3;
  }

  .badge-text {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-weight: 600;
    letter-spacing: 0.3px;
  }

  .badge-icon {
    flex-shrink: 0;
  }

  .badge-success {
    color: rgb(74, 222, 128);
  }

  .badge-warn {
    color: rgb(251, 191, 36);
  }

  .waveform-area {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    overflow: hidden;
  }

  .stop-btn,
  .cancel-btn {
    /* `-webkit-app-region: no-drag` is unnecessary here — drag is
       triggered by an explicit onmousedown on .pill, and these buttons
       call e.stopPropagation() in their own onmousedown handlers. */
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    border: none;
    border-radius: 50%;
    background: rgba(255, 255, 255, 0.1);
    color: rgba(255, 255, 255, 0.6);
    font-size: 14px;
    line-height: 1;
    cursor: pointer;
    padding: 0;
    flex-shrink: 0;
    transition: background 0.15s ease, color 0.15s ease;
  }

  .stop-btn {
    color: rgba(34, 197, 94, 0.8);
  }

  .stop-btn:hover {
    background: rgba(34, 197, 94, 0.25);
    color: #22c55e;
  }

  .cancel-btn:hover {
    background: rgba(239, 68, 68, 0.3);
    color: white;
  }
</style>
