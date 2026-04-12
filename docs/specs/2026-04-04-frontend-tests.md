# Frontend Tests with Vitest

- **Version:** 1.0
- **Date:** 2026-04-04
- **Status:** Approved

## Table of Contents

1. [Summary](#1-summary)
2. [Problem Statement](#2-problem-statement)
3. [Design Overview](#3-design-overview)
4. [Detailed Design](#4-detailed-design)
5. [Edge Cases](#5-edge-cases)
6. [File Changes](#6-file-changes)
7. [Testing Strategy](#7-testing-strategy)
8. [Security Considerations](#8-security-considerations)
9. [Cost Analysis](#9-cost-analysis)
10. [Implementation Tasks](#10-implementation-tasks)

---

## 1. Summary

Add a frontend test suite using Vitest to the SottoASR application. This is Phase 4 of a 5-phase testing initiative. The frontend currently has zero tests. This spec covers testing pure utility functions, Svelte 5 rune-based stores, and the Tauri API wrapper layer. The scope is deliberately limited to unit tests for logic modules -- component rendering tests are deferred to a future phase.

The test suite targets three categories of code:

1. **Pure utility functions** (`src/lib/utils/format.ts`) -- no dependencies, no mocking required.
2. **Svelte 5 rune stores** (`src/lib/stores/recording.svelte.ts`, `src/lib/stores/settings.svelte.ts`, `src/lib/stores/transcriptions.svelte.ts`) -- require Svelte compiler preprocessing for `$state()` runes and mocking of `@tauri-apps/api/core`.
3. **Tauri API wrappers** (`src/lib/utils/tauri.ts`) -- thin `invoke()` wrappers that validate command names and argument shapes.

## 2. Problem Statement

SottoASR's frontend has no automated tests. Every behavioral change -- hotkey display formatting, relative time labels, store state transitions, settings persistence -- is verified exclusively through manual testing. This creates three problems:

1. **Regressions go undetected.** Changes to utility functions or store logic can silently break UI behavior. For example, modifying `formatDuration` could produce incorrect timer displays in the recording overlay, but nothing catches this before a user reports it.

2. **Refactoring is risky.** The recording store manages a state machine (`Idle` -> `Recording` -> `Transcribing` -> `Pasting` -> `Idle`) with multiple side effects per transition (resetting audio levels, clearing bar levels, setting timestamps). Without tests documenting the expected state after each transition, refactoring this code requires painstaking manual verification of every path.

3. **Confidence gap in the release process.** The release checklist (`.claude/rules/release.md`) includes building and linting but has no frontend test gate. A release can ship with broken formatting logic or incorrect store behavior.

Adding unit tests for the three categories above covers the most critical frontend logic with minimal infrastructure overhead. Pure functions and store classes are testable without a DOM or component rendering, keeping test execution fast and the dependency footprint small.

## 3. Design Overview

```
vitest.config.ts          <-- Vitest config with Svelte plugin
src/
  lib/
    utils/
      format.ts           <-- Pure functions (no mocking)
      format.test.ts       <-- Tests: formatDuration, formatRelativeTime, truncateText
      tauri.ts             <-- invoke() wrappers
      tauri.test.ts        <-- Tests: mock invoke, verify command names + args
    stores/
      recording.svelte.ts            <-- Rune store ($state fields)
      recording.svelte.test.ts       <-- Tests: state transitions, computed getters
      settings.svelte.ts             <-- Rune store (async, calls Tauri)
      settings.svelte.test.ts        <-- Tests: load/save with mocked backend
      transcriptions.svelte.ts       <-- Rune store (async, calls Tauri)
      transcriptions.svelte.test.ts  <-- Tests: load/add/delete/clear
```

### Architectural decisions

1. **Vitest over Jest.** The project already uses Vite 8; Vitest shares the same config pipeline and transform chain, including the `@sveltejs/vite-plugin-svelte` plugin. Jest would require a separate Svelte/TypeScript transform setup.

2. **No component tests in this phase.** Component tests require `@testing-library/svelte`, a DOM environment, and dealing with Svelte 5's mounting lifecycle. The return on investment is lower than testing the logic layer first. Component tests are deferred to a future phase.

3. **jsdom environment.** The Svelte compiler's browser output references `document` and other DOM globals during store initialization in `.svelte.ts` files. Using `jsdom` ensures these globals exist. The `resolve.conditions: ['browser']` setting ensures Svelte's browser-side exports are used (the server-side exports have different reactivity semantics).

4. **Test files co-located with source.** Test files live next to the files they test (`format.test.ts` next to `format.ts`). This is the Vitest convention, keeps imports simple, and makes it obvious which files have tests.

5. **Store tests use `.svelte.test.ts` extension (required for signal compilation).** The `.svelte.test.ts` extension is mandatory, not optional. The Svelte compiler only processes files matching `.svelte.ts` (or `.svelte.js`). When a test file assigns to a `$state()` field on a store instance (e.g., `settingsStore.current = { ... }` in `beforeEach`), the Svelte compiler must transform that assignment into the underlying signal setter. In a plain `.test.ts` file, the assignment would target the raw property instead of the compiled setter, silently breaking reactivity and producing incorrect test results. The extension ensures the compiler processes both the store module and any test code that interacts with its reactive fields.

6. **Mock at the `invoke` level for wrapper tests; mock wrappers for store tests.** Testing `tauri.ts` means mocking `@tauri-apps/api/core` and verifying `invoke` is called with the right command string and arguments. Testing stores means mocking the wrapper functions in `tauri.ts` so store tests are isolated from the invoke layer.

## 4. Detailed Design

### 4.1 Vitest Configuration

Create `vitest.config.ts` at the project root (separate from `vite.config.ts` to avoid polluting the application build config):

```typescript
import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig({
  plugins: [svelte({ hot: false })],
  resolve: {
    conditions: ['browser'],
  },
  test: {
    include: ['src/**/*.test.ts'],
    environment: 'jsdom',
    globals: false,
    restoreMocks: true,
  },
});
```

Key settings:

| Setting | Purpose |
|---------|---------|
| `svelte({ hot: false })` | Enables Svelte compiler for `.svelte.ts` files; disables HMR (not applicable in tests) |
| `resolve.conditions: ['browser']` | Ensures Svelte 5 browser runtime is imported (not SSR variant) |
| `test.include` | Matches all `.test.ts` files (the `*.test.ts` glob already covers `*.svelte.test.ts`) |
| `test.environment: 'jsdom'` | Provides DOM globals (`document`, `window`) needed by compiled Svelte stores |
| `test.restoreMocks: true` | Auto-restores all mocks after each test to prevent cross-test leakage |

### 4.2 Utility Function Tests: `src/lib/utils/format.test.ts`

These are pure function tests with no dependencies. They form the foundation of the test suite.

#### `formatDuration(ms: number): string`

The function converts milliseconds to `M:SS` format using `Math.floor`.

All three `describe` blocks below belong to a single file (`src/lib/utils/format.test.ts`) with these shared imports at the top:

```typescript
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { formatDuration, formatRelativeTime, truncateText } from './format';
```

#### formatDuration tests

```typescript
describe('formatDuration', () => {
  it('formats zero milliseconds as 0:00', () => {
    expect(formatDuration(0)).toBe('0:00');
  });

  it('formats exact seconds', () => {
    expect(formatDuration(1000)).toBe('0:01');
    expect(formatDuration(10000)).toBe('0:10');
    expect(formatDuration(59000)).toBe('0:59');
  });

  it('formats minutes and seconds', () => {
    expect(formatDuration(60000)).toBe('1:00');
    expect(formatDuration(61000)).toBe('1:01');
    expect(formatDuration(90000)).toBe('1:30');
  });

  it('formats large durations (no hour boundary)', () => {
    expect(formatDuration(3600000)).toBe('60:00');
    expect(formatDuration(5400000)).toBe('90:00');
  });

  it('rounds sub-second values down (floor behavior)', () => {
    expect(formatDuration(500)).toBe('0:00');
    expect(formatDuration(999)).toBe('0:00');
    expect(formatDuration(1500)).toBe('0:01');
    expect(formatDuration(1999)).toBe('0:01');
  });

  it('handles negative values gracefully', () => {
    // Math.floor(-500/1000) = -1, which makes totalSeconds -1.
    // -1 % 60 = -1, Math.floor(-1/60) = -1. This is technically
    // undefined behavior for the function but should not throw.
    expect(() => formatDuration(-1000)).not.toThrow();
  });

  it('handles NaN input without throwing', () => {
    // Math.floor(NaN / 1000) = NaN, NaN % 60 = NaN, padStart still works.
    // Documents the current behavior: produces "NaN:NaN" which is ugly but
    // not a crash. Callers are responsible for passing valid numbers.
    expect(() => formatDuration(NaN)).not.toThrow();
    expect(formatDuration(NaN)).toBe('NaN:NaN');
  });
});
```

#### `formatRelativeTime(date: string): string`

This function is time-dependent. Tests use `vi.useFakeTimers()` to freeze `Date.now()`.

```typescript
describe('formatRelativeTime', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('returns "just now" for dates less than 10 seconds ago', () => {
    const now = new Date('2026-04-04T12:00:00Z');
    vi.setSystemTime(now);
    const fiveSecondsAgo = new Date('2026-04-04T11:59:55Z').toISOString();
    expect(formatRelativeTime(fiveSecondsAgo)).toBe('just now');
  });

  it('returns seconds ago for 10-59 seconds', () => {
    const now = new Date('2026-04-04T12:00:00Z');
    vi.setSystemTime(now);
    const thirtySecondsAgo = new Date('2026-04-04T11:59:30Z').toISOString();
    expect(formatRelativeTime(thirtySecondsAgo)).toBe('30 seconds ago');
  });

  it('returns singular "1 minute ago"', () => {
    const now = new Date('2026-04-04T12:00:00Z');
    vi.setSystemTime(now);
    const oneMinuteAgo = new Date('2026-04-04T11:59:00Z').toISOString();
    expect(formatRelativeTime(oneMinuteAgo)).toBe('1 minute ago');
  });

  it('returns plural minutes for 2-59 minutes', () => {
    const now = new Date('2026-04-04T12:00:00Z');
    vi.setSystemTime(now);
    const fiveMinutesAgo = new Date('2026-04-04T11:55:00Z').toISOString();
    expect(formatRelativeTime(fiveMinutesAgo)).toBe('5 minutes ago');
  });

  it('returns singular "1 hour ago"', () => {
    const now = new Date('2026-04-04T12:00:00Z');
    vi.setSystemTime(now);
    const oneHourAgo = new Date('2026-04-04T11:00:00Z').toISOString();
    expect(formatRelativeTime(oneHourAgo)).toBe('1 hour ago');
  });

  it('returns plural hours for 2-23 hours', () => {
    const now = new Date('2026-04-04T12:00:00Z');
    vi.setSystemTime(now);
    const threeHoursAgo = new Date('2026-04-04T09:00:00Z').toISOString();
    expect(formatRelativeTime(threeHoursAgo)).toBe('3 hours ago');
  });

  it('returns "yesterday" for 24-47 hours ago', () => {
    const now = new Date('2026-04-04T12:00:00Z');
    vi.setSystemTime(now);
    const yesterday = new Date('2026-04-03T12:00:00Z').toISOString();
    expect(formatRelativeTime(yesterday)).toBe('yesterday');
  });

  it('returns "N days ago" for 2-6 days', () => {
    const now = new Date('2026-04-04T12:00:00Z');
    vi.setSystemTime(now);
    const threeDaysAgo = new Date('2026-04-01T12:00:00Z').toISOString();
    expect(formatRelativeTime(threeDaysAgo)).toBe('3 days ago');
  });

  it('returns "Mon DD" format for 7+ days', () => {
    const now = new Date('2026-04-04T12:00:00Z');
    vi.setSystemTime(now);
    // Use mid-month noon UTC to avoid timezone-sensitive date boundary issues:
    // format.ts uses d.getDate() (local time), so in extreme timezones (UTC+13/+14)
    // a date near midnight UTC could shift to the next local day.
    const twoWeeksAgo = new Date('2025-06-15T12:00:00Z').toISOString();
    expect(formatRelativeTime(twoWeeksAgo)).toBe('Jun 15');
  });

  it('returns "Unknown" for invalid date strings', () => {
    expect(formatRelativeTime('not-a-date')).toBe('Unknown');
    expect(formatRelativeTime('')).toBe('Unknown');
  });
});
```

#### `truncateText(text: string, maxLength: number): string`

```typescript
describe('truncateText', () => {
  it('returns the original text when shorter than maxLength', () => {
    expect(truncateText('hello', 10)).toBe('hello');
  });

  it('returns the original text when exactly maxLength', () => {
    expect(truncateText('hello', 5)).toBe('hello');
  });

  it('truncates with ellipsis when text exceeds maxLength', () => {
    const result = truncateText('hello world', 5);
    expect(result).toBe('hello\u2026');
    expect(result.length).toBe(6); // 5 chars + ellipsis
  });

  it('trims trailing whitespace before adding ellipsis', () => {
    // "hello " truncated at 6 would be "hello " -> trimEnd -> "hello" + ellipsis
    expect(truncateText('hello world', 6)).toBe('hello\u2026');
  });

  it('handles maxLength of 0', () => {
    expect(truncateText('hello', 0)).toBe('\u2026');
  });

  it('handles empty string', () => {
    expect(truncateText('', 5)).toBe('');
  });
});
```

### 4.3 Tauri API Wrapper Tests: `src/lib/utils/tauri.test.ts`

These tests verify that wrapper functions call `invoke()` with the correct Tauri command name and argument shape. The `@tauri-apps/api/core` module is mocked at the module level.

```typescript
import { describe, it, expect, vi, beforeEach } from 'vitest';

// Mock the entire @tauri-apps/api/core module
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke } from '@tauri-apps/api/core';
import {
  startRecording,
  stopRecording,
  cancelRecording,
  getTranscriptions,
  getLastTranscription,
  deleteTranscription,
  clearTranscriptions,
  exportTranscriptionsCsv,
  getSettings,
  updateSettings,
  checkMicrophonePermission,
  checkAccessibilityPermission,
  requestAccessibilityPermission,
  requestMicrophonePermission,
  checkAllPermissions,
  openAccessibilitySettings,
  openMicrophoneSettings,
  getAsrBackend,
  getModelStatus,
  needsOnboarding,
  initAsr,
  downloadModel,
  completeSetup,
  getLlmStatus,
  checkLlmUpdate,
  downloadLlmModel,
  updateLlmModel,
  cancelLlmDownload,
  deleteLlmModel,
  loadLlmModel,
  unloadLlmModel,
  checkAppUpdate,
  performAppUpdate,
  getUpdateStatus,
} from './tauri';

const mockedInvoke = vi.mocked(invoke);

describe('tauri API wrappers', () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
  });

  describe('recording commands', () => {
    it('startRecording invokes "start_recording"', async () => {
      mockedInvoke.mockResolvedValueOnce(undefined);
      await startRecording();
      expect(mockedInvoke).toHaveBeenCalledWith('start_recording');
    });

    it('stopRecording invokes "stop_recording"', async () => {
      mockedInvoke.mockResolvedValueOnce(undefined);
      await stopRecording();
      expect(mockedInvoke).toHaveBeenCalledWith('stop_recording');
    });

    it('cancelRecording invokes "cancel_recording"', async () => {
      mockedInvoke.mockResolvedValueOnce(undefined);
      await cancelRecording();
      expect(mockedInvoke).toHaveBeenCalledWith('cancel_recording');
    });
  });

  describe('transcription commands', () => {
    it('getTranscriptions invokes "get_transcriptions"', async () => {
      mockedInvoke.mockResolvedValueOnce([]);
      const result = await getTranscriptions();
      expect(mockedInvoke).toHaveBeenCalledWith('get_transcriptions');
      expect(result).toEqual([]);
    });

    it('deleteTranscription passes id argument', async () => {
      mockedInvoke.mockResolvedValueOnce(undefined);
      await deleteTranscription('abc-123');
      expect(mockedInvoke).toHaveBeenCalledWith('delete_transcription', {
        id: 'abc-123',
      });
    });

    it('clearTranscriptions invokes "clear_transcriptions"', async () => {
      mockedInvoke.mockResolvedValueOnce(undefined);
      await clearTranscriptions();
      expect(mockedInvoke).toHaveBeenCalledWith('clear_transcriptions');
    });
  });

  describe('settings commands', () => {
    it('getSettings invokes "get_settings"', async () => {
      const mockSettings = { push_to_talk_shortcut: 'Ctrl+Space' };
      mockedInvoke.mockResolvedValueOnce(mockSettings);
      const result = await getSettings();
      expect(mockedInvoke).toHaveBeenCalledWith('get_settings');
      expect(result).toEqual(mockSettings);
    });

    it('updateSettings passes newSettings argument', async () => {
      const settings = { push_to_talk_shortcut: 'Ctrl+Space' };
      mockedInvoke.mockResolvedValueOnce(undefined);
      await updateSettings(settings as any);
      expect(mockedInvoke).toHaveBeenCalledWith('update_settings', {
        newSettings: settings,
      });
    });
  });

  describe('additional transcription commands', () => {
    it('getLastTranscription invokes "get_last_transcription"', async () => {
      mockedInvoke.mockResolvedValueOnce(null);
      const result = await getLastTranscription();
      expect(mockedInvoke).toHaveBeenCalledWith('get_last_transcription');
      expect(result).toBeNull();
    });

    it('exportTranscriptionsCsv invokes "export_transcriptions_csv"', async () => {
      mockedInvoke.mockResolvedValueOnce('csv-data');
      const result = await exportTranscriptionsCsv();
      expect(mockedInvoke).toHaveBeenCalledWith('export_transcriptions_csv');
      expect(result).toBe('csv-data');
    });
  });

  describe('permission commands', () => {
    it('checkMicrophonePermission invokes "check_microphone_permission"', async () => {
      mockedInvoke.mockResolvedValueOnce(true);
      const result = await checkMicrophonePermission();
      expect(mockedInvoke).toHaveBeenCalledWith('check_microphone_permission');
      expect(result).toBe(true);
    });

    it('checkAccessibilityPermission invokes "check_accessibility_permission"', async () => {
      mockedInvoke.mockResolvedValueOnce(false);
      const result = await checkAccessibilityPermission();
      expect(mockedInvoke).toHaveBeenCalledWith('check_accessibility_permission');
      expect(result).toBe(false);
    });

    it('requestAccessibilityPermission invokes "request_accessibility_permission"', async () => {
      mockedInvoke.mockResolvedValueOnce(undefined);
      await requestAccessibilityPermission();
      expect(mockedInvoke).toHaveBeenCalledWith('request_accessibility_permission');
    });

    it('requestMicrophonePermission invokes "request_microphone_permission"', async () => {
      mockedInvoke.mockResolvedValueOnce(true);
      const result = await requestMicrophonePermission();
      expect(mockedInvoke).toHaveBeenCalledWith('request_microphone_permission');
      expect(result).toBe(true);
    });

    it('checkAllPermissions invokes "check_all_permissions"', async () => {
      const status = { microphone: 'authorized', accessibility_api: true, accessibility_functional: true, needs_restart: false };
      mockedInvoke.mockResolvedValueOnce(status);
      const result = await checkAllPermissions();
      expect(mockedInvoke).toHaveBeenCalledWith('check_all_permissions');
      expect(result).toEqual(status);
    });

    it('openAccessibilitySettings invokes "open_accessibility_settings"', async () => {
      mockedInvoke.mockResolvedValueOnce(undefined);
      await openAccessibilitySettings();
      expect(mockedInvoke).toHaveBeenCalledWith('open_accessibility_settings');
    });

    it('openMicrophoneSettings invokes "open_microphone_settings"', async () => {
      mockedInvoke.mockResolvedValueOnce(undefined);
      await openMicrophoneSettings();
      expect(mockedInvoke).toHaveBeenCalledWith('open_microphone_settings');
    });
  });

  describe('setup / onboarding commands', () => {
    it('getAsrBackend invokes "get_asr_backend"', async () => {
      const info = { backend: 'fluidaudio', model_available: true };
      mockedInvoke.mockResolvedValueOnce(info);
      const result = await getAsrBackend();
      expect(mockedInvoke).toHaveBeenCalledWith('get_asr_backend');
      expect(result).toEqual(info);
    });

    it('getModelStatus invokes "get_model_status"', async () => {
      const status = { downloaded: true, loaded: true, path: '/tmp/model', name: 'test', size_bytes: 1000 };
      mockedInvoke.mockResolvedValueOnce(status);
      const result = await getModelStatus();
      expect(mockedInvoke).toHaveBeenCalledWith('get_model_status');
      expect(result).toEqual(status);
    });

    it('needsOnboarding invokes "needs_onboarding"', async () => {
      mockedInvoke.mockResolvedValueOnce(true);
      const result = await needsOnboarding();
      expect(mockedInvoke).toHaveBeenCalledWith('needs_onboarding');
      expect(result).toBe(true);
    });

    it('initAsr invokes "init_asr"', async () => {
      mockedInvoke.mockResolvedValueOnce(undefined);
      await initAsr();
      expect(mockedInvoke).toHaveBeenCalledWith('init_asr');
    });

    it('downloadModel invokes "download_model"', async () => {
      mockedInvoke.mockResolvedValueOnce(undefined);
      await downloadModel();
      expect(mockedInvoke).toHaveBeenCalledWith('download_model');
    });

    it('completeSetup invokes "complete_setup"', async () => {
      const result = { backend: 'fluidaudio', microphone_permission: true, accessibility_permission: true, asr_ready: true, model_available: true };
      mockedInvoke.mockResolvedValueOnce(result);
      const actual = await completeSetup();
      expect(mockedInvoke).toHaveBeenCalledWith('complete_setup');
      expect(actual).toEqual(result);
    });
  });

  describe('LLM commands', () => {
    it('getLlmStatus invokes "get_llm_status"', async () => {
      const status = { available: true, unavailable_reason: null, downloaded: true, downloading: false, loaded: false, model_name: 'test', model_path: '/tmp', update_available: false };
      mockedInvoke.mockResolvedValueOnce(status);
      const result = await getLlmStatus();
      expect(mockedInvoke).toHaveBeenCalledWith('get_llm_status');
      expect(result).toEqual(status);
    });

    it('checkLlmUpdate invokes "check_llm_update"', async () => {
      mockedInvoke.mockResolvedValueOnce(false);
      const result = await checkLlmUpdate();
      expect(mockedInvoke).toHaveBeenCalledWith('check_llm_update');
      expect(result).toBe(false);
    });

    it('downloadLlmModel invokes "download_llm_model"', async () => {
      mockedInvoke.mockResolvedValueOnce(undefined);
      await downloadLlmModel();
      expect(mockedInvoke).toHaveBeenCalledWith('download_llm_model');
    });

    it('updateLlmModel invokes "update_llm_model"', async () => {
      mockedInvoke.mockResolvedValueOnce(undefined);
      await updateLlmModel();
      expect(mockedInvoke).toHaveBeenCalledWith('update_llm_model');
    });

    it('cancelLlmDownload invokes "cancel_llm_download"', async () => {
      mockedInvoke.mockResolvedValueOnce(undefined);
      await cancelLlmDownload();
      expect(mockedInvoke).toHaveBeenCalledWith('cancel_llm_download');
    });

    it('deleteLlmModel invokes "delete_llm_model"', async () => {
      mockedInvoke.mockResolvedValueOnce(undefined);
      await deleteLlmModel();
      expect(mockedInvoke).toHaveBeenCalledWith('delete_llm_model');
    });

    it('loadLlmModel invokes "load_llm_model"', async () => {
      mockedInvoke.mockResolvedValueOnce(undefined);
      await loadLlmModel();
      expect(mockedInvoke).toHaveBeenCalledWith('load_llm_model');
    });

    it('unloadLlmModel invokes "unload_llm_model"', async () => {
      mockedInvoke.mockResolvedValueOnce(undefined);
      await unloadLlmModel();
      expect(mockedInvoke).toHaveBeenCalledWith('unload_llm_model');
    });
  });

  describe('app update commands', () => {
    it('checkAppUpdate invokes "check_app_update"', async () => {
      mockedInvoke.mockResolvedValueOnce('1.0.0');
      const result = await checkAppUpdate();
      expect(mockedInvoke).toHaveBeenCalledWith('check_app_update');
      expect(result).toBe('1.0.0');
    });

    it('performAppUpdate invokes "perform_app_update"', async () => {
      mockedInvoke.mockResolvedValueOnce('done');
      const result = await performAppUpdate();
      expect(mockedInvoke).toHaveBeenCalledWith('perform_app_update');
      expect(result).toBe('done');
    });

    it('getUpdateStatus invokes "get_update_status"', async () => {
      const status = { update_available: false, version: null, release_notes: null, downloading: false, restart_pending: false };
      mockedInvoke.mockResolvedValueOnce(status);
      const result = await getUpdateStatus();
      expect(mockedInvoke).toHaveBeenCalledWith('get_update_status');
      expect(result).toEqual(status);
    });
  });
});
```

### 4.4 RecordingStore Tests: `src/lib/stores/recording.svelte.test.ts`

The recording store uses `$state()` runes. When compiled by the Svelte plugin, these become reactive signals, but their properties are readable as plain values in synchronous test code. No Tauri mocking is needed -- this store has no backend calls.

```typescript
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { recordingStore } from './recording.svelte';

describe('RecordingStore', () => {
  beforeEach(() => {
    recordingStore.reset();
  });

  describe('initial state (after reset)', () => {
    it('has Idle appState', () => {
      expect(recordingStore.appState).toBe('Idle');
    });

    it('is not recording', () => {
      expect(recordingStore.isRecording).toBe(false);
    });

    it('has null startTime', () => {
      expect(recordingStore.startTime).toBeNull();
    });

    it('has zero audioLevel', () => {
      expect(recordingStore.audioLevel).toBe(0);
    });

    it('has 14 zero-filled barLevels', () => {
      expect(recordingStore.barLevels).toHaveLength(14);
      expect(recordingStore.barLevels.every((l) => l === 0)).toBe(true);
    });
  });

  describe('start()', () => {
    it('sets appState to Recording', () => {
      recordingStore.start();
      expect(recordingStore.appState).toBe('Recording');
    });

    it('sets isRecording to true', () => {
      recordingStore.start();
      expect(recordingStore.isRecording).toBe(true);
    });

    it('sets startTime to a recent timestamp', () => {
      const before = Date.now();
      recordingStore.start();
      const after = Date.now();
      expect(recordingStore.startTime).toBeGreaterThanOrEqual(before);
      expect(recordingStore.startTime).toBeLessThanOrEqual(after);
    });

    it('resets audioLevel and barLevels to zero', () => {
      recordingStore.setAudioLevel(0.8);
      recordingStore.start();
      expect(recordingStore.audioLevel).toBe(0);
      expect(recordingStore.barLevels.every((l) => l === 0)).toBe(true);
    });
  });

  describe('stop()', () => {
    it('sets appState to Transcribing', () => {
      recordingStore.start();
      recordingStore.stop();
      expect(recordingStore.appState).toBe('Transcribing');
    });

    it('sets isRecording to false', () => {
      recordingStore.start();
      recordingStore.stop();
      expect(recordingStore.isRecording).toBe(false);
    });

    it('resets audioLevel to zero', () => {
      recordingStore.start();
      recordingStore.setAudioLevel(0.7);
      recordingStore.stop();
      expect(recordingStore.audioLevel).toBe(0);
    });

    it('resets barLevels to zero', () => {
      recordingStore.start();
      recordingStore.setAudioLevel(0.7);
      recordingStore.stop();
      expect(recordingStore.barLevels.every((l) => l === 0)).toBe(true);
    });

    it('preserves startTime (does not clear it)', () => {
      recordingStore.start();
      const ts = recordingStore.startTime;
      recordingStore.stop();
      expect(recordingStore.startTime).toBe(ts);
    });
  });

  describe('cancel()', () => {
    it('sets appState to Idle', () => {
      recordingStore.start();
      recordingStore.cancel();
      expect(recordingStore.appState).toBe('Idle');
    });

    it('sets isRecording to false', () => {
      recordingStore.start();
      recordingStore.cancel();
      expect(recordingStore.isRecording).toBe(false);
    });

    it('clears startTime', () => {
      recordingStore.start();
      recordingStore.cancel();
      expect(recordingStore.startTime).toBeNull();
    });
  });

  describe('reset()', () => {
    it('returns to initial state from any state', () => {
      recordingStore.start();
      recordingStore.setAudioLevel(0.9);
      recordingStore.reset();

      expect(recordingStore.appState).toBe('Idle');
      expect(recordingStore.isRecording).toBe(false);
      expect(recordingStore.startTime).toBeNull();
      expect(recordingStore.audioLevel).toBe(0);
      expect(recordingStore.barLevels.every((l) => l === 0)).toBe(true);
    });
  });

  describe('setState()', () => {
    it('sets Recording state and isRecording=true', () => {
      recordingStore.setState('Recording');
      expect(recordingStore.appState).toBe('Recording');
      expect(recordingStore.isRecording).toBe(true);
    });

    it('sets Transcribing state and isRecording=false', () => {
      recordingStore.setState('Recording');
      recordingStore.setState('Transcribing');
      expect(recordingStore.appState).toBe('Transcribing');
      expect(recordingStore.isRecording).toBe(false);
    });

    it('sets Idle state and clears startTime', () => {
      recordingStore.start(); // sets startTime
      recordingStore.setState('Idle');
      expect(recordingStore.appState).toBe('Idle');
      expect(recordingStore.startTime).toBeNull();
    });

    it('sets Pasting state and isRecording=false', () => {
      recordingStore.setState('Pasting');
      expect(recordingStore.appState).toBe('Pasting');
      expect(recordingStore.isRecording).toBe(false);
    });

    it('sets CleaningUp state', () => {
      recordingStore.setState('CleaningUp');
      expect(recordingStore.appState).toBe('CleaningUp');
      expect(recordingStore.isRecording).toBe(false);
    });
  });

  describe('setAudioLevel()', () => {
    it('sets audioLevel to the given value', () => {
      recordingStore.setAudioLevel(0.5);
      expect(recordingStore.audioLevel).toBe(0.5);
    });

    it('clamps values above 1.0 to 1.0', () => {
      recordingStore.setAudioLevel(1.5);
      expect(recordingStore.audioLevel).toBe(1.0);
    });

    it('clamps negative values to 0.0', () => {
      recordingStore.setAudioLevel(-0.5);
      expect(recordingStore.audioLevel).toBe(0.0);
    });

    it('inserts the new level at barLevels[0]', () => {
      recordingStore.setAudioLevel(0.7);
      expect(recordingStore.barLevels[0]).toBe(0.7);
    });

    it('shifts previous bar levels to the right', () => {
      recordingStore.setAudioLevel(0.3);
      recordingStore.setAudioLevel(0.6);
      recordingStore.setAudioLevel(0.9);

      expect(recordingStore.barLevels[0]).toBe(0.9);
      expect(recordingStore.barLevels[1]).toBe(0.6);
      expect(recordingStore.barLevels[2]).toBe(0.3);
    });

    it('maintains exactly 14 bar levels', () => {
      for (let i = 0; i < 20; i++) {
        recordingStore.setAudioLevel(i * 0.05);
      }
      expect(recordingStore.barLevels).toHaveLength(14);
    });
  });

  describe('computed getters', () => {
    it('isTranscribing is true only in Transcribing state', () => {
      expect(recordingStore.isTranscribing).toBe(false);
      recordingStore.setState('Transcribing');
      expect(recordingStore.isTranscribing).toBe(true);
      recordingStore.setState('Recording');
      expect(recordingStore.isTranscribing).toBe(false);
    });

    it('isPasting is true only in Pasting state', () => {
      expect(recordingStore.isPasting).toBe(false);
      recordingStore.setState('Pasting');
      expect(recordingStore.isPasting).toBe(true);
      recordingStore.setState('Idle');
      expect(recordingStore.isPasting).toBe(false);
    });

    it('isActive is true for all states except Idle', () => {
      expect(recordingStore.isActive).toBe(false); // Idle
      recordingStore.setState('Recording');
      expect(recordingStore.isActive).toBe(true);
      recordingStore.setState('Transcribing');
      expect(recordingStore.isActive).toBe(true);
      recordingStore.setState('Pasting');
      expect(recordingStore.isActive).toBe(true);
      recordingStore.setState('CleaningUp');
      expect(recordingStore.isActive).toBe(true);
      recordingStore.setState('Idle');
      expect(recordingStore.isActive).toBe(false);
    });
  });
});
```

### 4.5 SettingsStore Tests: `src/lib/stores/settings.svelte.test.ts`

The settings store calls Tauri API wrappers. These tests mock the wrapper module (`../utils/tauri`) rather than `@tauri-apps/api/core` directly, keeping the mock boundary clean.

```typescript
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../utils/tauri', () => ({
  getSettings: vi.fn(),
  updateSettings: vi.fn(),
}));

import { settingsStore } from './settings.svelte';
import { getSettings, updateSettings } from '../utils/tauri';

const mockedGetSettings = vi.mocked(getSettings);
const mockedUpdateSettings = vi.mocked(updateSettings);

const DEFAULT_SETTINGS = {
  push_to_talk_shortcut: 'CommandOrControl+Shift+Space',
  push_to_talk_shortcut_alt: null,
  toggle_shortcut: 'CommandOrControl+Shift+D',
  toggle_shortcut_alt: null,
  cancel_shortcut: 'Escape',
  cancel_shortcut_alt: null,
  show_overlay: true,
  auto_paste: true,
  restore_clipboard: true,
  restore_focus_before_paste: true,
  model_path: '',
  language: 'auto',
  max_history: 500,
  launch_at_login: false,
  llm_cleanup_enabled: false,
  auto_check_updates: true,
};

describe('SettingsStore', () => {
  beforeEach(() => {
    // Reset store to defaults before each test.
    // The store is a singleton, so we manually reset its fields.
    settingsStore.current = { ...DEFAULT_SETTINGS };
    settingsStore.loaded = false;
    settingsStore.saving = false;
    mockedGetSettings.mockReset();
    mockedUpdateSettings.mockReset();
  });

  describe('initial state', () => {
    it('has default settings', () => {
      expect(settingsStore.current.push_to_talk_shortcut).toBe(
        'CommandOrControl+Shift+Space',
      );
      expect(settingsStore.current.show_overlay).toBe(true);
      expect(settingsStore.current.auto_paste).toBe(true);
      expect(settingsStore.current.language).toBe('auto');
    });

    it('has loaded=false', () => {
      expect(settingsStore.loaded).toBe(false);
    });

    it('has saving=false', () => {
      expect(settingsStore.saving).toBe(false);
    });
  });

  describe('load()', () => {
    it('fetches settings from backend and merges with defaults', async () => {
      const backendSettings = {
        push_to_talk_shortcut: 'Alt+Space',
        show_overlay: false,
      };
      mockedGetSettings.mockResolvedValueOnce(backendSettings as any);

      await settingsStore.load();

      expect(mockedGetSettings).toHaveBeenCalledOnce();
      expect(settingsStore.current.push_to_talk_shortcut).toBe('Alt+Space');
      expect(settingsStore.current.show_overlay).toBe(false);
      // Defaults preserved for fields not returned by backend
      expect(settingsStore.current.auto_paste).toBe(true);
      expect(settingsStore.current.language).toBe('auto');
      expect(settingsStore.loaded).toBe(true);
    });

    it('falls back to defaults on backend error and logs to console.error', async () => {
      const consoleSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
      mockedGetSettings.mockRejectedValueOnce(new Error('IPC failed'));

      await settingsStore.load();

      expect(settingsStore.current).toEqual(DEFAULT_SETTINGS);
      expect(settingsStore.loaded).toBe(true);
      expect(consoleSpy).toHaveBeenCalledWith('Failed to load settings:', expect.any(Error));
    });
  });

  describe('save()', () => {
    it('calls updateSettings with current settings', async () => {
      mockedUpdateSettings.mockResolvedValueOnce(undefined);

      await settingsStore.save();

      expect(mockedUpdateSettings).toHaveBeenCalledWith(settingsStore.current);
    });

    it('sets saving=true during the call and false after', async () => {
      let savingDuringCall = false;
      mockedUpdateSettings.mockImplementationOnce(async () => {
        savingDuringCall = settingsStore.saving;
      });

      await settingsStore.save();

      expect(savingDuringCall).toBe(true);
      expect(settingsStore.saving).toBe(false);
    });

    it('resets saving to false, logs to console.error, and re-throws on error', async () => {
      const consoleSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
      mockedUpdateSettings.mockRejectedValueOnce(new Error('Save failed'));

      await expect(settingsStore.save()).rejects.toThrow('Save failed');
      expect(settingsStore.saving).toBe(false);
      expect(consoleSpy).toHaveBeenCalledWith('Failed to save settings:', expect.any(Error));
    });
  });

  describe('update()', () => {
    it('updates a single setting field', () => {
      settingsStore.update('show_overlay', false);
      expect(settingsStore.current.show_overlay).toBe(false);
    });

    it('does not mutate the original object (creates new reference)', () => {
      const before = settingsStore.current;
      settingsStore.update('language', 'en');
      expect(settingsStore.current).not.toBe(before);
      expect(settingsStore.current.language).toBe('en');
    });

    it('preserves other fields when updating one', () => {
      settingsStore.update('max_history', 1000);
      expect(settingsStore.current.push_to_talk_shortcut).toBe(
        'CommandOrControl+Shift+Space',
      );
      expect(settingsStore.current.max_history).toBe(1000);
    });
  });
});
```

### 4.6 TranscriptionStore Tests: `src/lib/stores/transcriptions.svelte.test.ts`

The transcription store manages a list of transcriptions with CRUD operations backed by Tauri.

```typescript
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../utils/tauri', () => ({
  getTranscriptions: vi.fn(),
  deleteTranscription: vi.fn(),
  clearTranscriptions: vi.fn(),
}));

import { transcriptionStore } from './transcriptions.svelte';
import {
  getTranscriptions,
  deleteTranscription,
  clearTranscriptions,
} from '../utils/tauri';

import type { Transcription } from '../utils/tauri';

const mockedGetTranscriptions = vi.mocked(getTranscriptions);
const mockedDeleteTranscription = vi.mocked(deleteTranscription);
const mockedClearTranscriptions = vi.mocked(clearTranscriptions);

function makeTranscription(overrides: Partial<Transcription> = {}): Transcription {
  return {
    id: 'test-id-1',
    text: 'Hello world',
    duration_ms: 2000,
    created_at: '2026-04-04T12:00:00Z',
    word_count: 2,
    ...overrides,
  };
}

describe('TranscriptionStore', () => {
  beforeEach(() => {
    transcriptionStore.items = [];
    transcriptionStore.loaded = false;
    mockedGetTranscriptions.mockReset();
    mockedDeleteTranscription.mockReset();
    mockedClearTranscriptions.mockReset();
  });

  describe('initial state', () => {
    it('has empty items array', () => {
      expect(transcriptionStore.items).toEqual([]);
    });

    it('has loaded=false', () => {
      expect(transcriptionStore.loaded).toBe(false);
    });

    it('last returns null when empty', () => {
      expect(transcriptionStore.last).toBeNull();
    });
  });

  describe('load()', () => {
    it('fetches transcriptions from backend', async () => {
      const items = [makeTranscription({ id: '1' }), makeTranscription({ id: '2' })];
      mockedGetTranscriptions.mockResolvedValueOnce(items);

      await transcriptionStore.load();

      expect(mockedGetTranscriptions).toHaveBeenCalledOnce();
      expect(transcriptionStore.items).toEqual(items);
      expect(transcriptionStore.loaded).toBe(true);
    });

    it('does not set loaded on error and logs to console.error', async () => {
      const consoleSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
      mockedGetTranscriptions.mockRejectedValueOnce(new Error('fail'));

      await transcriptionStore.load();

      expect(transcriptionStore.loaded).toBe(false);
      expect(transcriptionStore.items).toEqual([]);
      expect(consoleSpy).toHaveBeenCalledWith('Failed to load transcriptions:', expect.any(Error));
    });
  });

  describe('add()', () => {
    it('prepends transcription to the list', () => {
      const existing = makeTranscription({ id: '1' });
      transcriptionStore.items = [existing];

      const newItem = makeTranscription({ id: '2', text: 'New item' });
      transcriptionStore.add(newItem);

      expect(transcriptionStore.items).toHaveLength(2);
      expect(transcriptionStore.items[0].id).toBe('2');
      expect(transcriptionStore.items[1].id).toBe('1');
    });

    it('updates the last getter', () => {
      const item = makeTranscription({ id: 'latest' });
      transcriptionStore.add(item);
      expect(transcriptionStore.last?.id).toBe('latest');
    });
  });

  describe('delete()', () => {
    it('removes the transcription from the list', async () => {
      const items = [
        makeTranscription({ id: '1' }),
        makeTranscription({ id: '2' }),
      ];
      transcriptionStore.items = items;
      mockedDeleteTranscription.mockResolvedValueOnce(undefined);

      await transcriptionStore.delete('1');

      expect(mockedDeleteTranscription).toHaveBeenCalledWith('1');
      expect(transcriptionStore.items).toHaveLength(1);
      expect(transcriptionStore.items[0].id).toBe('2');
    });

    it('does not remove on backend error and logs to console.error', async () => {
      const consoleSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
      const items = [makeTranscription({ id: '1' })];
      transcriptionStore.items = items;
      mockedDeleteTranscription.mockRejectedValueOnce(new Error('fail'));

      await transcriptionStore.delete('1');

      expect(transcriptionStore.items).toHaveLength(1);
      expect(consoleSpy).toHaveBeenCalledWith('Failed to delete transcription:', expect.any(Error));
    });
  });

  describe('clear()', () => {
    it('empties the items list', async () => {
      transcriptionStore.items = [
        makeTranscription({ id: '1' }),
        makeTranscription({ id: '2' }),
      ];
      mockedClearTranscriptions.mockResolvedValueOnce(undefined);

      await transcriptionStore.clear();

      expect(mockedClearTranscriptions).toHaveBeenCalledOnce();
      expect(transcriptionStore.items).toEqual([]);
    });

    it('does not clear on backend error and logs to console.error', async () => {
      const consoleSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
      transcriptionStore.items = [makeTranscription({ id: '1' })];
      mockedClearTranscriptions.mockRejectedValueOnce(new Error('fail'));

      await transcriptionStore.clear();

      expect(transcriptionStore.items).toHaveLength(1);
      expect(consoleSpy).toHaveBeenCalledWith('Failed to clear transcriptions:', expect.any(Error));
    });
  });
});
```

### 4.7 npm Scripts

Add to `package.json`:

```json
{
  "scripts": {
    "test": "vitest run",
    "test:watch": "vitest"
  }
}
```

- `npm test` (or `npm run test`) -- single run, exits with code 0/1. Suitable for CI.
- `npm run test:watch` -- interactive watch mode for development.

## 5. Edge Cases

### 5.1 Svelte 5 Rune Compilation in Tests

**Problem:** `.svelte.ts` files use `$state()` syntax that is not valid TypeScript. Vitest must route these files through the Svelte compiler.

**Solution:** The `@sveltejs/vite-plugin-svelte` in `vitest.config.ts` handles this. The plugin recognizes `.svelte.ts` as a Svelte module and compiles it. Test files that import these modules get the compiled output.

**Risk:** If a future Vitest or Svelte version changes how `.svelte.ts` files are resolved, tests may fail to compile. Pinning `@sveltejs/vite-plugin-svelte` to the `^7.x` range (matching the existing version in `package.json`) mitigates this.

### 5.2 Singleton Store State Leaking Between Tests

**Problem:** The stores (`recordingStore`, `settingsStore`, `transcriptionStore`) are module-level singletons. State mutations in one test leak into the next.

**Solution:** Every test suite includes a `beforeEach` that resets the store to its initial state. For `RecordingStore`, calling `reset()` suffices. For `SettingsStore` and `TranscriptionStore`, which lack a comprehensive `reset()` method, individual fields are manually reset in `beforeEach`.

**Alternative considered:** Using `vi.resetModules()` to re-import the store module for each test. This is cleaner in theory but significantly slower and can cause issues with mock scoping. Manual reset is sufficient for these small stores.

### 5.3 Time-Dependent Tests

**Problem:** `formatRelativeTime` compares the input date against `Date.now()`. Without controlling time, tests are non-deterministic.

**Solution:** Use `vi.useFakeTimers()` and `vi.setSystemTime()` to freeze `Date.now()` at a known value. `afterEach` restores real timers to prevent leakage.

### 5.4 `@tauri-apps/api/core` Not Available Outside Tauri

**Problem:** The `@tauri-apps/api/core` package is designed to run inside a Tauri webview. Importing it in a Node/jsdom environment without mocking will fail because it tries to access `window.__TAURI_INTERNALS__`.

**Solution:** Module-level `vi.mock('@tauri-apps/api/core', ...)` intercepts the import before any code tries to access Tauri internals. This mock must appear before the import of any module that depends on it.

### 5.5 `resolve.conditions: ['browser']` Side Effects

**Problem:** Setting `resolve.conditions: ['browser']` in Vitest config changes how *all* packages resolve their exports. Some test-only packages might not have browser-specific exports.

**Solution:** This is the standard configuration for Svelte 5 with Vitest. The Svelte package explicitly provides `browser` and `default` export conditions. jsdom provides the DOM globals that the browser bundle expects. If a future test dependency has issues, the condition can be moved to a per-file override via `// @vitest-environment` comments.

### 5.6 Concurrent Test Execution and Shared Singletons

**Problem:** Vitest runs test files in parallel by default. Since stores are singletons within a module, two test files importing the same store could interfere.

**Solution:** Each test file imports from a separate module scope (Vitest isolates modules per file by default with `isolate: true`). The singleton is unique per test file, not shared across files. No additional configuration is needed.

## 6. File Changes

| File | Action | Purpose |
|------|--------|---------|
| `vitest.config.ts` | Create | Vitest configuration with Svelte plugin, jsdom, browser conditions |
| `src/lib/utils/format.test.ts` | Create | Tests for `formatDuration`, `formatRelativeTime`, `truncateText` |
| `src/lib/utils/tauri.test.ts` | Create | Tests for Tauri API wrapper functions (mock `invoke`) |
| `src/lib/stores/recording.svelte.test.ts` | Create | Tests for RecordingStore state transitions and computed getters |
| `src/lib/stores/settings.svelte.test.ts` | Create | Tests for SettingsStore load/save/update with mocked backend |
| `src/lib/stores/transcriptions.svelte.test.ts` | Create | Tests for TranscriptionStore CRUD with mocked backend |
| `package.json` | Modify | Add `vitest`, `jsdom` to devDependencies; add `test` and `test:watch` scripts |
| `tsconfig.app.json` | Modify | Add `*.test.ts` and `*.svelte.test.ts` to the `include` glob (optional -- Vitest uses its own tsconfig, but this helps IDE support) |

### Files NOT Changed

| File | Reason |
|------|--------|
| `vite.config.ts` | Vitest config is separate to avoid polluting the app build |
| `svelte.config.js` | No changes needed; the Vitest config creates its own Svelte plugin instance |
| `src/lib/utils/format.ts` | Source code is not modified; tests are additive |
| `src/lib/stores/*.svelte.ts` | Source code is not modified; tests are additive |

## 7. Testing Strategy

This spec *is* the testing strategy for the frontend. Verification that the spec itself is correctly implemented:

### 7.1 Verify Vitest Runs

```bash
npm test 2>&1 | tee /tmp/vitest-output.txt
```

Expected: all test files discovered, all tests pass, exit code 0.

### 7.2 Verify Test Coverage Categories

| Category | Files | Min Tests | Mocking Required |
|----------|-------|-----------|------------------|
| Pure utilities | `format.test.ts` | 15+ | `vi.useFakeTimers()` only |
| Tauri wrappers | `tauri.test.ts` | 34+ | `vi.mock('@tauri-apps/api/core')` |
| Recording store | `recording.svelte.test.ts` | 20+ | None |
| Settings store | `settings.svelte.test.ts` | 8+ | `vi.mock('../utils/tauri')` |
| Transcriptions store | `transcriptions.svelte.test.ts` | 8+ | `vi.mock('../utils/tauri')` |

### 7.3 Verify No Test Pollution

Run tests in isolation to confirm no inter-test dependencies:

```bash
npm test -- --reporter=verbose 2>&1 | tee /tmp/vitest-verbose.txt
```

Then run a single file to confirm it passes independently:

```bash
npm test -- src/lib/utils/format.test.ts
npm test -- src/lib/stores/recording.svelte.test.ts
```

### 7.4 Negative Verification

Introduce a deliberate failure (e.g., change an expected value) and confirm the test fails. This validates that tests are actually asserting behavior and not silently passing.

## 8. Security Considerations

This spec adds test infrastructure only. No security impact.

- **No secrets in test files.** Tests use hardcoded mock data, not real API keys or tokens.
- **No new runtime dependencies.** `vitest` and `jsdom` are devDependencies only; they are not bundled into the production `.app`.
- **Mock boundaries prevent real IPC.** All Tauri `invoke` calls are mocked. Tests never communicate with the Rust backend or access system resources.

## 9. Cost Analysis

### 9.1 Dependencies Added

| Package | Size (approx) | Purpose |
|---------|---------------|---------|
| `vitest` | ~3 MB (node_modules) | Test runner |
| `jsdom` | ~8 MB (node_modules) | DOM environment |

These are devDependencies and do not affect the production bundle size.

### 9.2 CI Impact

- **Added time:** ~5-10 seconds for the test suite (pure logic tests with no browser or network IO).
- **No new CI job required yet.** Tests can be added to the existing build workflow as an additional step, or run in a separate job. CI configuration is covered by Phase 5 of the testing initiative.

### 9.3 Maintenance Cost

- **Low.** The tests mirror the source code structure. When a utility function or store method changes, the corresponding test file is the obvious place to update.
- **Rune store tests are coupled to Svelte 5's compilation model.** A major Svelte version upgrade (e.g., Svelte 6) could require test infrastructure changes. This is an accepted cost -- the alternative (no tests) has higher ongoing cost.

### 9.4 Developer Experience

- `npm run test:watch` provides sub-second feedback during development.
- Co-located test files make it easy to find and update tests alongside source changes.
- No new tooling to learn beyond standard Vitest API.

## 10. Implementation Tasks

Tasks are ordered by dependency. Each task is independently committable.

- [ ] **Task 1: Install dependencies and create Vitest config**
  - `npm install --save-dev vitest@^4.1.0 jsdom` (Vitest 4.1.x+ is required for Vite 8 compatibility; 4.0.x has `vite` as a direct dependency rather than a peer dependency and does not support Vite 8's plugin API)
  - Create `vitest.config.ts` with Svelte plugin, jsdom environment, browser conditions
  - Add `"test": "vitest run"` and `"test:watch": "vitest"` scripts to `package.json`
  - Verify: `npm test` runs and reports "no test files found" (no tests exist yet)

- [ ] **Task 2: Add `format.ts` utility tests**
  - Create `src/lib/utils/format.test.ts`
  - Implement all `formatDuration` tests (zero, seconds, minutes, large values, sub-second rounding, negative)
  - Implement all `formatRelativeTime` tests (just now, seconds, minutes, hours, yesterday, days, month+day, invalid)
  - Implement all `truncateText` tests (short, exact, long, whitespace trim, zero, empty)
  - Verify: `npm test` passes with 15+ tests

- [ ] **Task 3: Add `tauri.ts` wrapper tests**
  - Create `src/lib/utils/tauri.test.ts`
  - Mock `@tauri-apps/api/core` at module level
  - Test all 34 wrapper functions (one test per function verifying command name and argument shape):
    - Recording: `startRecording`, `stopRecording`, `cancelRecording`
    - Transcriptions: `getTranscriptions`, `getLastTranscription`, `deleteTranscription`, `clearTranscriptions`, `exportTranscriptionsCsv`
    - Settings: `getSettings`, `updateSettings`
    - Permissions: `checkMicrophonePermission`, `checkAccessibilityPermission`, `requestAccessibilityPermission`, `requestMicrophonePermission`, `checkAllPermissions`, `openAccessibilitySettings`, `openMicrophoneSettings`
    - Setup/onboarding: `getAsrBackend`, `getModelStatus`, `needsOnboarding`, `initAsr`, `downloadModel`, `completeSetup`
    - LLM: `getLlmStatus`, `checkLlmUpdate`, `downloadLlmModel`, `updateLlmModel`, `cancelLlmDownload`, `deleteLlmModel`, `loadLlmModel`, `unloadLlmModel`
    - App updates: `checkAppUpdate`, `performAppUpdate`, `getUpdateStatus`
  - Verify: `npm test` passes with all wrapper tests

- [ ] **Task 4: Add `RecordingStore` tests**
  - Create `src/lib/stores/recording.svelte.test.ts`
  - Test initial state (after reset)
  - Test `start()`, `stop()`, `cancel()`, `reset()` transitions
  - Test `setState()` for all `AppStateEnum` values
  - Test `setAudioLevel()` clamping and bar level shifting
  - Test computed getters (`isTranscribing`, `isPasting`, `isActive`)
  - Verify: `npm test` passes with all store tests

- [ ] **Task 5: Add `SettingsStore` tests**
  - Create `src/lib/stores/settings.svelte.test.ts`
  - Mock `../utils/tauri` wrapper functions
  - Test initial state with DEFAULT_SETTINGS
  - Test `load()` success (merge with defaults) and failure (fallback to defaults)
  - Test `save()` success, saving flag lifecycle, and error handling
  - Test `update()` immutability and field preservation
  - Verify: `npm test` passes

- [ ] **Task 6: Add `TranscriptionStore` tests**
  - Create `src/lib/stores/transcriptions.svelte.test.ts`
  - Mock `../utils/tauri` wrapper functions
  - Test initial state and `last` getter
  - Test `load()` success and error behavior
  - Test `add()` prepend behavior
  - Test `delete()` success and error behavior
  - Test `clear()` success and error behavior
  - Verify: `npm test` passes

- [ ] **Task 7: Final verification**
  - Run full suite: `npm test 2>&1 | tee /tmp/vitest-final.txt`
  - Verify test count is 85+ across all files
  - Run `npm run build` to confirm test infrastructure does not affect production build
  - Run `npm run check` to confirm TypeScript types are clean
