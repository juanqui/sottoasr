<script lang="ts">
  import { listen } from '@tauri-apps/api/event';
  import { onMount } from 'svelte';
  import Waveform from './waveform.svelte';
  import RecordingTimer from './recording-timer.svelte';

  // The overlay window is ONLY created/shown during recording.
  // Initialize as recording=true immediately.
  let isRecording = $state(true);
  let isTranscribing = $state(false);
  let startTime = $state<number>(Date.now());

  // Audio levels — append-only, the Waveform component uses a ring buffer internally
  let audioLevels = $state<number[]>([]);

  let isActive = $derived(isRecording || isTranscribing);

  onMount(() => {
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
    }).then((u) => unlisteners.push(u));

    listen('recording-cancelled', () => {
      isRecording = false;
      isTranscribing = false;
    }).then((u) => unlisteners.push(u));

    listen('state-changed', (event) => {
      const state = event.payload as string;
      if (state === 'Idle') {
        isRecording = false;
        isTranscribing = false;
      } else if (state === 'Recording') {
        isRecording = true;
        isTranscribing = false;
      } else if (state === 'Transcribing') {
        isRecording = false;
        isTranscribing = true;
      }
    }).then((u) => unlisteners.push(u));

    return () => {
      unlisteners.forEach((fn) => fn());
    };
  });
</script>

<div class="pill-container" class:visible={isActive}>
  <div class="pill" class:transcribing={isTranscribing}>
    <!-- Recording indicator dot -->
    <div class="indicator">
      {#if isRecording}
        <div class="dot recording-dot"></div>
      {:else if isTranscribing}
        <div class="spinner"></div>
      {/if}
    </div>

    <!-- Waveform bars -->
    <div class="waveform-area">
      <Waveform levels={audioLevels} />
    </div>

    <!-- Timer -->
    <RecordingTimer {startTime} running={isRecording} />
  </div>
</div>

<style>
  :global(body) {
    background: transparent !important;
    margin: 0;
    padding: 0;
    overflow: hidden;
  }

  .pill-container {
    display: flex;
    justify-content: center;
    align-items: center;
    width: 100%;
    height: 100%;
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
    background: rgba(20, 20, 22, 0.92);
    backdrop-filter: blur(20px);
    -webkit-backdrop-filter: blur(20px);
    border: 1px solid rgba(255, 255, 255, 0.1);
    box-shadow:
      0 8px 32px rgba(0, 0, 0, 0.5),
      0 2px 8px rgba(0, 0, 0, 0.3);
    box-sizing: border-box;
    user-select: none;
  }

  .pill.transcribing {
    background: rgba(20, 20, 22, 0.8);
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

  .waveform-area {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    overflow: hidden;
  }
</style>
