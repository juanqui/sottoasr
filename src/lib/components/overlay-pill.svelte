<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { onMount } from 'svelte';
  import { fade } from 'svelte/transition';
  import Waveform from './waveform.svelte';
  import RecordingTimer from './recording-timer.svelte';
  import type { LlmCleanupStatus } from '../utils/tauri';

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
  let showSlowMessage = $state(false);
  let cleanupTimer: ReturnType<typeof setTimeout> | null = null;
  let startTime = $state<number>(0);
  // Cleanup outcome — set when the llm-cleanup-status event arrives, then
  // displayed as a badge for the brief window before the overlay hides.
  // Reset on the next state-changed:Recording so we don't carry stale state.
  let cleanupStatus = $state<LlmCleanupStatus | null>(null);

  // Audio levels — append-only, the Waveform component uses a ring buffer internally
  let audioLevels = $state<number[]>([]);

  // Duration cap and warning. Mirrors MAX_RECORDING_SECS in Rust
  // (src-tauri/src/hotkeys/manager.rs and src-tauri/src/pipeline.rs).
  const MAX_DURATION_MS = 20 * 60 * 1000;
  let showWarning = $state(false);
  let remainingSeconds = $state(60);
  let countdownInterval: ReturnType<typeof setInterval> | null = null;

  let isActive = $derived(isRecording || isTranscribing || isCleaningUp);
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

  let waveformRef: Waveform;

  // Called by Rust via eval after the overlay is hidden.
  // Clears visual state (waveform, audio levels) while invisible.
  // Does NOT set isRecording — that must happen via state-changed event
  // so Svelte detects an actual false→true transition and re-fires effects.
  function resetOverlay() {
    audioLevels = [];
    isRecording = false;
    isTranscribing = false;
    isCleaningUp = false;
    cleanupStatus = null;
    startTime = 0;
    waveformRef?.reset();
  }

  /// Compute label and visual variant for the current cleanup status badge.
  /// Returns null when no badge should be shown (Disabled, SkippedTooShort, Idle).
  function badgeFor(status: LlmCleanupStatus | null): { label: string; variant: 'success' | 'warn' } | null {
    if (!status) return null;
    switch (status.kind) {
      case 'applied':
        return { label: 'Cleaned', variant: 'success' };
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
    // Expose reset function so Rust can call it via eval after hiding the window
    (window as any).__resetOverlay = resetOverlay;

    const unlisteners: Array<() => void> = [];

    // Audio level events from Rust: { level: f32 } emitted ~30 times/sec
    listen<{ level: number }>('audio-level', (event) => {
      const level = event.payload.level;
      // Append — the waveform component reads the latest values via ring buffer
      audioLevels = [...audioLevels, Math.max(0, level)];
    }).then((u) => unlisteners.push(u));

    listen('recording-stopped', () => {
      isRecording = false;
      isTranscribing = true;
      clearWarning();
    }).then((u) => unlisteners.push(u));

    listen('recording-cancelled', () => {
      isRecording = false;
      isTranscribing = false;
      clearWarning();
    }).then((u) => unlisteners.push(u));

    listen('recording-time-warning', () => {
      if (isRecording) startCountdown();
    }).then((u) => unlisteners.push(u));

    listen('state-changed', (event) => {
      const state = event.payload as string;
      if (state === 'Idle') {
        isRecording = false;
        isTranscribing = false;
        isCleaningUp = false;
        showSlowMessage = false;
        if (cleanupTimer) { clearTimeout(cleanupTimer); cleanupTimer = null; }
        clearWarning();
      } else if (state === 'Recording') {
        isRecording = true;
        isTranscribing = false;
        isCleaningUp = false;
        showSlowMessage = false;
        cleanupStatus = null;
        if (cleanupTimer) { clearTimeout(cleanupTimer); cleanupTimer = null; }
        startTime = Date.now();
        audioLevels = [];
        clearWarning();
      } else if (state === 'Transcribing') {
        isRecording = false;
        isTranscribing = true;
        isCleaningUp = false;
        clearWarning();
      } else if (state === 'CleaningUp') {
        isRecording = false;
        isTranscribing = false;
        isCleaningUp = true;
        showSlowMessage = false;
        cleanupStatus = null;
        // Show slow message after 5 seconds
        cleanupTimer = setTimeout(() => {
          showSlowMessage = true;
        }, 5000);
      }
    }).then((u) => unlisteners.push(u));

    // Cleanup outcome arrives from Rust right after run_cleanup() finishes,
    // BEFORE the overlay hides. We replace the "Cleaning up..." spinner with
    // a brief badge. Rust holds the overlay open for a short window
    // (badge_dwell_ms) before it tells us to hide.
    listen<LlmCleanupStatus>('llm-cleanup-status', (event) => {
      cleanupStatus = event.payload;
      // Once a status arrives, the slow-message timer is no longer relevant.
      if (cleanupTimer) { clearTimeout(cleanupTimer); cleanupTimer = null; }
      showSlowMessage = false;
    }).then((u) => unlisteners.push(u));

    return () => {
      unlisteners.forEach((fn) => fn());
      clearWarning();
      delete (window as any).__resetOverlay;
    };
  });
</script>

<div class="pill-container" class:visible={isActive}>
  {#if showWarning}
    <div class="warning-banner" in:fade={{ duration: 200 }}>
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
          <span class="status-text">Cleaning up...</span>
        {/if}
      </div>
    {:else}
      <!-- Waveform bars -->
      <div class="waveform-area">
        <Waveform bind:this={waveformRef} levels={audioLevels} />
      </div>

      <!-- Timer -->
      <RecordingTimer running={isRecording} />
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
