import { describe, it, expect, beforeEach } from 'vitest';

import { recordingStore } from './recording.svelte';

import type { AppStateEnum } from '../utils/tauri';

const BAR_COUNT = 14;

beforeEach(() => {
  // Reset the singleton to a clean state before each test
  recordingStore.reset();
});

// ---------------------------------------------------------------------------
// Initial / reset state
// ---------------------------------------------------------------------------
describe('initial state', () => {
  it('has Idle appState', () => {
    expect(recordingStore.appState).toBe('Idle');
  });

  it('is not recording', () => {
    expect(recordingStore.isRecording).toBe(false);
  });

  it('has null startTime', () => {
    expect(recordingStore.startTime).toBeNull();
  });

  it('has audioLevel at 0', () => {
    expect(recordingStore.audioLevel).toBe(0);
  });

  it('has barLevels as an array of zeros', () => {
    expect(recordingStore.barLevels).toHaveLength(BAR_COUNT);
    expect(recordingStore.barLevels.every((v: number) => v === 0)).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// start()
// ---------------------------------------------------------------------------
describe('start()', () => {
  it('sets appState to Recording', () => {
    recordingStore.start();
    expect(recordingStore.appState).toBe('Recording');
  });

  it('sets isRecording to true', () => {
    recordingStore.start();
    expect(recordingStore.isRecording).toBe(true);
  });

  it('sets startTime to a timestamp', () => {
    recordingStore.start();
    expect(recordingStore.startTime).toBeTypeOf('number');
    expect(recordingStore.startTime).toBeGreaterThan(0);
  });

  it('resets audioLevel and barLevels', () => {
    recordingStore.setAudioLevel(0.5);
    recordingStore.start();
    expect(recordingStore.audioLevel).toBe(0);
    expect(recordingStore.barLevels.every((v: number) => v === 0)).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// stop()
// ---------------------------------------------------------------------------
describe('stop()', () => {
  it('sets isRecording to false and appState to Transcribing', () => {
    recordingStore.start();
    recordingStore.stop();
    expect(recordingStore.isRecording).toBe(false);
    expect(recordingStore.appState).toBe('Transcribing');
  });

  it('zeroes audioLevel and barLevels', () => {
    recordingStore.start();
    recordingStore.setAudioLevel(0.8);
    recordingStore.stop();
    expect(recordingStore.audioLevel).toBe(0);
    expect(recordingStore.barLevels.every((v: number) => v === 0)).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// cancel()
// ---------------------------------------------------------------------------
describe('cancel()', () => {
  it('resets to idle state', () => {
    recordingStore.start();
    recordingStore.cancel();
    expect(recordingStore.appState).toBe('Idle');
    expect(recordingStore.isRecording).toBe(false);
    expect(recordingStore.startTime).toBeNull();
    expect(recordingStore.audioLevel).toBe(0);
  });
});

// ---------------------------------------------------------------------------
// reset()
// ---------------------------------------------------------------------------
describe('reset()', () => {
  it('returns all fields to their initial values', () => {
    recordingStore.start();
    recordingStore.setAudioLevel(0.9);
    recordingStore.reset();
    expect(recordingStore.appState).toBe('Idle');
    expect(recordingStore.isRecording).toBe(false);
    expect(recordingStore.startTime).toBeNull();
    expect(recordingStore.audioLevel).toBe(0);
    expect(recordingStore.barLevels).toHaveLength(BAR_COUNT);
    expect(recordingStore.barLevels.every((v: number) => v === 0)).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// setState()
// ---------------------------------------------------------------------------
describe('setState()', () => {
  const states: AppStateEnum[] = ['Idle', 'Recording', 'Transcribing', 'CleaningUp', 'Pasting'];

  it('sets appState to the given value', () => {
    for (const state of states) {
      recordingStore.setState(state);
      expect(recordingStore.appState).toBe(state);
    }
  });

  it('sets isRecording to true only for Recording', () => {
    recordingStore.setState('Recording');
    expect(recordingStore.isRecording).toBe(true);

    recordingStore.setState('Transcribing');
    expect(recordingStore.isRecording).toBe(false);
  });

  it('clears startTime when set to Idle', () => {
    recordingStore.start(); // sets startTime
    recordingStore.setState('Idle');
    expect(recordingStore.startTime).toBeNull();
  });

  it('preserves startTime when set to non-Idle states', () => {
    recordingStore.start();
    const ts = recordingStore.startTime;
    recordingStore.setState('Transcribing');
    expect(recordingStore.startTime).toBe(ts);
  });
});

// ---------------------------------------------------------------------------
// setAudioLevel()
// ---------------------------------------------------------------------------
describe('setAudioLevel()', () => {
  it('clamps values above 1 to 1', () => {
    recordingStore.setAudioLevel(5.0);
    expect(recordingStore.audioLevel).toBe(1);
  });

  it('clamps values below 0 to 0', () => {
    recordingStore.setAudioLevel(-0.5);
    expect(recordingStore.audioLevel).toBe(0);
  });

  it('accepts values in [0, 1]', () => {
    recordingStore.setAudioLevel(0.42);
    expect(recordingStore.audioLevel).toBeCloseTo(0.42);
  });

  it('shifts bar levels to the right and inserts new level at index 0', () => {
    recordingStore.setAudioLevel(0.5);
    expect(recordingStore.barLevels[0]).toBe(0.5);
    expect(recordingStore.barLevels[1]).toBe(0); // previous was 0

    recordingStore.setAudioLevel(0.8);
    expect(recordingStore.barLevels[0]).toBe(0.8);
    expect(recordingStore.barLevels[1]).toBe(0.5);
    expect(recordingStore.barLevels[2]).toBe(0);
  });

  it('barLevels length stays at BAR_COUNT', () => {
    for (let i = 0; i < BAR_COUNT + 5; i++) {
      recordingStore.setAudioLevel(i / (BAR_COUNT + 5));
    }
    expect(recordingStore.barLevels).toHaveLength(BAR_COUNT);
  });
});

// ---------------------------------------------------------------------------
// Computed getters
// ---------------------------------------------------------------------------
describe('computed getters', () => {
  it('isTranscribing is true when appState is Transcribing', () => {
    recordingStore.setState('Transcribing');
    expect(recordingStore.isTranscribing).toBe(true);
  });

  it('isTranscribing is false for other states', () => {
    recordingStore.setState('Idle');
    expect(recordingStore.isTranscribing).toBe(false);
    recordingStore.setState('Recording');
    expect(recordingStore.isTranscribing).toBe(false);
  });

  it('isPasting is true when appState is Pasting', () => {
    recordingStore.setState('Pasting');
    expect(recordingStore.isPasting).toBe(true);
  });

  it('isPasting is false for other states', () => {
    recordingStore.setState('Idle');
    expect(recordingStore.isPasting).toBe(false);
  });

  it('isActive is true for all non-Idle states', () => {
    const activeStates: AppStateEnum[] = ['Recording', 'Transcribing', 'CleaningUp', 'Pasting'];
    for (const state of activeStates) {
      recordingStore.setState(state);
      expect(recordingStore.isActive).toBe(true);
    }
  });

  it('isActive is false when Idle', () => {
    recordingStore.setState('Idle');
    expect(recordingStore.isActive).toBe(false);
  });
});
