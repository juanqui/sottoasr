import type { AppStateEnum } from '../utils/tauri';

const BAR_COUNT = 14;

class RecordingStore {
  /** Current application state */
  appState: AppStateEnum = $state('Idle');

  /** Whether recording is currently active */
  isRecording: boolean = $state(false);

  /** Timestamp (ms) when recording started, or null if not recording */
  startTime: number | null = $state(null);

  /** Current audio level (0..1) from the microphone */
  audioLevel: number = $state(0);

  /** Per-bar audio levels for the waveform visualisation */
  barLevels: number[] = $state(new Array(BAR_COUNT).fill(0));

  /** Whether the app is currently transcribing audio */
  get isTranscribing(): boolean {
    return this.appState === 'Transcribing';
  }

  /** Whether the app is currently pasting text */
  get isPasting(): boolean {
    return this.appState === 'Pasting';
  }

  /** Whether the app is in an active workflow (not idle) */
  get isActive(): boolean {
    return this.appState !== 'Idle';
  }

  /** Start a new recording session */
  start() {
    this.appState = 'Recording';
    this.isRecording = true;
    this.startTime = Date.now();
    this.audioLevel = 0;
    this.barLevels = new Array(BAR_COUNT).fill(0);
  }

  /** Stop the recording (transitions to transcribing) */
  stop() {
    this.isRecording = false;
    this.appState = 'Transcribing';
    this.audioLevel = 0;
    this.barLevels = new Array(BAR_COUNT).fill(0);
  }

  /** Cancel the recording and reset to idle */
  cancel() {
    this.isRecording = false;
    this.appState = 'Idle';
    this.startTime = null;
    this.audioLevel = 0;
    this.barLevels = new Array(BAR_COUNT).fill(0);
  }

  /** Reset to idle state */
  reset() {
    this.appState = 'Idle';
    this.isRecording = false;
    this.startTime = null;
    this.audioLevel = 0;
    this.barLevels = new Array(BAR_COUNT).fill(0);
  }

  /** Update audio level and shift bar levels for the waveform */
  setAudioLevel(level: number) {
    this.audioLevel = Math.min(1, Math.max(0, level));
    // Shift bars to the right and insert new level at the left
    const next = new Array(BAR_COUNT).fill(0);
    next[0] = this.audioLevel;
    for (let i = 1; i < BAR_COUNT; i++) {
      next[i] = this.barLevels[i - 1];
    }
    this.barLevels = next;
  }

  /** Update the app state from a Tauri event */
  setState(state: AppStateEnum) {
    this.appState = state;
    if (state === 'Recording') {
      this.isRecording = true;
    } else {
      this.isRecording = false;
    }
    if (state === 'Idle') {
      this.startTime = null;
    }
  }
}

export const recordingStore = new RecordingStore();
