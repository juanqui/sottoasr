# SottoASR Application Audit Report

- **Version:** 1.0
- **Date:** 2026-03-29
- **Status:** Draft
- **Review Passes:** 0/5 completed

---

## Table of Contents

1. [Summary](#1-summary)
2. [Critical Issues](#2-critical-issues)
3. [Security Issues](#3-security-issues)
4. [Performance Issues](#4-performance-issues)
5. [Memory & Resource Issues](#5-memory--resource-issues)
6. [UX/Usability Issues](#6-uxusability-issues)
7. [Reliability Issues](#7-reliability-issues)
8. [Code Quality Issues](#8-code-quality-issues)
9. [Testing Gaps](#9-testing-gaps)
10. [Documentation Issues](#10-documentation-issues)
11. [Summary & Prioritization](#11-summary--prioritization)

---

## 1. Summary

This audit covers the SottoASR application — a local, privacy-first speech-to-text app for macOS built with Tauri v2 (Rust backend) and Svelte 5 (frontend). The application uses Apple Neural Engine via FluidAudio for ASR, with optional LLM cleanup via Qwen3.5 running on MLX.

The codebase is generally well-structured with good separation of concerns. However, several significant issues were identified that require attention, ranging from memory leaks and potential panics to UX gaps and testing deficiencies.

**Key Technologies:**
- Tauri v2 (Rust backend, WebView frontend)
- Svelte 5 with Runes ($state, $derived, $effect)
- FluidAudio via fluidaudio-rs (CoreML/ANE) for ASR
- parakeet-rs (ONNX) as cross-platform fallback
- Python MLX sidecar for LLM cleanup
- cpal for audio capture
- CGEvent API for paste simulation

---

## 2. Critical Issues

### C-001: Audio Buffer Unbounded Growth Potential

**Location:** `src-tauri/src/audio/capture.rs:51-84`

**Issue:** The `level_buffer` in the cpal callback grows via `extend_from_slice` without bounds checking. While there's a `MAX_AUDIO_BUFFER_SAMPLES` constant, it only applies to the total recording buffer, not the per-callback level buffer.

```rust
let mut level_buffer = Vec::with_capacity(level_window);
// ...
level_buffer.extend_from_slice(&mono);
if level_buffer.len() >= level_window {
```

If the callback is called with chunks larger than expected, `level_buffer` could grow unexpectedly within a single callback invocation.

**Severity:** Medium
**Likely Impact:** Memory exhaustion during long recordings on systems with high sample rates

---

### C-002: Potential Panic in CGEventTap Callback

**Location:** `src-tauri/src/commands/keycapture.rs:106`

**Issue:** The callback dereferences a raw pointer without null check:
```rust
let app = &*(user_info as *const AppHandle);
```

If `user_info` is null (shouldn't happen given how it's set up), this would panic.

**Severity:** Medium
**Likely Impact:** App crash when key capture is active

---

### C-003: No Timeout on ASR Engine Lock

**Location:** `src-tauri/src/hotkeys/manager.rs:449`

**Issue:** The ASR engine mutex is locked without a timeout:
```rust
let mut engine = state.asr_engine.lock().await;
```

If the ASR engine is in a bad state (e.g., deadlock in internal operations), this will block indefinitely.

**Severity:** Medium
**Likely Impact:** App hang during transcription

---

### C-004: LLM Engine Drop Called During Mutex Unwinding

**Location:** `src-tauri/src/llm/engine.rs:199-203`

**Issue:** The `Drop` implementation for `LlmEngine` calls `self.quit()` which involves blocking I/O. If the `LlmEngine` is dropped while holding a lock that another async task is waiting on, this can cause deadlock.

```rust
impl Drop for LlmEngine {
    fn drop(&mut self) {
        self.quit();
    }
}
```

**Severity:** Medium
**Likely Impact:** Deadlock when LLM engine is dropped during async operations

---

### C-005: Shortcut Recorder Commit Timeout Not Cancelled on Component Destroy

**Location:** `src/lib/components/shortcut-recorder.svelte:360-374`

**Issue:** In `onDestroy`, the `commitTimeoutId` timeout is cleared and recording is stopped, but if `stopRecording()` fails or the async cleanup doesn't complete before the component is destroyed, the timeout callback could fire on a destroyed component.

```rust
onDestroy(() => {
    if (commitTimeoutId !== null) {
        clearTimeout(commitTimeoutId);
        commitTimeoutId = null;
    }
    if (recording) {
        invoke('stop_key_capture').catch(() => {});
    }
    // ... listeners
});
```

**Severity:** Low
**Likely Impact:** Memory leak or state inconsistency

---

### C-006: CGEventTap Thread May Leak on Crash

**Location:** `src-tauri/src/commands/keycapture.rs:252-257`

**Issue:** The `CFRunLoopRun()` blocks indefinitely. If the app crashes, the thread may not be cleaned up properly:
```rust
// Block in the run loop. CFRunLoopRun only returns if all sources
// are removed, which means the tap was invalidated by the system.
CFRunLoopRun();
```

The thread spawn loop in `init_key_capture_thread` does restart on tap death, but abrupt termination could leave orphaned threads.

**Severity:** Low
**Likely Impact:** Resource leak over repeated crash cycles

---

## 3. Security Issues

### S-001: CGEventTap Captures All Key Events

**Location:** `src-tauri/src/commands/keycapture.rs:90-93`

**Issue:** The event mask captures ALL key events including potentially sensitive data:
```rust
let event_mask: u64 = (1 << 10)  // kCGEventKeyDown
    | (1 << 11)  // kCGEventKeyUp
    | (1 << 12)  // kCGEventFlagsChanged
    | (1 << 14); // NX_SYSDEFINED (media keys)
```

While the code only processes key codes (not actual keystroke data), the tap still receives all keyboard events which could theoretically be logged or intercepted if the tap is compromised.

**Severity:** Low
**Likely Impact:** Information disclosure if tap is compromised

---

### S-002: No Input Sanitization on Transcribed Text

**Location:** `src-tauri/src/hotkeys/manager.rs:609-613`

**Issue:** Transcribed text is copied to clipboard and pasted without sanitization:
```rust
let paste_result = if restore_clipboard {
    crate::paste::paste_text_and_restore(&final_text, target_pid)
} else {
    crate::paste::paste_text(&final_text, target_pid)
};
```

While modern apps handle Unicode properly, malformed output from the ASR could potentially cause issues in some applications.

**Severity:** Low
**Likely Impact:** Unexpected behavior in target applications

---

### S-003: Accessibility Permission Not Re-Checked Before Paste

**Location:** `src-tauri/src/paste/macos.rs:17-30`

**Issue:** Permission is checked at the start of `paste_text_inner` but not re-verified before the actual paste operation. If permission is revoked mid-operation, the paste would still be attempted:
```rust
if !is_accessibility_trusted() {
    return Err("Accessibility permission not granted...".into());
}
// ... time passes ...
simulate_cmd_v()?; // No re-check
```

**Severity:** Low
**Likely Impact:** Paste failure if permission revoked during operation

---

### S-004: No Rate Limiting on LLM Cleanup

**Location:** `src-tauri/src/hotkeys/manager.rs:482-560`

**Issue:** LLM cleanup is triggered without any rate limiting. If the user rapidly starts/stops recordings with LLM enabled, multiple cleanup operations could be queued, potentially causing resource exhaustion.

**Severity:** Low
**Likely Impact:** Resource exhaustion (memory/CPU) from multiple concurrent LLM cleanups

---

## 4. Performance Issues

### P-001: Audio Level Logging in Production

**Location:** `src-tauri/src/audio/capture.rs:79-82`

**Issue:** Audio levels are logged every ~1 second in production:
```rust
if level_emit_count % 30 == 1 {
    log::info!("Audio level: {:.4} (emit #{})", rms, level_emit_count);
}
```

This logging could impact performance, especially with high-frequency audio callbacks.

**Severity:** Low
**Likely Impact:** Performance degradation, log file bloat

---

### P-002: Waveform Animation Runs in Background Tabs

**Location:** `src/lib/components/waveform.svelte:126-128`

**Issue:** The `requestAnimationFrame` loop doesn't pause when the tab is in the background:
```rust
onMount(() => {
    animFrameId = requestAnimationFrame(render);
});
```

**Severity:** Low
**Likely Impact:** Unnecessary CPU usage when app is not visible

---

### P-003: Transcription History Loads All Items at Once

**Location:** `src/lib/components/history-view.svelte:13-21`

**Issue:** All transcriptions are loaded into memory at once without pagination or virtualization:
```rust
let filteredItems = $derived(
    searchQuery.trim()
        ? transcriptionStore.items.filter((item) => { ... })
        : transcriptionStore.items
);
```

With 500 max items and potentially large text fields, this could be slow.

**Severity:** Low
**Likely Impact:** Slow load time for history window with many entries

---

### P-004: LLM Model Not Cached Between Cleanups

**Location:** `src-tauri/src/hotkeys/manager.rs:496-527`

**Issue:** The LLM model is loaded from disk on every cleanup if not already running. While there's a `needs_spawn` check, the model must be loaded via `load_model()` after spawning, which adds latency:
```rust
match tokio::task::spawn_blocking(move || {
    crate::llm::engine::LlmEngine::spawn_with_model(&model_id_for_spawn)
}).await {
    Ok(Ok(engine)) => { *llm_guard = Some(engine); }
    // ... then load_model() is called separately
}
```

**Severity:** Medium
**Likely Impact:** Slow LLM cleanup on first use after app start

---

### P-005: Python Sidecar Startup Overhead

**Location:** `src-tauri/src/llm/engine.rs:29-60`

**Issue:** The Python sidecar imports `mlx_lm` on startup, which is slow (~2-5 seconds). This happens even if LLM cleanup is rarely used:
```rust
let mut child = Command::new(&python)
    .arg(&sidecar_path)
    .args(["--model", model_id])
    // ...
    .spawn()
```

**Severity:** Medium
**Likely Impact:** Delay when first LLM cleanup is triggered

---

### P-006: Dead Code - Audio Resampling Module

**Location:** `src-tauri/src/audio/resample.rs:1-60`

**Issue:** The entire `resample_to_16khz` function is marked `#[allow(dead_code)]` and never called. FluidAudio handles resampling internally, and parakeet-rs handles it internally as well.

**Severity:** Low
**Likely Impact:** Unnecessary code complexity and binary size

---

## 5. Memory & Resource Issues

### M-001: Potential Memory Leak in Event Listeners

**Location:** `src/lib/components/overlay-pill.svelte:85-154`

**Issue:** Event listeners created in `onMount` are stored in an array and cleaned up on return, but if the component is destroyed while recording, there could be a brief window where events arrive to a destroyed component:
```rust
const unlisteners: Array<() => void> = [];
// ...
listen('state-changed', (event) => { ... }).then((u) => unlisteners.push(u));
// ...
return () => {
    unlisteners.forEach((fn) => fn());
};
```

**Severity:** Low
**Likely Impact:** Memory leak for event listeners

---

### M-002: Transcription Store Could Grow Without Limit

**Location:** `src/lib/stores/transcriptions.svelte.ts:32-34`

**Issue:** While the backend limits to 5000 items (`src-tauri/src/commands/transcription.rs:97-98`), the frontend store has no limit enforcement and could grow beyond what the backend will eventually persist:
```rust
add(transcription: Transcription) {
    this.items = [transcription, ...this.items];
}
```

**Severity:** Low
**Likely Impact:** Memory exhaustion for frontend if many transcriptions arrive

---

### M-003: Audio Samples Channel Could Block

**Location:** `src-tauri/src/state.rs:18-19`

**Issue:** The audio sender uses an unbounded channel. If the receiver is slow to process samples (e.g., during heavy ASR processing), memory could grow unboundedly:
```rust
pub audio_sender: StdMutex<std::sync::mpsc::Sender<Vec<f32>>>,
pub audio_receiver: StdMutex<std::sync::mpsc::Receiver<Vec<f32>>>,
```

**Severity:** Medium
**Likely Impact:** Memory exhaustion during long recordings

---

### M-004: Clipboard Restore Thread Not Guaranteed to Complete

**Location:** `src-tauri/src/paste/macos.rs:73-81`

**Issue:** The clipboard restore runs in a spawned thread with no guarantee it will complete if the app exits:
```rust
std::thread::spawn(move || {
    std::thread::sleep(std::time::Duration::from_millis(500));
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        let _ = clipboard.set_text(&original);
    }
});
```

**Severity:** Low
**Likely Impact:** User's clipboard may not be restored if app exits unexpectedly

---

## 6. UX/Usability Issues

### U-001: Short Recording Discarded Silently

**Location:** `src-tauri/src/hotkeys/manager.rs:394-399`

**Issue:** Recordings with fewer than 4000 samples are discarded without user notification:
```rust
if samples.len() < 4000 {
    log::warn!("Recording too short ({} samples), discarding", samples.len());
    state.set_state(AppStateEnum::Idle);
    return;
}
```

The user sees the overlay disappear without explanation.

**Severity:** Medium
**Likely Impact:** User confusion when brief recordings disappear

---

### U-002: Overlay Not Respected in Some Scenarios

**Location:** `src-tauri/src/hotkeys/manager.rs:288-289`

**Issue:** The overlay is shown unconditionally in `handle_start_recording` regardless of the `show_overlay` setting in settings. The setting is only checked in the frontend, not the backend:
```rust
// Register cancel shortcut (only active while recording)
register_cancel_shortcut(app);

// Show the overlay window
show_overlay(app);
```

**Severity:** Low
**Likely Impact:** Overlay appears even when user disabled it

---

### U-003: No Keyboard Navigation in Settings

**Location:** `src/lib/components/settings-panel.svelte`

**Issue:** The settings panel lacks keyboard navigation support (Tab, Enter, Escape). Users must use mouse exclusively.

**Severity:** Low
**Likely Impact:** Poor accessibility, inconvenient power users

---

### U-004: Recording Timer Shows Total Time Including Transcription

**Location:** `src/lib/components/recording-timer.svelte`

**Issue:** The timer starts when recording begins but doesn't stop visually when transcription starts. The `running` prop controls it, but by the time `Transcribing` state is emitted, the overlay is already showing.

**Severity:** Low
**Likely Impact:** User sees timer "freeze" at the last recording time

---

### U-005: No Undo for Deleted Transcriptions

**Location:** `src/lib/components/history-item.svelte:62-64`

**Issue:** When a transcription is deleted, it's permanently removed with no recovery option:
```rust
function handleDelete() {
    ondelete(item.id);
}
```

**Severity:** Low
**Likely Impact:** Accidental deletion with no recourse

---

### U-006: No Export Format Options for CSV

**Location:** `src-tauri/src/commands/transcription.rs:76-91`

**Issue:** The CSV export format doesn't include `cancelled` field and has inconsistent formatting:
```rust
csv.push_str(&format!(
    "{},{}.,{},{},{},\"{}\",\"{}\"\n",  // Note the stray "." after created_at
    t.id, t.created_at, t.duration_ms, t.word_count, t.llm_applied,
    text_escaped, raw_escaped,
));
```

**Severity:** Low
**Likely Impact:** Data loss in exports, malformed CSV

---

### U-007: No Visual Indicator of LLM Cleanup in History

**Location:** `src/lib/components/history-item.svelte`

**Issue:** When viewing a cleaned transcription, there's no clear indication that LLM cleanup was applied without expanding to see the diff.

**Severity:** Low
**Likely Impact:** User confusion about transcription origin

---

### U-008: Settings Discard Requires Page Reload

**Location:** `src/lib/components/settings-panel.svelte:165-168`

**Issue:** When discard is clicked, settings are reverted in the store but the form's bound values aren't re-bound until the next load:
```rust
function handleDiscard() {
    settingsStore.current = JSON.parse(savedSnapshot);
    // The UI bindings still show old values until parent re-renders
}
```

**Severity:** Low
**Likely Impact:** Inconsistent UI state temporarily

---

### U-009: Permission Check Not Real-Time

**Location:** `src/lib/components/settings-panel.svelte:105-122`

**Issue:** Permissions are only checked when the settings page loads and when explicitly refreshed. If permission is revoked while settings is open, no indication is given.

**Severity:** Low
**Likely Impact:** User may not know permission was revoked without reopening settings

---

## 7. Reliability Issues

### R-001: No Retry on Model Download Failure

**Location:** `src-tauri/src/asr/model.rs:131-217`

**Issue:** If the model download fails partway through, there's no retry mechanism. The user must manually retry:
```rust
let response = client.get(&url).send().await
    .map_err(|e| format!("Download failed for {}: {}", filename, e))?;
```

**Severity:** Medium
**Likely Impact:** Failed downloads require manual restart

---

### R-002: Hotkey Re-registration Could Conflict with Active Recording

**Location:** `src-tauri/src/commands/settings.rs:81-112`

**Issue:** When shortcuts are re-applied (e.g., from settings change), all shortcuts are unregistered and re-registered. If this happens during a recording session, it could cause issues:
```rust
pub async fn apply_shortcuts(...) {
    let app_clone = app.clone();
    app.run_on_main_thread(move || {
        match crate::hotkeys::manager::register_shortcuts(...) {
```

**Severity:** Low
**Likely Impact:** Unexpected behavior if shortcuts change during recording

---

### R-003: CGEvent Pipeline Warmup May Fail Silently

**Location:** `src-tauri/src/paste/macos.rs:155-169`

**Issue:** The `warmup_cgevent_pipeline()` uses an invalid keycode (0xFF) which won't produce visible effects but could fail silently on some macOS versions:
```rust
if let Ok(event) = CGEvent::new_keyboard_event(source, 0xFF, false) {
    event.post(CGEventTapLocation::HID);
```

**Severity:** Low
**Likely Impact:** Paste may not work on first try on some systems

---

### R-004: Accessibility Functional Check Can Give False Negatives

**Location:** `src-tauri/src/paste/macos.rs:96-148`

**Issue:** The functional accessibility check uses `AXUIElementCopyAttributeValue` which can return errors even when accessibility is granted, especially from background threads:
```rust
let ok = result == 0 || result == -25205;
if !ok {
    log::warn!("AX functional check returned error code: {}", result);
}
```

**Severity:** Medium
**Likely Impact:** Users may see "needs restart" warning when restart doesn't help

---

### R-005: No Handling for Concurrent Recording Attempts

**Location:** `src-tauri/src/hotkeys/manager.rs:237-244`

**Issue:** If the hotkey is pressed rapidly multiple times, `handle_start_recording` could be called concurrently even though there's a state check:
```rust
if current != AppStateEnum::Idle {
    log::warn!("Cannot start recording: currently in {:?} state", current);
    return;
}
```

The check is advisory - there's no mutex preventing concurrent entry.

**Severity:** Low
**Likely Impact:** Race condition could cause duplicate recording sessions

---

### R-006: LLM Sidecar Process Orphaning

**Location:** `src-tauri/src/llm/engine.rs:142-174`

**Issue:** If the Rust process crashes or is killed, the Python sidecar may continue running, orphaned:
```rust
pub fn quit(&mut self) {
    let _ = self.request(&serde_json::json!({"action": "quit"}));
    // Wait with timeout, then kill
}
```

The timeout is 3 seconds, after which it kills the process. But if the parent crashes before calling `quit()`, the sidecar lives on.

**Severity:** Low
**Likely Impact:** Resource leak, stale processes

---

### R-007: State Transitions Not Atomic

**Location:** `src-tauri/src/state.rs:75-83`

**Issue:** State transitions use a simple mutex lock without atomic compare-and-swap:
```rust
pub fn set_state(&self, new_state: AppStateEnum) {
    if let Ok(mut state) = self.current_state.lock() {
        *state = new_state;
    }
}
```

Concurrent transitions could interleave.

**Severity:** Low
**Likely Impact:** Race condition in state machine

---

## 8. Code Quality Issues

### Q-001: Magic Numbers Without Constants

**Location:** Multiple files

**Issue:** Various magic numbers scattered throughout:
- 4000 samples minimum (`manager.rs:394`)
- 4000 samples too short threshold
- 30fps for audio level emission (`capture.rs:79`)
- 500ms clipboard restore delay (`macos.rs:76`)
- 30 second LLM timeout (`manager.rs:533`)

**Severity:** Low
**Likely Impact:** Hard to maintain, inconsistent values

---

### Q-002: Missing Error Propagation in Some Cases

**Location:** `src-tauri/src/hotkeys/manager.rs:328-331`

**Issue:** Some errors are logged but not propagated to the caller:
```rust
Err(e) => {
    log::error!("Failed to start audio capture: {}", e);
    state.is_recording.store(false, Ordering::SeqCst);
    let _ = app.emit("recording-error", serde_json::json!({ "error": e }));
}
```

**Severity:** Low
**Likely Impact:** Silent failures make debugging harder

---

### Q-003: Inconsistent Error Handling Patterns

**Location:** Multiple files

**Issue:** Some functions return `Result<(), String>` with detailed errors, others return `()` and log errors. This inconsistency makes it hard to know what errors to handle.

**Severity:** Low
**Likely Impact:** Inconsistent user-facing error messages

---

### Q-004: Dead Code in Onboarding

**Location:** `src/lib/components/onboarding-view.svelte`

**Issue:** The `permissionCheckTimeout` is created but the polling function `checkPermissions` doesn't use it consistently.

**Severity:** Low
**Likely Impact:** Confusing code, potential bugs

---

### Q-005: No Validation of Settings Values

**Location:** `src/lib/utils/tauri.ts` and `src-tauri/src/models.rs`

**Issue:** Settings are accepted from the frontend without validation:
```rust
pub max_history: usize,  // Could be 0, causing division by zero later
```

**Severity:** Medium
**Likely Impact:** Crash or unexpected behavior with invalid settings

---

## 9. Testing Gaps

### T-001: No Unit Tests for Core Logic

**Issue:** The codebase has no `#[test]` modules in the Rust code and no test files in the frontend. Critical paths like state machine transitions, audio buffer handling, and paste logic have no automated tests.

**Severity:** High
**Likely Impact:** Regressions undetected, bugs in edge cases

---

### T-002: No Integration Tests

**Issue:** There's no testing infrastructure for integration tests that exercise the full recording→transcription→paste flow.

**Severity:** High
**Likely Impact:** User-facing bugs in multi-step flows

---

### T-003: No Frontend Tests

**Issue:** The Svelte components have no tests. Critical UI logic like the shortcut recorder, waveform animation, and state transitions aren't tested.

**Severity:** High
**Likely Impact:** UI bugs undetected

---

### T-004: No Performance/Benchmark Tests

**Issue:** No benchmarks for audio processing latency, transcription speed, or memory usage under load.

**Severity:** Medium
**Likely Impact:** Performance regressions undetected

---

### T-005: No Model Download Resume Testing

**Issue:** The model download logic handles existing files correctly but there's no test for resume after network interruption.

**Severity:** Medium
**Likely Impact:** Users may need to re-download large models on flaky connections

---

## 10. Documentation Issues

### D-001: Architecture Doc Outdated

**Location:** `docs/designs/architecture.md`

**Issue:** The architecture doc references "parakeet-rs" as the primary ASR engine, but the actual implementation uses FluidAudio by default. The data models shown in the doc don't match the current implementation (e.g., `CleaningUp` state, `llm_cleanup` fields).

**Severity:** Medium
**Likely Impact:** Developer confusion, wrong assumptions

---

### D-002: No API Documentation

**Issue:** There's no generated documentation for the Tauri command interface. The `models.rs` types are referenced but not formally documented.

**Severity:** Low
**Likely Impact:** Hard for new developers to understand the API

---

### D-003: Inline Comments in Shortcut Recorder

**Location:** `src/lib/components/shortcut-recorder.svelte:1-13`

**Issue:** The component has a detailed comment block explaining the UX flow but the implementation doesn't match all the described behaviors (e.g., no mention of the JS fallback limitation).

**Severity:** Low
**Likely Impact:** Misleading documentation

---

## 11. Summary & Prioritization

### Priority 1 (Critical - Fix Before Release)

| ID | Issue | Fix Difficulty |
|----|-------|----------------|
| T-001 | No unit tests for core logic | High |
| T-002 | No integration tests | High |
| T-003 | No frontend tests | High |
| C-003 | No timeout on ASR engine lock | Medium |
| C-004 | LLM Engine Drop deadlock potential | Medium |

### Priority 2 (Important - Fix Soon)

| ID | Issue | Fix Difficulty |
|----|-------|----------------|
| M-003 | Unbounded audio channel | Medium |
| R-001 | No retry on model download | Low |
| R-004 | Accessibility check false negatives | Medium |
| S-004 | No rate limiting on LLM cleanup | Low |
| Q-005 | No settings validation | Low |
| C-002 | Potential panic in CGEventTap | Low |

### Priority 3 (Nice to Fix)

| ID | Issue | Fix Difficulty |
|----|-------|----------------|
| U-001 | Short recording discarded silently | Low |
| U-002 | Overlay not respecting setting | Low |
| P-004 | LLM model not cached | Medium |
| P-005 | Python sidecar startup overhead | Medium |
| D-001 | Architecture doc outdated | Low |
| C-001 | Audio buffer growth potential | Low |

---

## Appendix: File Coverage

| File | Lines | Issues Found |
|------|-------|---------------|
| `src-tauri/src/lib.rs` | 208 | 0 |
| `src-tauri/src/state.rs` | 84 | 1 (R-007) |
| `src-tauri/src/models.rs` | 109 | 1 (Q-005) |
| `src-tauri/src/llm/engine.rs` | 327 | 2 (C-004, P-005) |
| `src-tauri/src/llm/prompts.rs` | 54 | 0 |
| `src-tauri/src/llm/download.rs` | 65 | 0 |
| `src-tauri/src/audio/capture.rs` | 115 | 3 (C-001, P-001, M-001) |
| `src-tauri/src/audio/resample.rs` | 60 | 1 (P-006) |
| `src-tauri/src/asr/engine.rs` | 79 | 0 |
| `src-tauri/src/asr/fluidaudio_backend.rs` | 144 | 0 |
| `src-tauri/src/asr/parakeet_backend.rs` | 123 | 0 |
| `src-tauri/src/asr/model.rs` | 217 | 1 (R-001) |
| `src-tauri/src/commands/recording.rs` | 69 | 0 |
| `src-tauri/src/commands/transcription.rs` | 103 | 1 (U-006) |
| `src-tauri/src/commands/settings.rs` | 112 | 1 (R-002) |
| `src-tauri/src/commands/permissions.rs` | 343 | 1 (R-004) |
| `src-tauri/src/commands/setup.rs` | 153 | 0 |
| `src-tauri/src/commands/keycapture.rs` | 329 | 3 (C-002, C-006, S-001) |
| `src-tauri/src/commands/llm.rs` | 140 | 0 |
| `src-tauri/src/hotkeys/manager.rs` | 1065 | 5 (C-003, S-003, S-004, U-001, U-002, R-005) |
| `src-tauri/src/paste/macos.rs` | 320 | 3 (M-004, R-003, S-002) |
| `src-tauri/src/paste/mod.rs` | 46 | 0 |
| `src-tauri/src/tray/menu.rs` | 214 | 0 |
| `src/lib/components/overlay-pill.svelte` | 384 | 1 (M-001) |
| `src/lib/components/waveform.svelte` | 147 | 1 (P-002) |
| `src/lib/components/recording-timer.svelte` | 62 | 1 (U-004) |
| `src/lib/components/settings-panel.svelte` | 993 | 2 (U-003, U-009) |
| `src/lib/components/history-view.svelte` | 313 | 1 (P-003) |
| `src/lib/components/history-item.svelte` | 332 | 1 (U-005) |
| `src/lib/components/shortcut-recorder.svelte` | 490 | 2 (C-005, D-003) |
| `src/lib/components/onboarding-view.svelte` | 700 | 1 (Q-004) |
| `src/lib/components/about-view.svelte` | 227 | 0 |
| `src/lib/stores/settings.svelte.ts` | 70 | 0 |
| `src/lib/stores/transcriptions.svelte.ts` | 57 | 1 (M-002) |
| `src/lib/stores/recording.svelte.ts` | 97 | 0 |
| `src/lib/utils/tauri.ts` | 206 | 0 |
| `src/lib/utils/format.ts` | 46 | 0 |
| `src-tauri/sidecar/llm_cleanup.py` | 239 | 0 |
| `docs/designs/architecture.md` | ~1300 | 1 (D-001) |

---

*Audit completed on 2026-03-29*
