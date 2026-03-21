<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { onMount } from 'svelte';
  import { fade } from 'svelte/transition';
  import Waveform from './waveform.svelte';
  import RecordingTimer from './recording-timer.svelte';

  async function handleCancel() {
    try {
      await invoke('cancel_recording');
    } catch (e) {
      console.error('Cancel failed:', e);
    }
  }

  // The overlay window is ONLY created/shown during recording.
  // Initialize as recording=true immediately.
  let isRecording = $state(true);
  let isTranscribing = $state(false);
  let isCleaningUp = $state(false);
  let showSlowMessage = $state(false);
  let cleanupTimer: ReturnType<typeof setTimeout> | null = null;
  let startTime = $state<number>(Date.now());

  // Audio levels — append-only, the Waveform component uses a ring buffer internally
  let audioLevels = $state<number[]>([]);

  // Duration cap and warning
  const MAX_DURATION_MS = 12 * 60 * 1000;
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
    waveformRef?.reset();
  }

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
        // Show slow message after 5 seconds
        cleanupTimer = setTimeout(() => {
          showSlowMessage = true;
        }, 5000);
      }
    }).then((u) => unlisteners.push(u));

    return () => {
      unlisteners.forEach((fn) => fn());
      clearWarning();
    };
  });
</script>

<div class="pill-container" class:visible={isActive}>
  {#if showWarning}
    <div class="warning-banner" in:fade={{ duration: 200 }}>
      <span class="warning-text">Recording stops in {countdownDisplay}</span>
    </div>
  {/if}
  <div class="pill" class:transcribing={isTranscribing || isCleaningUp} class:warning={showWarning}>
    <!-- Recording indicator dot -->
    <div class="indicator">
      {#if isRecording}
        <div class="dot recording-dot"></div>
      {:else if isTranscribing || isCleaningUp}
        <div class="spinner"></div>
      {/if}
    </div>

    {#if isCleaningUp}
      <!-- Cleaning up label -->
      <div class="status-label">
        {#if showSlowMessage}
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
      <RecordingTimer {startTime} running={isRecording} />
    {/if}

    <!-- Cancel button -->
    {#if isRecording}
      <button class="cancel-btn" onclick={handleCancel} type="button" aria-label="Cancel recording">
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
    gap: 10px;
    width: 280px;
    height: 44px;
    padding: 0 14px;
    border-radius: 22px;
    background: rgba(20, 20, 22, 0.95);
    border: 1px solid rgba(255, 255, 255, 0.1);
    box-shadow:
      0 8px 32px rgba(0, 0, 0, 0.5),
      0 2px 8px rgba(0, 0, 0, 0.3);
    box-sizing: border-box;
    user-select: none;
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
    box-shadow: 0 4px 16px rgba(220, 38, 38, 0.4);
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
      box-shadow: 0 4px 16px rgba(220, 38, 38, 0.4);
    }
    50% {
      opacity: 0.85;
      box-shadow: 0 4px 24px rgba(220, 38, 38, 0.6);
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

  .waveform-area {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    overflow: hidden;
  }

  .cancel-btn {
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
    margin-left: 2px;
  }

  .cancel-btn:hover {
    background: rgba(239, 68, 68, 0.3);
    color: white;
  }
</style>
