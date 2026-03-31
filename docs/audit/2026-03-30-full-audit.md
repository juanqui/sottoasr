# SottoASR Full Application Audit

- **Version:** 1.2 (Pass 4)
- **Date:** 2026-03-30
- **Status:** In Review
- **App Version Audited:** 0.3.4
- **Auditor:** Automated (Claude Code, multi-pass)
- **Pass 2 Date:** 2026-03-30
- **Pass 4 Date:** 2026-03-30

## Table of Contents

1. [Executive Summary](#1-executive-summary)
2. [Methodology](#2-methodology)
3. [Critical Findings](#3-critical-findings)
4. [High-Priority Findings](#4-high-priority-findings)
5. [Medium-Priority Findings](#5-medium-priority-findings)
6. [Low-Priority Findings](#6-low-priority-findings)
7. [Informational Notes](#7-informational-notes)
8. [Positive Practices](#8-positive-practices)
9. [Summary Table](#9-summary-table)
10. [Recommendations & Priorities](#10-recommendations--priorities)

---

## 1. Executive Summary

This audit covers the complete SottoASR codebase: Rust backend (Tauri v2), Svelte 5 frontend, CI/CD pipeline, configuration, and infrastructure. Every source file was read and analyzed across multiple independent passes.

**Overall assessment:** The codebase is well-architected and demonstrates strong engineering fundamentals. Error handling, state management, and security boundaries are generally well thought out. However, the audit identified **5 critical**, **13 high**, **24 medium**, and **17 low** priority issues across security, reliability, correctness, and code quality dimensions.

The most impactful findings are:
- Heap allocations inside the real-time audio callback (audio glitches)
- `start_recording` command is broken (sets state but never starts audio)
- State machine can get stuck in Transcribing/CleaningUp after stale job detection
- LLM sidecar lost on cleanup timeout (resource leak)
- App runs without macOS App Sandbox (no filesystem/network containment)
- CSV export corrupts output when transcriptions contain newlines
- Non-atomic file writes risk total transcription history loss on crash
- No input validation on settings updates (untrusted data flows)
- `'unsafe-inline'` in Content Security Policy (weakened XSS protection)
- Zero automated tests across the entire application
- Outdated CI action (`tauri-action@v0`)
- Git dependency on unpinned branch (`tauri-nspanel`)

---

## 2. Methodology

1. **Full code read** of every `.rs`, `.svelte`, `.ts`, `.css`, `.json`, `.yml` file in the project.
2. **Three independent audit passes** (Rust backend, Svelte frontend, config/infrastructure) run in parallel.
3. **External validation** of key findings against current documentation and best-practice sources (Tauri CSP docs, Rust mutex poisoning guides, cpal real-time audio guidelines, tauri-action releases).
4. **Manual verification** of critical findings against actual source code with line-number confirmation.
5. **Three additional review passes** by independent sub-agents to catch missed issues and validate/revise existing findings.
6. **Pass 2 (deep dive):** Complete re-read of all source files with focus on state machine correctness, LLM engine lifecycle, concurrency/lock ordering, error path analysis, and frontend store reactivity. Verified existing findings and identified new issues missed by Pass 1.
7. **Pass 3 (security & edge-case focus):** Security-focused review of all registered IPC commands (argument validation, state preconditions, callable scope), Tauri capabilities/permissions scope, file system operations (atomicity, path traversal, TOCTOU), process spawning (command injection), ObjC FFI safety (ARM64 calling conventions, `objc_msgSend` type casts), macOS entitlements, supply chain dependencies (`Cargo.toml` git refs, `Cargo.lock` pinning), and frontend XSS surface (`{@html}`, `innerHTML`, DOM manipulation).
8. **Pass 4 (frontend/config/build):** Deep dive into Vite configuration, TypeScript configuration, package.json dependency analysis, Svelte component lifecycle audit (every .svelte file), HTML entry points, CSS audit (animations, accessibility, stacking), build output verification, Tauri plugin configuration, and website audit.

---

## 3. Critical Findings

### C-001: Heap Allocation in Real-Time Audio Callback

**File:** `src-tauri/src/audio/capture.rs:60-69`
**Severity:** CRITICAL
**Category:** Performance / Correctness

The cpal audio input callback performs heap allocations on every invocation:

```rust
// Line 60-66: Vec allocation for downmix
let mono: Vec<f32> = if channels > 1 {
    data.chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
} else {
    data.to_vec()  // Also allocates!
};

// Line 69: Another allocation via clone
let _ = sender_clone.send(mono.clone());
```

**Why this matters:** Audio callbacks run on a real-time thread managed by the OS audio subsystem (CoreAudio on macOS). Heap allocations can trigger the system allocator, which may acquire locks, page memory, or call `mmap()`. Any of these can cause the callback to miss its deadline, producing audible glitches (clicks, pops, dropouts). This is a well-documented anti-pattern in real-time audio programming, confirmed by cpal community guidelines and audio engineering literature.

**Impact:** Users may experience intermittent audio artifacts, especially under memory pressure or with higher sample rates. The issue is probabilistic — it may work fine 95% of the time but produce occasional glitches.

**Recommendation:**
- Pre-allocate a reusable buffer outside the callback, pass it in via `Arc<Mutex<Vec<f32>>>` or a lock-free ring buffer.
- Use `data` directly (without copy) when channels == 1.
- Replace `mono.clone()` with a ring buffer write (e.g., `ringbuf` crate) to avoid cloning the entire sample vector.

---

### C-002: Missing Null Check in CGEventTap Callback

**File:** `src-tauri/src/commands/keycapture.rs:106`
**Severity:** CRITICAL
**Category:** Memory Safety

```rust
let app = &*(user_info as *const AppHandle);  // No null check
```

The callback dereferences `user_info` without checking for null. While the pointer is set at line 217 (`Box::into_raw(Box::new(app.clone()))`), macOS can invoke the callback with a null `user_info` in edge cases: permission revocation, system sleep/wake, or if the event tap is invalidated by the OS.

**Impact:** Null pointer dereference causes undefined behavior — likely a segfault crash with no recovery.

**Recommendation:**
```rust
if user_info.is_null() {
    return event;
}
let app = &*(user_info as *const AppHandle);
```

---

### C-003: No Automated Tests

**Files:** Entire codebase
**Severity:** CRITICAL
**Category:** Quality Assurance

The project has zero automated tests — no Rust unit tests, no integration tests, no frontend component tests, no end-to-end tests.

- `src-tauri/src/` contains no `#[cfg(test)]` modules
- `src/` contains no test files
- No test runner is configured (no vitest, no jest)
- CI workflow does not run any test step

**Why this matters:** Without tests, every code change risks introducing regressions that are only discovered by users. The state machine logic (Idle -> Recording -> Transcribing -> CleaningUp -> Idle), hotkey registration, clipboard operations, and transcription persistence are all critical paths with no automated verification.

**Recommendation:**
1. Add Rust unit tests for: state machine transitions, settings validation, audio buffer management, transcription storage
2. Add frontend tests with Vitest + Testing Library for: overlay state transitions, settings form validation, history rendering
3. Add a CI step to run `cargo test` and `npm test`

---

### C-004: Outdated tauri-action@v0 in CI

**File:** `.github/workflows/build-release.yml:75`
**Severity:** CRITICAL
**Category:** CI/CD / Security

```yaml
uses: tauri-apps/tauri-action@v0
```

`tauri-action@v0` is the original 2022 release. The action has had significant updates including security fixes, Tauri v2 support improvements, and bug fixes. Using an outdated action means:
- Missing security patches in action dependencies
- Missing compatibility improvements for newer Tauri versions
- Potential build failures when GitHub runner images update

**Recommendation:** Upgrade to the latest stable version and pin to a specific SHA for security:
```yaml
uses: tauri-apps/tauri-action@v0  # Update to latest tag or pin SHA
```
Verify compatibility with Tauri v2 before upgrading.

---

### C-005: `start_recording` Command Is Broken (Pass 2)

**File:** `src-tauri/src/commands/recording.rs:6-25`
**Severity:** CRITICAL
**Category:** Correctness
**Added in:** Pass 2

```rust
pub async fn start_recording(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let current = state.get_state();
    if current != AppStateEnum::Idle {
        return Err(format!("Cannot start recording: currently in {:?} state", current));
    }

    state.set_state(AppStateEnum::Recording);
    state.is_recording.store(true, std::sync::atomic::Ordering::SeqCst);
    // ...emits events but NEVER starts audio capture, cancel shortcut, overlay, or auto-stop timer
```

The `start_recording` Tauri command sets state to `Recording` and emits events, but it **never actually starts the microphone**. Compare to `handle_start_recording` in `hotkeys/manager.rs` which properly:
1. Starts `audio_capture`
2. Registers cancel shortcut
3. Shows overlay
4. Captures target PID
5. Spawns auto-stop timer

If the frontend ever calls `start_recording` via IPC, the app enters a `Recording` state with no audio capture, no way to stop (cancel shortcut not registered), and no auto-stop timer. The state is permanently stuck.

**Impact:** Any frontend code path that uses `startRecording()` from `tauri.ts` will break the app's state machine until restart.

**Recommendation:** Either:
1. Remove the command entirely and rely solely on hotkey-triggered recording
2. Or make it call `handle_start_recording` from `hotkeys/manager.rs`:
```rust
pub async fn start_recording(app: AppHandle) -> Result<(), String> {
    crate::hotkeys::manager::handle_start_recording(&app);
    Ok(())
}
```

---

## 4. High-Priority Findings

### H-001: No Input Validation on Settings

**File:** `src-tauri/src/commands/settings.rs:62-78`
**Severity:** HIGH
**Category:** Security / Data Integrity

```rust
pub async fn update_settings(
    new_settings: Settings,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut settings = state.settings.lock().await;
    *settings = new_settings.clone();  // No validation
```

The `update_settings` Tauri command accepts a `Settings` struct from the frontend with no validation:
- Shortcut strings could be empty or malformed
- `max_history` could be 0 (breaks truncation logic) or extremely large (memory exhaustion)
- `llm_model_size` could be an invalid model identifier
- No check that shortcuts don't conflict with each other

While the frontend provides basic HTML validation (`min="10" max="10000"`), Tauri commands are directly callable via IPC, bypassing frontend guards.

**Recommendation:** Add a `validate()` method to `Settings` and call it before applying:
```rust
impl Settings {
    fn validate(&self) -> Result<(), String> {
        if self.push_to_talk_shortcut.is_empty() {
            return Err("Push-to-talk shortcut cannot be empty".into());
        }
        if self.max_history < 10 || self.max_history > 10_000 {
            return Err("max_history must be between 10 and 10,000".into());
        }
        // Check shortcut conflicts...
        Ok(())
    }
}
```

---

### H-002: CSP Allows unsafe-inline Styles

**File:** `src-tauri/tauri.conf.json:16`
**Severity:** HIGH
**Category:** Security

```json
"csp": "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; ..."
```

`'unsafe-inline'` in `style-src` allows arbitrary inline styles, which weakens XSS protection. In a Tauri app, the attack surface is smaller than a web app (no external content loading), but if any user-controlled text is rendered with `{@html}` or injected into style attributes, CSS-based data exfiltration becomes possible.

Svelte components do use inline transitions (e.g., `fade`) which may require inline styles. Tauri's official documentation recommends avoiding `'unsafe-inline'` where possible.

**Recommendation:**
- Audit all Svelte components for `{@html}` usage (none found, which is good)
- If inline styles are needed for transitions, use a nonce-based CSP
- Otherwise, remove `'unsafe-inline'` and ensure all styles are in external sheets

---

### H-003: Push-to-Talk Key Release Polling Without Timeout

**File:** `src-tauri/src/hotkeys/manager.rs:95-107`
**Severity:** HIGH
**Category:** Reliability

```rust
std::thread::spawn(move || {
    std::thread::sleep(std::time::Duration::from_millis(100));
    unsafe {
        // ...
        loop {
            std::thread::sleep(std::time::Duration::from_millis(33));
            let still_pressed = CGEventSourceKeyState(0, vk);
            if !still_pressed {
                break;
            }
        }
    }
    // ...
});
```

This loop polls `CGEventSourceKeyState` every 33ms until the key is released. It has no timeout:
- If the key state gets stuck (hardware issue, system sleep while key is pressed), the thread runs forever.
- If the app is shutting down, the thread persists until process termination.
- Each stuck invocation consumes a system thread.

**Recommendation:** Add a timeout (e.g., MAX_RECORDING_SECS + buffer):
```rust
let deadline = std::time::Instant::now() + std::time::Duration::from_secs(MAX_RECORDING_SECS + 10);
loop {
    std::thread::sleep(std::time::Duration::from_millis(33));
    if std::time::Instant::now() > deadline {
        log::warn!("PTT key release polling timed out");
        break;
    }
    // ...
}
```

---

### H-004: Mutex Poisoning Not Handled

**Files:** `src-tauri/src/hotkeys/manager.rs` (lines 64, 66, 177-182, 217-220, 253, 263, 348, 366)
**Severity:** HIGH
**Category:** Reliability

Multiple locations use `.lock().unwrap()` on `std::sync::Mutex`:

```rust
let mut cs = state.cancel_shortcut.lock().unwrap();   // Line 64
let mut capture = state.audio_capture.lock().unwrap(); // Line 263
let rx = state.audio_receiver.lock().unwrap();         // Line 366
```

If any thread panics while holding one of these locks, the mutex becomes poisoned and all subsequent `.unwrap()` calls will panic, cascading the failure across the application.

Note: `register_cancel_shortcut` (line 177-182) does handle poisoning correctly:
```rust
let cancel_shortcut = state.cancel_shortcut.lock()
    .map(|cs| cs.clone())
    .unwrap_or_else(|_| "Escape".to_string());
```

This pattern should be applied consistently.

**Recommendation:** Either:
1. Use `.lock().unwrap_or_else(|e| e.into_inner())` to recover from poisoning
2. Or switch to `parking_lot::Mutex` which doesn't support poisoning (simpler API, better perf)

---

### H-005: Clipboard Restoration Race Condition

**File:** `src-tauri/src/paste/macos.rs:73-82`
**Severity:** HIGH
**Category:** Data Loss

```rust
if let Some(original) = saved_clipboard {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if let Ok(mut clipboard) = arboard::Clipboard::new() {
            let _ = clipboard.set_text(&original);
        }
    });
}
```

After pasting transcribed text, the app restores the original clipboard contents after a blind 500ms delay. If the user copies something new within that 500ms window, their new clipboard content is overwritten with the old content.

**Impact:** User loses clipboard data silently. This is particularly likely when users quickly copy-paste after transcription completes.

**Recommendation:**
- Read the clipboard change count (`NSPasteboard.changeCount`) before and after the delay. Only restore if the count matches (meaning the user hasn't copied anything new).
- Or: store the change count at paste time and compare before restoring.

---

### H-006: CSV Export Has Typo — Extra Dot in Output

**File:** `src-tauri/src/commands/transcription.rs:84-87`
**Severity:** HIGH
**Category:** Bug

```rust
csv.push_str(&format!(
    "{},{}.,{},{},{},\"{}\",\"{}\"\n",
    t.id, t.created_at, t.duration_ms, t.word_count, t.llm_applied,
    text_escaped, raw_escaped,
));
```

The format string `{}.,{}` produces `2026-03-30T12:00:00Z.,1500` — a spurious dot between `created_at` and `duration_ms`. This corrupts the CSV output and will break any downstream parsing.

**Recommendation:** Remove the extra dot:
```rust
"{},{},{},{},{},\"{}\",\"{}\"\n",
```

---

### H-007: No Dependency Vulnerability Scanning

**Files:** Missing from CI pipeline
**Severity:** HIGH
**Category:** Security / CI

Neither `cargo audit` nor `npm audit` is run in CI or documented as a development practice. The project has 50+ transitive Rust dependencies and 30+ npm dependencies — any of which could have known vulnerabilities.

**Recommendation:**
1. Add `cargo install cargo-audit && cargo audit` to CI
2. Add `npm audit --production` to CI
3. Consider GitHub Dependabot or `cargo-deny` for continuous monitoring

---

### H-008: Unnecessary Clone in Settings Update (Pass 2 Correction)

**File:** `src-tauri/src/commands/settings.rs:67-68`
**Severity:** HIGH (correctness issue)
**Category:** Bug

```rust
*settings = new_settings.clone();  // Clone here...
drop(settings);

if let Err(e) = persist_settings(&new_settings) {  // ...original used here
```

The `clone()` is unnecessary since `new_settings` is owned and still used after. But more importantly, if `Settings` contained reference-counted or interior-mutable fields, the stored and persisted copies could diverge. Currently `Settings` is a simple data struct so this is benign, but it's a latent bug.

**Pass 2 correction:** The clone IS technically needed because the function needs to both store a copy in the mutex and pass a reference to `persist_settings`. However, the fix is simply to reverse the order: persist first (borrowing `new_settings`), then move into the mutex. This eliminates the clone and the divergence risk.

**Recommendation:**
```rust
if let Err(e) = persist_settings(&new_settings) {
    log::error!("Failed to persist settings: {}", e);
}
let mut settings = state.settings.lock().await;
*settings = new_settings;  // Move, don't clone
```

---

### H-009: State Machine Stuck After Stale Job Discard (Pass 2)

**File:** `src-tauri/src/hotkeys/manager.rs:475-478`
**Severity:** HIGH
**Category:** Reliability / State Machine
**Added in:** Pass 2

```rust
// Check if this job is still current (user may have started a new recording)
if !state.is_current_job(job_id) {
    log::info!("Job {} is stale, discarding transcription", job_id);
    return;  // BUG: state is still Transcribing!
}
```

When a transcription job is detected as stale (because a new recording started), the function returns early **without transitioning state back to Idle**. The state remains in `Transcribing` (or `CleaningUp` if it reaches line 575-578). The overlay is hidden by the new recording flow, but the state machine is corrupted.

A second stale-job check at line 575-578 does call `hide_overlay` but also fails to set state to Idle.

**Impact:** If a user starts recording, then quickly stops and starts again before the first transcription completes, the state machine can become stuck. Subsequent recordings may be blocked because `handle_start_recording` checks `if current != AppStateEnum::Idle`.

**Recommendation:** Always transition to Idle when discarding a stale job:
```rust
if !state.is_current_job(job_id) {
    log::info!("Job {} is stale, discarding transcription", job_id);
    state.set_state(AppStateEnum::Idle);
    let _ = app_clone.emit("state-changed", &AppStateEnum::Idle);
    hide_overlay(&app_clone);
    return;
}
```

---

### H-010: LLM Sidecar Lost on Cleanup Timeout (Pass 2)

**File:** `src-tauri/src/hotkeys/manager.rs:541-568`
**Severity:** HIGH
**Category:** Resource Leak
**Added in:** Pass 2

```rust
// Run cleanup via sidecar (take/put pattern for spawn_blocking Send requirement)
if let Some(mut llm) = llm_guard.take() {  // <-- takes sidecar OUT of mutex
    // ...
    let cleanup_result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tokio::task::spawn_blocking(move || {
            let result = llm.cleanup(&text_for_cleanup, mode);
            (llm, result)  // <-- returns sidecar
        }),
    ).await;

    match cleanup_result {
        Ok(Ok((llm_back, Ok(cleaned)))) => {
            *llm_guard = Some(llm_back);  // <-- put back
        }
        Ok(Ok((llm_back, Err(e)))) => {
            *llm_guard = Some(llm_back);  // <-- put back
        }
        Ok(Err(e)) => {
            // Task panicked -- sidecar is gone forever
            log::error!("LLM cleanup task panicked: {}, sidecar lost", e);
        }
        Err(_) => {
            // TIMEOUT: sidecar is inside the spawn_blocking task which is
            // still running! It's taken from the mutex and never put back.
            log::warn!("LLM cleanup timed out after 30s, using raw text");
        }
    }
}
```

When the 30-second timeout fires, `tokio::time::timeout` returns `Err`, but the `spawn_blocking` task is **still running** in the background with the `LlmEngine` inside it. The sidecar is:
1. Not returned to the mutex (state is `None`)
2. Still alive in the background task (blocking thread)
3. When the background task eventually completes, the `LlmEngine` is dropped, calling `quit()` and killing the process

The next LLM cleanup will spawn a **new** sidecar process. If timeouts happen repeatedly, multiple orphaned sidecar processes accumulate until the blocking tasks complete.

**Recommendation:** Use `tokio::task::spawn_blocking` with an abort handle, or restructure to not take the sidecar out of the mutex. Alternatively, accept the sidecar loss on timeout but explicitly document it and ensure the orphaned task's `Drop` is safe.

---

### H-011: `get_llm_status` Spawns and Kills Sidecar Just to Check Status (Pass 2)

**File:** `src-tauri/src/commands/llm.rs:43-56`
**Severity:** HIGH
**Category:** Performance / Resource Waste
**Added in:** Pass 2

```rust
let downloaded = if available && venv_ready {
    let model_id_owned = model_id.to_string();
    tokio::task::spawn_blocking(move || {
        match engine::LlmEngine::spawn_with_model(&model_id_owned) {
            Ok(mut e) => {
                let status = e.status();
                e.quit();  // Immediately kills the process
                // ...
            }
            Err(_) => false,
        }
    }).await.unwrap_or(false)
} else {
    false
};
```

Every time the settings panel opens (or calls `refreshLlmStatus`), this spawns a complete Python sidecar process, waits for it to initialize, queries status, then kills it. This is called from `onMount` in `settings-panel.svelte`.

**Impact:**
- Each invocation takes 1-3 seconds (Python startup + venv activation)
- Creates unnecessary process churn
- If the user already has a running sidecar (via `state.llm_engine`), this spawns a SECOND one

**Recommendation:** Check the HuggingFace cache directory directly from Rust (like the ASR model check does) without spawning a Python process. The download status can be determined by checking if the model files exist at the expected cache path.

---

## 5. Medium-Priority Findings

### M-001: `unsafe impl Send for AudioCapture`

**File:** `src-tauri/src/audio/capture.rs:13`
**Severity:** MEDIUM
**Category:** Safety

```rust
unsafe impl Send for AudioCapture {}
```

`cpal::Stream` is `!Send` because it holds OS-specific audio resources. The manual `unsafe impl Send` asserts that moving `AudioCapture` across threads is safe. The safety argument (access only via `Mutex<AudioCapture>`) is sound but fragile — it relies on a structural invariant (Mutex wrapping) that isn't enforced by the type system.

**Recommendation:** Add a safety comment explaining the invariant:
```rust
// SAFETY: AudioCapture is only accessed through Mutex<AudioCapture> in AppState.
// The cpal::Stream handle is created and destroyed on the same thread (the
// audio capture thread). The Mutex ensures exclusive access.
unsafe impl Send for AudioCapture {}
```

---

### M-002: Accessibility Functional Check Accepts Error Code -25205

**File:** `src-tauri/src/paste/macos.rs:143`
**Severity:** MEDIUM
**Category:** Correctness

```rust
let ok = result == 0 || result == -25205;
```

The code accepts `kAXErrorAttributeUnsupported` (-25205) as a success indicator. While the comment explains the rationale (distinguishing "API disabled" from "attribute not available"), this could mask a real permission issue if the AX API returns -25205 for a different reason.

**Recommendation:** Log the specific error code path taken so debugging is easier:
```rust
match result {
    0 => true,
    -25205 => {
        log::debug!("AX check: attribute unsupported (expected for system-wide element)");
        true
    }
    code => {
        log::warn!("AX functional check failed with code: {}", code);
        false
    }
}
```

---

### M-003: Event Listener Cleanup Without Error Handling (Frontend)

**File:** `src/lib/components/overlay-pill.svelte:89-96`
**Severity:** MEDIUM
**Category:** Resource Leak

```typescript
listen<{ level: number }>('audio-level', (event) => {
    // ...
}).then((u) => unlisteners.push(u));  // No .catch()
```

Multiple `listen()` calls use `.then()` without `.catch()`. If `listen()` rejects (unlikely but possible during app teardown), the unlistener is never registered, causing a silent memory leak.

**Pattern repeats in:** `settings-panel.svelte`, `history-view.svelte`, `onboarding-view.svelte`

**Recommendation:** Use `Promise.all` with error handling:
```typescript
const listeners = await Promise.all([
    listen('audio-level', handler1),
    listen('recording-stopped', handler2),
    // ...
]);
unlisteners.push(...listeners);
```

---

### M-004: Untyped Event Payloads

**File:** `src/lib/components/overlay-pill.svelte:114`
**Severity:** MEDIUM
**Category:** Type Safety

```typescript
listen('state-changed', (event) => {
    const state = event.payload as string;  // Unchecked cast
```

Several event listeners cast payloads without type validation. If the Rust backend changes an event's payload structure, the frontend will fail silently or produce incorrect behavior.

**Affected events:** `state-changed`, `recording-time-warning`, `setup-progress`, `key-captured`

**Recommendation:** Define explicit TypeScript interfaces for all IPC events and use them consistently:
```typescript
interface StateChangedPayload {
    state: 'Idle' | 'Recording' | 'Transcribing' | 'CleaningUp';
}
listen<StateChangedPayload>('state-changed', (event) => {
    const state = event.payload.state;
});
```

---

### M-005: Settings Persistence Failure Is Logged But Not Reported

**File:** `src-tauri/src/commands/settings.rs:72-74`
**Severity:** MEDIUM
**Category:** User Experience

```rust
if let Err(e) = persist_settings(&new_settings) {
    log::error!("Failed to persist settings: {}", e);
}
```

If settings fail to persist to disk, the function returns `Ok(())` to the frontend. The user believes settings are saved, but they'll revert on next app launch. Same pattern exists for transcription persistence (`transcription.rs:100-102`).

**Recommendation:** Return the error to the frontend so the UI can display a warning:
```rust
persist_settings(&new_settings)
    .map_err(|e| format!("Settings applied but failed to save to disk: {}", e))?;
```

---

### M-006: No Hotkey Conflict Detection

**File:** `src-tauri/src/hotkeys/manager.rs:47-55`
**Severity:** MEDIUM
**Category:** User Experience

`register_shortcuts()` doesn't validate that shortcuts don't conflict. If a user sets push-to-talk and toggle to the same key, behavior is undefined (likely both handlers fire).

**Recommendation:** Validate before registration:
```rust
if ptt_shortcut == toggle_shortcut {
    return Err("Push-to-talk and toggle shortcuts cannot be the same".into());
}
```

---

### M-007: Signing Identity Hardcoded in tauri.conf.json

**File:** `src-tauri/tauri.conf.json:38`
**Severity:** MEDIUM
**Category:** Configuration

```json
"signingIdentity": "Developer ID Application: Juan Villa (DR3FNR9MW9)"
```

The signing identity is hardcoded in version-controlled config. While not a secret (it's a public certificate name), it:
- Prevents other developers from building signed versions
- Creates noise in diffs when the cert changes
- CI already overrides this via `APPLE_SIGNING_IDENTITY` env var

**Recommendation:** Remove from config and rely solely on the CI environment variable, or document that this is intentionally developer-specific.

---

### M-008: Global `__resetOverlay` Function on Window

**File:** `src/lib/components/overlay-pill.svelte:87`
**Severity:** MEDIUM
**Category:** Code Quality / Security

```typescript
(window as any).__resetOverlay = resetOverlay;
```

Exposing a function on the global `window` object is a code smell. While necessary for Rust's `eval()` to call into the Svelte component, it bypasses type safety and could be called at unexpected times.

**Recommendation:** Use Tauri's event system instead:
```typescript
// Frontend
listen('reset-overlay', resetOverlay);

// Rust
app.emit("reset-overlay", ()).unwrap();
```

---

### M-009: No Log Rotation / Size Limits

**File:** `src-tauri/src/lib.rs` (logging setup)
**Severity:** MEDIUM
**Category:** Operations

The app logs extensively (audio levels every second, key events, state transitions) but has no log rotation. Over time, the log file at `~/Library/Logs/com.sottoasr.app/SottoASR.log` will grow unbounded.

**Recommendation:** Configure log rotation with a max file size (e.g., 10MB) and keep 3 rotated files.

---

### M-010: Incorrect ARIA Role on Shortcut Recorder

**File:** `src/lib/components/shortcut-recorder.svelte`
**Severity:** MEDIUM
**Category:** Accessibility

The shortcut recorder button uses `role="textbox"` which is semantically incorrect for a button element. Screen readers will announce it as a text input, confusing users.

**Recommendation:** Remove the explicit `role` attribute (the native `<button>` role is correct) and provide a descriptive `aria-label`.

---

### M-011: Verbose Logging in Key Capture Callback

**File:** `src-tauri/src/commands/keycapture.rs:113-118`
**Severity:** MEDIUM
**Category:** Performance / Privacy

```rust
log::info!(
    "[keycap] event_type={}, keycode=0x{:02X} ({})",
    event_type_raw, keycode_for_log, keycode_for_log
);
```

Every single key event (including non-capturing events) is logged at `info` level. When CAPTURING is false (99% of the time), this still logs every keystroke. This:
- Creates excessive log noise
- May inadvertently log sensitive keystrokes (passwords, messages) to disk
- Adds minor latency to every keystroke system-wide

**Recommendation:** Move diagnostic logging behind `CAPTURING` check or downgrade to `trace` level:
```rust
if CAPTURING.load(Ordering::SeqCst) {
    log::debug!("[keycap] event_type={}, keycode=0x{:02X}", event_type_raw, keycode_for_log);
}
```

Wait — re-reading the code, the `CAPTURING` check at line 102 returns early. But the logging at line 113-118 is INSIDE the callback and runs AFTER the CAPTURING check. Let me re-verify...

Actually, looking again at lines 101-118: the CAPTURING check at line 102 returns `event` early if not capturing. The log at line 113-118 is only reached when CAPTURING is true. This is correct behavior — logging only during key capture sessions (shortcut recording). **Revised: This is NOT a privacy concern.** However, it's still verbose during capture sessions and could be `debug` level.

---

### M-012: `dirs::data_dir()` Fallback Could Create Invalid Path

**File:** `src-tauri/src/commands/settings.rs:9`, `src-tauri/src/commands/transcription.rs:17`
**Severity:** MEDIUM
**Category:** Correctness

Both `settings_path()` and `storage_path()` use:
```rust
let data_dir = dirs::data_dir().ok_or("Could not determine data directory")?;
```

This is correct — they return an error if `data_dir()` returns None. However, the ASR backend (if using FluidAudio) may have a different pattern. Worth ensuring consistency across all path resolution.

---

### M-013: Cancelled Recording Races With New Recording (Pass 2)

**File:** `src-tauri/src/hotkeys/manager.rs:673-784`
**Severity:** MEDIUM
**Category:** Concurrency / State Machine
**Added in:** Pass 2

In `handle_cancel_recording`, the state is set to `Idle` and events emitted at line 698, but a background transcription task is spawned at line 741 that runs concurrently. During the time this background task is executing:

1. State is `Idle` (set at line 698 via event emission, and formally at line 763/766/781)
2. The user can start a new recording (since state is Idle)
3. The background cancel-transcription task holds the `asr_engine` lock (line 743)
4. If the new recording finishes quickly, `handle_stop_recording` will block waiting for `asr_engine.lock()` at line 460

This means the cancelled recording's transcription blocks the new recording's transcription. The user sees a delay.

Additionally, the background task at line 763 sets `state.set_state(AppStateEnum::Idle)` which could overwrite a `Recording` state if the user has already started a new recording.

**Recommendation:** Either:
1. Don't transcribe cancelled recordings (just discard the audio)
2. Or use the job ID / generation counter to prevent the background task from interfering with state

---

### M-014: `show_overlay` Setting Is Never Checked (Pass 2)

**File:** `src-tauri/src/hotkeys/manager.rs:289`, `src-tauri/src/models.rs:50`
**Severity:** MEDIUM
**Category:** Feature Bug
**Added in:** Pass 2

The `Settings` struct has a `show_overlay: bool` field (default `true`), and the settings panel has a toggle for it. However, `handle_start_recording` always calls `show_overlay(app)` at line 289 without checking this setting. The user's preference is silently ignored.

**Recommendation:** Check the setting before showing:
```rust
let settings = state.settings.lock().await;
let should_show = settings.show_overlay;
drop(settings);
if should_show {
    show_overlay(app);
}
```

---

### M-015: `read_log_tail` Reads Entire Log File Into Memory (Pass 2)

**File:** `src-tauri/src/tray/menu.rs:168-178`
**Severity:** MEDIUM
**Category:** Performance
**Added in:** Pass 2

```rust
let reader = BufReader::new(file);
let lines: Vec<String> = reader.lines().map_while(Result::ok).collect();
let start = lines.len().saturating_sub(n);
lines[start..].join("\n")
```

This reads the entire log file into a `Vec<String>` to extract the last 100 lines. Combined with M-009 (no log rotation), the log file can grow to hundreds of megabytes. Each click of "Copy Diagnostics" in the tray menu reads the full file.

**Recommendation:** Use a reverse-reading approach (read from end of file) or cap the read to the last 1MB of the file.

---

### M-016: `Pasting` State Defined But Never Used (Pass 2)

**File:** `src-tauri/src/models.rs:36`, `src-tauri/src/hotkeys/manager.rs`
**Severity:** MEDIUM
**Category:** Code Quality / Dead Code
**Added in:** Pass 2

```rust
pub enum AppStateEnum {
    Idle,
    Recording,
    Transcribing,
    CleaningUp,
    Pasting,  // <-- never set anywhere in the backend
}
```

The `Pasting` variant is defined in `AppStateEnum` and its TypeScript mirror (`AppStateEnum` in `tauri.ts`) but is never set by any Rust code. The `RecordingStore.isPasting` getter checks for it but it will never be true. The state machine transitions are: `Idle -> Recording -> Transcribing -> [CleaningUp ->] Idle`, skipping `Pasting` entirely.

**Recommendation:** Either implement the `Pasting` state (set it before paste, clear after) or remove it to avoid confusion.

---

## 6. Low-Priority Findings

### L-001: Audio Levels Array Growth During Recording

**File:** `src/lib/components/overlay-pill.svelte:95`
**Severity:** LOW
**Category:** Performance

```typescript
audioLevels = [...audioLevels, Math.max(0, level)];
```

The `audioLevels` array grows for the duration of a recording (~30 entries/sec). At max recording duration (12 minutes), this reaches ~21,600 entries (~170KB). This is bounded and reset on each recording (line 130), so it's not a memory leak, but the spread operator creates a new array copy on every append.

**Recommendation:** Use a fixed-size ring buffer or `.push()` instead of spread:
```typescript
audioLevels.push(Math.max(0, level));
audioLevels = audioLevels;  // Trigger Svelte reactivity
```

---

### L-002: Logging Modulo Check Uses `% 30 == 1`

**File:** `src-tauri/src/audio/capture.rs:79`
**Severity:** LOW
**Category:** Code Style

```rust
if level_emit_count % 30 == 1 {
```

Using `== 1` instead of the conventional `== 0` means the first log happens on count 1 (immediately) rather than count 30 (after ~1 second). This is likely intentional but non-obvious.

**Recommendation:** Add a comment explaining the intent, or use `== 0` if the first-emission behavior isn't important.

---

### L-003: No `.nvmrc` or `.node-version` File

**File:** Missing
**Severity:** LOW
**Category:** Developer Experience

No Node.js version is pinned for the project. CI uses `lts/*` which could be Node 20.x, 22.x, or 24.x. Local development uses whatever the developer has installed.

**Recommendation:** Create `.nvmrc` with the target version:
```
22
```

---

### L-004: No `.editorconfig`

**File:** Missing
**Severity:** LOW
**Category:** Developer Experience

No `.editorconfig` to enforce consistent indentation across editors. Rust uses 4-space indent, TypeScript uses 2-space — this should be codified.

---

### L-005: Missing ESLint/Prettier Configuration

**File:** Missing
**Severity:** LOW
**Category:** Code Quality

No linting or formatting tools are configured for the frontend. The codebase is already fairly consistent, but automated formatting would prevent style drift.

---

### L-006: `svelte.config.js` Is Empty

**File:** `svelte.config.js`
**Severity:** LOW
**Category:** Configuration

The Svelte config exports an empty object. While Vite handles most configuration, explicitly enabling `runes: true` would document the project's reliance on Svelte 5 rune mode.

---

### L-007: Large CSV Export Loads Entire String Into Memory

**File:** `src-tauri/src/commands/transcription.rs:78-91`
**Severity:** LOW
**Category:** Performance

`export_transcriptions_csv()` builds the entire CSV as a single `String` in memory before returning it over IPC. With 5,000 transcriptions (the storage cap), this could be several megabytes. Not dangerous, but inefficient.

**Recommendation:** For future scaling, consider writing directly to a temp file and returning the path.

---

### L-008: Hardcoded Model File Sizes

**File:** `src-tauri/src/asr/model.rs` (if using parakeet backend)
**Severity:** LOW
**Category:** Maintainability

Model download validation uses hardcoded expected file sizes. If upstream model files are updated, the download logic may unnecessarily re-download or fail validation.

---

### L-009: Alt Shortcut Registration Failures Are Silently Swallowed

**File:** `src-tauri/src/hotkeys/manager.rs:154-162`
**Severity:** LOW
**Category:** User Experience

```rust
if let Err(e) = register_ptt(app, alt) {
    log::warn!("Failed to register alt push-to-talk shortcut: {}", e);
}
```

Users won't know their alt shortcut failed to register. This is logged but not communicated to the UI.

**Recommendation:** Emit an event so the settings panel can show a warning.

---

### L-010: Dark Mode Is Hardcoded

**File:** `src/app.css`
**Severity:** LOW
**Category:** Design

```css
color-scheme: dark;
```

The app only supports dark mode. While appropriate for the current use case (menu bar app, overlay), there's no `prefers-color-scheme` media query for users who prefer light mode.

---

### L-011: `PermissionStatus` TypeScript Type Missing `input_monitoring` Field (Pass 2)

**File:** `src/lib/utils/tauri.ts:94-103`
**Severity:** LOW
**Category:** Type Safety
**Added in:** Pass 2

The Rust `PermissionStatus` struct (in `commands/permissions.rs:6-17`) includes an `input_monitoring: String` field, but the TypeScript `PermissionStatus` interface omits it:

```typescript
// Missing from TypeScript interface:
// input_monitoring: string;
```

Any frontend code attempting to access `status.input_monitoring` would get `undefined` instead of a type error at compile time. Currently no frontend code uses this field, so the impact is low.

**Recommendation:** Add the field to the TypeScript interface to maintain parity with the Rust type.

---

## 7. Informational Notes

These are observations that are not issues but are worth documenting:

### I-005: Pass 2 Corrections to Existing Findings

**C-002 (CGEventTap null check):** Pass 2 notes that the "edge cases: permission revocation, system sleep/wake" claim for null `user_info` is not well-supported by macOS documentation. The kernel preserves the pointer passed to `CGEventTapCreate`. A null `user_info` would indicate a catastrophic system bug, not a normal edge case. The null check is still good defensive practice, but the severity may be overstated -- downgrade consideration warranted.

**H-008 (Unnecessary clone in settings update):** Pass 2 correction: the clone IS needed because `new_settings` is both stored in the mutex and passed by reference to `persist_settings`. The fix is to reverse the order (persist first, then move into mutex) rather than simply removing the clone. The finding title is misleading -- it's more of an "inefficient ordering" issue than an "unnecessary clone." Severity revised from HIGH to MEDIUM.

**M-011 (Verbose logging in key capture):** The audit already self-corrected inline. The logging only fires when `CAPTURING` is true (during shortcut recording sessions), not on every keystroke. Not a privacy concern. Downgrade to LOW.

---

### I-001: CGEventTap Pointer Is Intentionally Leaked

**File:** `src-tauri/src/commands/keycapture.rs:217`

```rust
let app_ptr = Box::into_raw(Box::new(app.clone()));
```

The pointer is cleaned up on tap creation failure (line 237: `Box::from_raw(app_ptr)`) but not on success. This is intentional — the tap runs for the entire app lifetime in an infinite retry loop. The memory is reclaimed on process exit. This is a valid pattern for app-lifetime resources.

### I-002: `.env` File Contains Apple Credentials But Is Properly Gitignored

The `.env` file contains an app-specific password for Apple notarization. It is correctly listed in `.gitignore` (line 30) and was **never committed** to git history (verified via `git log`). However, the plaintext password on disk is a mild risk if the developer's machine is compromised.

**Suggestion:** Consider using macOS Keychain instead of a `.env` file for local credentials.

### I-003: `macOSPrivateApi: true` Is Required

**File:** `src-tauri/tauri.conf.json:14`

This flag is necessary for the app's core functionality (CGEventTap for global hotkeys, NSPanel for overlay). It's correctly configured and documented.

### I-004: Recording Generation Counter Prevents Stale Timers

**File:** `src-tauri/src/hotkeys/manager.rs:294`

The `recording_generation` atomic counter is a well-implemented guard against stale auto-stop timers. Each timer checks its generation before acting, preventing race conditions when recordings are stopped and restarted quickly.

---

## 8. Positive Practices

The codebase demonstrates several strong engineering practices worth preserving:

1. **Accessibility permission handling** — Both compile-time (`AXIsProcessTrusted`) and runtime functional checks, with clear user-facing error messages.
2. **CGEvent pipeline warmup** — Workaround for macOS Sequoia's first-event-dropped bug, preventing paste failures on first use.
3. **Target PID capture before overlay** — Records the frontmost app before showing the overlay, ensuring paste goes to the correct application.
4. **Recording generation counter** — Prevents stale timers and stale transcription results.
5. **Job ID for stale result prevention** — The `new_job()` / `is_current_job()` pattern prevents old transcription results from being used.
6. **Audio buffer size limits** — Explicit cap at `MAX_AUDIO_BUFFER_SAMPLES` prevents memory exhaustion.
7. **Dynamic cancel shortcut registration** — Cancel shortcut is only registered during recording, avoiding global key conflicts.
8. **Silence padding for ASR** — Appending 750ms of silence to audio ensures the ASR model processes the final words.
9. **Comprehensive logging** — State transitions, timing, and error conditions are well-logged for debugging.
10. **AppleScript fallback for activation** — Falls back to `osascript` when ObjC activation fails, improving reliability.

---

## 9. Summary Table

| ID | Severity | Category | Title | File | Pass |
|------|----------|----------|-------|------|------|
| C-001 | CRITICAL | Performance | Heap allocation in audio callback | `audio/capture.rs:60-69` | 1 |
| C-002 | CRITICAL | Safety | Missing null check in CGEventTap | `keycapture.rs:106` | 1 (see I-005) |
| C-003 | CRITICAL | QA | No automated tests | Entire codebase | 1 |
| C-004 | CRITICAL | CI/CD | Outdated tauri-action@v0 | `build-release.yml:75` | 1 |
| C-005 | CRITICAL | Correctness | `start_recording` command broken | `commands/recording.rs:6-25` | 2 |
| H-001 | HIGH | Security | No settings input validation | `settings.rs:62-78` | 1 |
| H-002 | HIGH | Security | CSP unsafe-inline styles | `tauri.conf.json:16` | 1 |
| H-003 | HIGH | Reliability | PTT polling without timeout | `manager.rs:95-107` | 1 |
| H-004 | HIGH | Reliability | Mutex poisoning not handled | `manager.rs` (multiple) | 1 |
| H-005 | HIGH | Data Loss | Clipboard restore race condition | `paste/macos.rs:73-82` | 1 |
| H-006 | HIGH | Bug | CSV export extra dot typo | `transcription.rs:84-87` | 1 |
| H-007 | HIGH | Security | No dependency vulnerability scanning | CI pipeline | 1 |
| H-008 | MEDIUM | Bug | Inefficient settings update ordering | `settings.rs:67-68` | 1 (revised P2) |
| H-009 | HIGH | Reliability | State stuck after stale job discard | `manager.rs:475-478` | 2 |
| H-010 | HIGH | Resource Leak | LLM sidecar lost on cleanup timeout | `manager.rs:541-568` | 2 |
| H-011 | HIGH | Performance | get_llm_status spawns+kills sidecar | `commands/llm.rs:43-56` | 2 |
| M-001 | MEDIUM | Safety | unsafe impl Send for AudioCapture | `capture.rs:13` | 1 |
| M-002 | MEDIUM | Correctness | AX check accepts -25205 | `paste/macos.rs:143` | 1 |
| M-003 | MEDIUM | Resource Leak | Event listener cleanup without .catch() | `overlay-pill.svelte` | 1 |
| M-004 | MEDIUM | Type Safety | Untyped event payloads | Multiple frontend files | 1 |
| M-005 | MEDIUM | UX | Settings persistence failure silent | `settings.rs:72-74` | 1 |
| M-006 | MEDIUM | UX | No hotkey conflict detection | `manager.rs:47-55` | 1 |
| M-007 | MEDIUM | Config | Signing identity hardcoded | `tauri.conf.json:38` | 1 |
| M-008 | MEDIUM | Code Quality | Global __resetOverlay on window | `overlay-pill.svelte:87` | 1 |
| M-009 | MEDIUM | Operations | No log rotation | `lib.rs` | 1 |
| M-010 | MEDIUM | A11y | Incorrect ARIA role on shortcut recorder | `shortcut-recorder.svelte` | 1 |
| M-011 | LOW | Performance | Verbose logging in key capture | `keycapture.rs:113-118` | 1 (revised P2) |
| M-012 | MEDIUM | Correctness | dirs::data_dir() fallback consistency | Multiple files | 1 |
| M-013 | MEDIUM | Concurrency | Cancel recording races with new recording | `manager.rs:673-784` | 2 |
| M-014 | MEDIUM | Feature Bug | show_overlay setting never checked | `manager.rs:289` | 2 |
| M-015 | MEDIUM | Performance | read_log_tail reads entire file | `tray/menu.rs:168-178` | 2 |
| M-016 | MEDIUM | Dead Code | Pasting state defined but never used | `models.rs:36` | 2 |
| L-001 | LOW | Performance | Audio levels array spread copy | `overlay-pill.svelte:95` | 1 |
| L-002 | LOW | Style | Logging modulo uses == 1 | `capture.rs:79` | 1 |
| L-003 | LOW | DX | No .nvmrc file | Missing | 1 |
| L-004 | LOW | DX | No .editorconfig | Missing | 1 |
| L-005 | LOW | Quality | No ESLint/Prettier config | Missing | 1 |
| L-006 | LOW | Config | Empty svelte.config.js | `svelte.config.js` | 1 |
| L-007 | LOW | Performance | CSV export loads full string | `transcription.rs:78-91` | 1 |
| L-008 | LOW | Maintainability | Hardcoded model file sizes | `asr/model.rs` | 1 |
| L-009 | LOW | UX | Alt shortcut failures swallowed | `manager.rs:154-162` | 1 |
| L-010 | LOW | Design | Dark mode only | `app.css` | 1 |
| L-011 | LOW | Type Safety | PermissionStatus TS type missing field | `tauri.ts:94-103` | 2 |
| P3-H-001 | HIGH | Security | App runs without macOS App Sandbox | `Entitlements.plist` | 3 |
| P3-H-002 | HIGH | Bug | CSV export does not escape newlines | `transcription.rs:82-88` | 3 |
| P3-M-001 | MEDIUM | Data Integrity | Non-atomic file writes | `settings.rs:48`, `transcription.rs:41` | 3 |
| P3-M-002 | MEDIUM | Safety | unsafe impl Send for LlmEngine undocumented | `engine.rs:25` | 3 |
| P3-M-003 | MEDIUM | Resource Leak | CF objects leaked on EventTap restart | `keycapture.rs:241-254` | 3 |
| P3-M-004 | MEDIUM | Supply Chain | tauri-nspanel uses unpinned git branch | `Cargo.toml:30` | 3 |
| P3-M-005 | MEDIUM | Security | fix_accessibility_permission runs tccutil via IPC | `permissions.rs:157-178` | 3 |
| P3-L-001 | LOW | Security | Broad capability scope for all windows | `capabilities/default.json` | 3 |
| P3-L-002 | LOW | Dead Code | model_path field unused but persisted | `models.rs:55` | 3 |
| P4-H-012 | HIGH | Resource Leak | Onboarding leaks 4 event listeners | `onboarding-view.svelte:42-68` | 4 |
| P4-M-017 | MEDIUM | Config | Duplicate HTML files, missing body class | Root vs src/ HTML files | 4 |
| P4-M-018 | MEDIUM | Correctness | $effect used where onMount should be | `history-view.svelte:73-85` | 4 |
| P4-M-019 | MEDIUM | Accessibility | No prefers-reduced-motion support | Multiple CSS files | 4 |
| P4-L-012 | LOW | Type Safety | TS strict lint rules not in app config | `tsconfig.app.json` | 4 |
| P4-L-014 | LOW | Maintenance | Website version badge stale (v0.3.3) | `website/index.html:392` | 4 |
| P4-L-015 | LOW | Reproducibility | Floating npm dependency versions | `package.json` | 4 |

**Totals:** 5 Critical, 13 High, 24 Medium, 17 Low = **59 findings** (9 from Pass 2, 9 from Pass 3, 7 from Pass 4, 2 severity revisions)

---

## 10. Recommendations & Priorities

### Immediate (Before Next Release)

1. **Fix or remove `start_recording` command** (C-005) -- Broken code path can permanently corrupt state machine
2. **Fix CSV export typo** (H-006) -- One-character fix, corrupts all exported data
3. **Fix CSV newline escaping** (P3-H-002) -- Newlines in transcription text corrupt CSV rows
4. **Fix stale job state leak** (H-009) -- State gets stuck in Transcribing/CleaningUp, blocking all future recordings
5. **Add null check in CGEventTap callback** (C-002) -- One-line fix, defensive coding
6. **Fix clipboard restore race** (H-005) -- Check change count before restoring
7. **Check `show_overlay` setting** (M-014) -- User preference silently ignored
8. **Add settings validation** (H-001) -- Prevents invalid state from being stored
9. **Add timeout to PTT polling loop** (H-003) -- Prevents thread leak

### Short-Term (Next 2 Weeks)

10. **Reduce audio callback allocations** (C-001) -- Pre-allocate downmix buffer
11. **Fix LLM sidecar timeout leak** (H-010) -- Sidecar orphaned on cleanup timeout
12. **Replace sidecar-spawning status check** (H-011) -- Check HuggingFace cache directly instead of spawning Python
13. **Make file writes atomic** (P3-M-001) -- Write-then-rename to prevent transcription data loss on crash
14. **Pin tauri-nspanel to commit hash** (P3-M-004) -- Prevent unintended dependency updates on `cargo update`
15. **Fix cancel recording race** (M-013) -- Background transcription task can corrupt state
16. **Remove dead `Pasting` state** (M-016) -- Or implement it properly
17. **Add basic test suite** (C-003) -- Start with state machine and settings validation tests
18. **Upgrade tauri-action** (C-004) -- Evaluate latest version compatibility
19. **Add cargo-audit / npm audit to CI** (H-007) -- Automated vulnerability detection
20. **Handle mutex poisoning consistently** (H-004) -- Use `unwrap_or_else` or switch to `parking_lot`
21. **Remove CSP unsafe-inline** (H-002) -- Evaluate if Svelte transitions can work without it

### Medium-Term (Next Month)

22. **Evaluate App Sandbox** (P3-H-001) -- Document decision or implement with appropriate exceptions
23. **Split capabilities by window** (P3-L-001) -- Least-privilege per webview
24. **Restrict fix_accessibility_permission scope** (P3-M-005) -- Limit IPC access to settings/onboarding windows only
25. **Add event type definitions** (M-004) -- TypeScript interfaces for all IPC events
26. **Add log rotation** (M-009) -- Prevent unbounded log growth
27. **Fix `read_log_tail` memory usage** (M-015) -- Don't read entire file for last 100 lines
28. **Add hotkey conflict detection** (M-006) -- Validate in `register_shortcuts()`
29. **Replace __resetOverlay with event** (M-008) -- Use Tauri event system
30. **Propagate persistence errors** (M-005) -- Return errors to frontend
31. **Fix PermissionStatus TypeScript type** (L-011) -- Add missing `input_monitoring` field

### Low Priority (Backlog)

32. **Add .editorconfig, .nvmrc, ESLint** (L-003, L-004, L-005) -- Developer experience
33. **Ring buffer for audio levels** (L-001) -- Minor performance improvement
34. **Light mode support** (L-010) -- Design decision
35. **Fix onboarding listener leak** (P4-H-012) -- Store and clean up all 4 event listeners
36. **Fix overlay.html missing body class** (P4-M-017) -- Add `class="overlay"`, remove duplicate src/*.html files
37. **Replace $effect with onMount in history-view** (P4-M-018) -- Prevent potential re-registration of listeners
38. **Add prefers-reduced-motion support** (P4-M-019) -- Global CSS media query
39. **Update website version badge** (P4-L-014) -- Currently shows v0.3.3, should be v0.3.4
40. **Remove unused model_path field** (P3-L-002) -- Dead code cleanup, prevents future path traversal
41. **Add safety comments to unsafe Send impls** (P3-M-002, M-001) -- Document invariants
42. **Release CF resources on tap restart** (P3-M-003) -- Prevent resource leak on EventTap invalidation

---

## Pass 4: Frontend, Configuration, and Build Audit

**Focus:** Vite configuration, TypeScript configuration, package.json analysis, Svelte component lifecycle, HTML entry points, CSS audit, build output, Tauri plugin configuration, and website audit.

**Pass 4 Summary:** 7 new findings (1 HIGH, 3 MEDIUM, 3 LOW). This pass found no additional CRITICAL issues. The frontend is generally well-structured with proper code splitting, scoped CSS, and lean build output. The most significant finding is leaked event listeners in the onboarding component.

### P4-H-012: Onboarding Component Leaks 4 Event Listeners

**File:** `src/lib/components/onboarding-view.svelte:42-68`
**Severity:** HIGH
**Category:** Resource Leak

The `onMount` callback calls `await listen(...)` four times for events (`setup-progress`, `model-download-progress`, `asr-init-complete`, `asr-init-error`) but never stores the returned unlisten functions. The `onDestroy` callback only cleans up the polling interval, not these listeners.

```typescript
// Lines 42-68: All four listen() calls discard the unlisten function
await listen<{ step: string; message: string }>('setup-progress', (event) => { ... });
await listen<{ progress: number; ... }>('model-download-progress', (event) => { ... });
await listen('asr-init-complete', () => { ... });
await listen<{ error: string }>('asr-init-error', (event) => { ... });

// Line 71-73: Only cleans up polling
onDestroy(() => {
    stopPermissionPolling();
});
```

**Impact:** When the onboarding window is closed and reopened, the old listeners remain active. Each open/close cycle leaks 4 listeners. These leaked listeners will continue receiving events and potentially updating state on a destroyed component instance.

**Recommendation:** Store all unlisten functions and clean them up in `onDestroy`:
```typescript
let unlisteners: UnlistenFn[] = [];

onMount(async () => {
    unlisteners.push(await listen('setup-progress', handler1));
    unlisteners.push(await listen('model-download-progress', handler2));
    // ...
});

onDestroy(() => {
    stopPermissionPolling();
    unlisteners.forEach(fn => fn());
});
```

---

### P4-M-017: Duplicate HTML Entry Points with Missing Body Class

**Files:** Root `overlay.html` vs `src/overlay.html` (and 4 other pairs)
**Severity:** MEDIUM
**Category:** Configuration / Correctness

The project has two sets of HTML entry point files:
- Root level: `overlay.html`, `history.html`, `settings.html`, `onboarding.html`, `about.html` (used by `vite.config.ts`)
- `src/` directory: Same filenames with slightly different content (appear to be unused leftovers)

The critical difference: `src/overlay.html` has `<body class="overlay">` but the root `overlay.html` (the one Vite actually uses) has plain `<body>`. This means the CSS rule `body.overlay { background: transparent; -webkit-user-select: none; user-select: none; }` in `app.css` does NOT apply.

The overlay window's transparent background is achieved through Tauri's window configuration, not CSS alone, so the production app likely works correctly. However:
1. The `user-select: none` property on the overlay body is lost, meaning text in the overlay could be unintentionally selectable
2. The duplicate files are confusing for contributors
3. Having two divergent sources of truth for HTML structure is an ongoing maintenance risk

**Recommendation:**
1. Add `class="overlay"` to the root `overlay.html` `<body>` tag
2. Delete the `src/*.html` files since they are unused (Vite resolves from root)

---

### P4-M-018: `$effect` Used Where `onMount` Should Be (history-view.svelte)

**File:** `src/lib/components/history-view.svelte:73-85`
**Severity:** MEDIUM
**Category:** Correctness / Performance

```typescript
$effect(() => {
    transcriptionStore.load();
    const unlisteners: Array<() => void> = [];
    listen<Transcription>('transcription-complete', (event) => {
        transcriptionStore.add(event.payload);
    }).then((unlisten) => unlisteners.push(unlisten));
    return () => {
        unlisteners.forEach((fn) => fn());
    };
});
```

`$effect` re-runs whenever its tracked reactive dependencies change. `transcriptionStore.load()` writes to `this.items` and `this.loaded` (both `$state` properties), which means the effect's body reads reactive state that it then mutates. This can trigger the effect to re-run, causing:
1. Redundant `transcriptionStore.load()` IPC calls
2. Accumulation of duplicate `transcription-complete` listeners (the cleanup function only runs on re-trigger, not immediately)

This should use `onMount` instead, which runs exactly once.

**Recommendation:** Replace `$effect` with `onMount`:
```typescript
onMount(() => {
    transcriptionStore.load();
    // ... listener setup ...
    return () => { unlisteners.forEach((fn) => fn()); };
});
```

---

### P4-M-019: No `prefers-reduced-motion` Support for Animations

**Files:** `overlay-pill.svelte`, `onboarding-view.svelte`, `shortcut-recorder.svelte`, `settings-panel.svelte`
**Severity:** MEDIUM
**Category:** Accessibility

The app uses several CSS animations and none respect the `prefers-reduced-motion` media query:
- `@keyframes pulse` -- recording dot (overlay-pill.svelte)
- `@keyframes warningPulse` -- warning banner (overlay-pill.svelte)
- `@keyframes spin` -- transcription/download spinners (3 components)
- `@keyframes pulse-border` -- shortcut recorder active state
- Multiple CSS transitions on opacity/transform

Users with "Reduce motion" enabled in macOS System Settings will still see all animations. This is a WCAG 2.3.3 (AAA) concern but also a general usability issue for motion-sensitive users.

**Recommendation:** Add a global reduced-motion query in `app.css`:
```css
@media (prefers-reduced-motion: reduce) {
  *, *::before, *::after {
    animation-duration: 0.01ms !important;
    animation-iteration-count: 1 !important;
    transition-duration: 0.01ms !important;
  }
}
```

---

### P4-L-012: TypeScript Strict Mode Not Explicit in App Config

**File:** `tsconfig.app.json`
**Severity:** LOW
**Category:** Type Safety

The `tsconfig.app.json` extends `@tsconfig/svelte/tsconfig.json` which enables `strict: true`. However, `tsconfig.app.json` does not explicitly set stricter lint rules like `noUnusedLocals`, `noUnusedParameters`, or `noFallthroughCasesInSwitch`. In contrast, `tsconfig.node.json` (used only for `vite.config.ts`) has all of these enabled.

The disparity means unused variables and parameters in Svelte/TypeScript source will not produce compilation errors, but they would in `vite.config.ts`. This is an inconsistency that could allow dead code to accumulate.

**Recommendation:** Add the same lint rules to `tsconfig.app.json`.

---

### P4-L-014: Website Version Badge Is Stale

**File:** `website/index.html:392`
**Severity:** LOW
**Category:** Maintenance

```html
<span class="version-badge">v0.3.3</span>
```

The website shows `v0.3.3` but the current app version is `0.3.4`. The release process in `.claude/rules/release.md` includes a step to update this badge, but it was missed for the 0.3.4 release.

**Recommendation:** Update to `v0.3.4` and consider adding a CI check that compares `website/index.html` version badge to `package.json` version.

---

### P4-L-015: Floating npm Dependency Versions

**File:** `package.json`
**Severity:** LOW
**Category:** Build Reproducibility

All npm dependencies use caret (`^`) ranges. While `package-lock.json` pins exact versions (and `npm ci` is correctly used in CI), local `npm install` could resolve to different versions than tested. The dependency tree is modest (123 packages total) which limits risk.

**Recommendation:** No immediate action. Optionally pin exact versions for production dependencies to eliminate ambiguity.

---

### Pass 4: Informational Observations

#### P4-I-001: Build Output Is Clean (No Source Maps in Production)

The `dist/` directory contains no `.map` files. Vite's default is to not produce source maps in production builds. This is correct for a desktop app where source maps could aid reverse engineering.

#### P4-I-002: Tauri Plugin Configuration Is Correct

The `capabilities/default.json` correctly lists scoped permissions for all Tauri plugins. The empty `"plugins": {}` in `tauri.conf.json` is correct -- Tauri v2 plugins are configured via capabilities, not the plugins field.

#### P4-I-003: Vite Multi-Page Setup Is Well-Configured

Six entry points are properly configured. Build output shows effective code splitting: shared Svelte runtime (42KB), Tauri event module, and format utilities are deduplicated into shared chunks. Total JS output is approximately 130KB minified.

#### P4-I-004: CSS Architecture Is Sound

Global CSS custom properties provide theming. Svelte scoped styles prevent collisions. No `z-index` stacking issues exist. `:focus-visible` styles are defined for keyboard accessibility. No `{@html}` usage found (good for XSS prevention).

#### P4-I-005: Website Open Graph Image Uses Relative URL

The `og:image` meta tag in `website/index.html` uses `content="assets/logo.png"` (relative). Social media platforms that require absolute URLs will not resolve this image correctly. Should be an absolute URL like `https://sottoasr.com/assets/logo.png`.

#### P4-I-006: Component Lifecycle Management Is Generally Good

Most components handle cleanup correctly:
- `overlay-pill.svelte`: Stores unlisten functions, cleans up timers, removes `window.__resetOverlay`
- `shortcut-recorder.svelte`: Thorough `onDestroy` cleanup of CGEventTap, JS event listeners, and timeouts
- `history-item.svelte`: Cleans up copy feedback timeout
- `recording-timer.svelte`: Effect cleanup cancels animation frame
- `waveform.svelte`: `onDestroy` cancels animation frame
- `settings-panel.svelte`: Stores unlisten functions and timeout array, cleans up both
- Exception: `onboarding-view.svelte` (see P4-H-012)

---

### Updated Summary Table (Pass 4 Additions)

| ID | Severity | Category | Title | File | Pass |
|------|----------|----------|-------|------|------|
| P4-H-012 | HIGH | Resource Leak | Onboarding leaks 4 event listeners | `onboarding-view.svelte:42-68` | 4 |
| P4-M-017 | MEDIUM | Config | Duplicate HTML files, missing body class | Root vs src/ HTML files | 4 |
| P4-M-018 | MEDIUM | Correctness | $effect used where onMount should be | `history-view.svelte:73-85` | 4 |
| P4-M-019 | MEDIUM | Accessibility | No prefers-reduced-motion support | Multiple CSS files | 4 |
| P4-L-012 | LOW | Type Safety | TS strict lint rules not in app config | `tsconfig.app.json` | 4 |
| P4-L-014 | LOW | Maintenance | Website version badge stale (v0.3.3) | `website/index.html:392` | 4 |
| P4-L-015 | LOW | Reproducibility | Floating npm dependency versions | `package.json` | 4 |

**Pass 4 Subtotals:** 1 High, 3 Medium, 3 Low = **7 new findings in Pass 4**

---

## Pass 3 Additions — Security & Edge-Case Focus

**Focus:** IPC command security, Tauri capabilities/permissions scope, file system operations (atomicity, path traversal, TOCTOU), process spawning (command injection), ObjC FFI safety (ARM64 calling convention), macOS entitlements, supply chain dependencies, and frontend XSS surface.

**Pass 3 Summary:** 9 new findings (2 HIGH, 5 MEDIUM, 2 LOW). No additional CRITICAL issues. The most impactful findings are the missing App Sandbox and CSV newline corruption. Positive findings: zero `{@html}` usage in frontend (no XSS surface), minimal entitlements, and correct ARM64 `objc_msgSend` signatures across all call sites.

### P3-H-001: App Runs Without macOS App Sandbox

**File:** `src-tauri/Entitlements.plist`
**Severity:** HIGH
**Category:** Security

The entitlements file contains only:
```xml
<key>com.apple.security.device.audio-input</key>
<true/>
<key>com.apple.security.accessibility</key>
<true/>
```

The `com.apple.security.app-sandbox` entitlement is **absent**, meaning the app runs completely unsandboxed. If a vulnerability in the Tauri webview or a dependency allows code execution, the attacker has full access to the user's filesystem, network, and all running processes.

**Mitigating factors:** The CSP blocks external script loading, and no `{@html}` usage exists in the frontend (verified in Pass 3). The app does not load external web content. However, unsandboxed apps are a higher-risk target.

**Note:** Enabling App Sandbox requires careful testing because the app uses CGEventTap (requires an exception), writes to `~/Library/Application Support/`, and spawns Python subprocesses. Full sandboxing may not be feasible, but this should be evaluated.

**Recommendation:** Evaluate enabling the App Sandbox with appropriate exceptions, or document the security rationale for running unsandboxed.

---

### P3-H-002: CSV Export Does Not Escape Newlines in Fields

**File:** `src-tauri/src/commands/transcription.rs:82-88`
**Severity:** HIGH
**Category:** Bug / Data Integrity

The CSV export escapes double quotes (`"` to `""`) but does not handle newlines within text fields:

```rust
let text_escaped = t.text.replace('"', "\"\"");
let raw_escaped = t.raw_text.as_deref().unwrap_or("").replace('"', "\"\"");
csv.push_str(&format!(
    "{},{}.,{},{},{},\"{}\",\"{}\"\n",
    ...
));
```

If a transcription contains a newline (common for multi-sentence dictation or LLM-cleaned markdown-mode output), the CSV output is corrupted: the newline breaks the row boundary and downstream parsers will misinterpret subsequent fields.

**Note:** This compounds with the existing H-006 finding (extra dot typo in the same line). The CSV export has two bugs on the same format string.

**Recommendation:** Replace newlines in the escaped text:
```rust
let text_escaped = t.text.replace('"', "\"\"").replace('\n', " ").replace('\r', "");
```
Or use a proper CSV library (e.g., the `csv` crate) that handles escaping correctly.

---

### P3-M-001: Non-Atomic File Writes for Settings and Transcriptions

**Files:** `src-tauri/src/commands/settings.rs:48`, `src-tauri/src/commands/transcription.rs:41`
**Severity:** MEDIUM
**Category:** Data Integrity

Both `persist_settings` and `save_to_disk` use `std::fs::write()` directly:

```rust
std::fs::write(&path, data)
```

`std::fs::write` is not atomic -- it truncates the file, then writes. If the process crashes or is killed during the write (e.g., force-quit, power loss, OS-level kill), the file is left truncated or partially written. On next launch, `serde_json::from_str` will fail to parse the corrupted file, and the app falls back to defaults (settings) or an empty list (transcriptions).

For settings this means a minor annoyance (user re-configures). For transcriptions, this means **complete loss of transcription history** -- potentially hundreds of entries.

**Recommendation:** Write to a temporary file first, then atomically rename:
```rust
let temp_path = path.with_extension("tmp");
std::fs::write(&temp_path, data)?;
std::fs::rename(&temp_path, &path)?;
```

---

### P3-M-002: `unsafe impl Send` for `LlmEngine` Without Safety Justification

**File:** `src-tauri/src/llm/engine.rs:25`
**Severity:** MEDIUM
**Category:** Safety

```rust
unsafe impl Send for LlmEngine {}
```

`LlmEngine` contains a `Child` process handle, `BufWriter<ChildStdin>`, and `BufReader<ChildStdout>`. The manual `Send` impl has no safety comment explaining the invariant, unlike `AudioCapture` (M-001) which at least has a partial comment.

The safety argument (single-owner behind `TokioMutex<Option<LlmEngine>>`) is sound but undocumented.

**Recommendation:** Add a `// SAFETY:` comment explaining why this is safe.

---

### P3-M-003: CFMachPort and CFRunLoopSource Leaked on EventTap Restart

**File:** `src-tauri/src/commands/keycapture.rs:241-254`
**Severity:** MEDIUM
**Category:** Resource Leak

When the CGEventTap run loop exits (tap invalidated by OS), `try_create_cgevent_tap` returns `true` and the retry loop creates a new tap. However, the `CFMachPortCreateRunLoopSource` and the tap `CFMachPort` from the previous iteration are never released:

```rust
let source = CFMachPortCreateRunLoopSource(std::ptr::null(), tap, 0);
let run_loop = CFRunLoopGetCurrent();
CFRunLoopAddSource(run_loop, source, kCFRunLoopCommonModes);
// ... CFRunLoopRun() returns ...
// No CFRelease(source), no CFRelease(tap)
true  // returns to retry loop
```

The `Box::into_raw(Box::new(app.clone()))` pointer (line 217) is also leaked on each retry iteration.

**Impact:** Each tap restart leaks Core Foundation objects and a Rust `AppHandle` allocation. Since taps rarely restart (only on permission revocation or system sleep edge cases), this is unlikely to cause problems in practice.

**Recommendation:** After `CFRunLoopRun()` returns:
```rust
CFRelease(source);
CFRelease(tap);
let _ = Box::from_raw(app_ptr); // reclaim the AppHandle
```

---

### P3-M-004: `tauri-nspanel` Dependency Uses Unpinned Git Branch

**File:** `src-tauri/Cargo.toml:30`
**Severity:** MEDIUM
**Category:** Supply Chain Security

```toml
tauri-nspanel = { git = "https://github.com/ahkohd/tauri-nspanel", branch = "v2.1" }
```

While `Cargo.lock` pins the specific commit (`a3122e8`), the `Cargo.toml` references a branch rather than a tag or commit hash. Running `cargo update` will silently pull whatever is on the `v2.1` branch tip, which could include breaking changes, bugs, or (in a supply-chain attack scenario) malicious code.

**Mitigating factor:** `Cargo.lock` is committed, so CI builds are reproducible. The risk is only when a developer runs `cargo update`.

**Recommendation:** Pin to a specific commit hash in `Cargo.toml`:
```toml
tauri-nspanel = { git = "https://github.com/ahkohd/tauri-nspanel", rev = "a3122e8" }
```

---

### P3-M-005: `fix_accessibility_permission` Runs `tccutil reset` via IPC

**File:** `src-tauri/src/commands/permissions.rs:157-178`
**Severity:** MEDIUM
**Category:** Security

The `fix_accessibility_permission` Tauri command executes `tccutil reset Accessibility com.sottoasr.app` via `std::process::Command`. While `tccutil` only affects the app's own TCC entry (not other apps), this command:

1. Is callable from any webview window (overlay, history, settings, onboarding, about) -- all are listed in the capabilities `windows` array
2. Resets a security permission without user confirmation beyond clicking a button

The bundle ID is hardcoded (`com.sottoasr.app`), so the blast radius is limited to the app's own accessibility permission.

**Recommendation:** Restrict this command to only the `settings` and `onboarding` windows via a dedicated capability scope, rather than the broad `default` capability that grants access to all windows.

---

### P3-L-001: Broad Capability Scope -- All Windows Get All Permissions

**File:** `src-tauri/capabilities/default.json`
**Severity:** LOW
**Category:** Security / Defense in Depth

The single `default` capability grants all permissions to all six windows:
```json
"windows": ["main", "overlay", "onboarding", "history", "settings", "about"]
```

The overlay window (always visible during recording) and the about window (static content) have access to clipboard read/write, store, window creation, and all custom Tauri commands. The principle of least privilege suggests these windows should have minimal access.

**Recommendation:** Split into per-window capabilities:
- `overlay`: only `core:event:default`
- `about`: `core:default` only
- `settings`, `onboarding`: full permissions
- `history`: read-only transcription access

---

### P3-L-002: `model_path` Field in Settings Is Unused But Persisted

**File:** `src-tauri/src/models.rs:55`
**Severity:** LOW
**Category:** Dead Code / Attack Surface

The `Settings` struct includes a `model_path: String` field that is serialized/deserialized and persisted to disk, but never read by any backend code. The FluidAudio backend uses its own hardcoded cache directory, and the parakeet backend uses `dirs::data_dir()`.

A future developer might use `model_path` without validation, creating a path traversal vulnerability.

**Recommendation:** Remove the field or add a `#[serde(skip)]` attribute.

---

### Pass 3: Positive Security Findings

#### P3-I-001: No `{@html}` Usage in Frontend

All `.svelte` files confirmed free of `{@html}`, `dangerouslySetInnerHTML`, `innerHTML` (in components), `insertAdjacentHTML`, and `document.write`. All user-controlled text is rendered via Svelte's default text interpolation with auto-escaping. The single `innerHTML` usage in `src/main.ts` is a hardcoded static string.

#### P3-I-002: Entitlements Are Minimal

Only `com.apple.security.device.audio-input` and `com.apple.security.accessibility` -- both required for core functionality. No overly broad entitlements present.

#### P3-I-003: ObjC FFI Signatures Are Correct for ARM64

All `objc_msgSend` call sites in `paste/macos.rs` and `commands/permissions.rs` correctly cast to typed function pointers with exact signatures (not variadic). Comments explain the ARM64 calling convention requirement. Return types and parameter types match the ObjC method signatures being called.

#### P3-I-004: Process Spawning Uses Hardcoded Commands Only

All `Command::new()` call sites use hardcoded command names: `osascript`, `open`, `tccutil`, `sw_vers`, `python3`. No user-controlled strings are interpolated into command names or directly into argument lists (the `osascript` call uses `format!()` with a PID integer, which cannot inject commands). The LLM sidecar receives its model ID via `--model` argument but this comes from a compile-time constant (`model_id_for_size()`), not user input.

---

### Updated Summary Table (Pass 3 Additions)

| ID | Severity | Category | Title | File | Pass |
|------|----------|----------|-------|------|------|
| P3-H-001 | HIGH | Security | App runs without macOS App Sandbox | `Entitlements.plist` | 3 |
| P3-H-002 | HIGH | Bug | CSV export does not escape newlines | `transcription.rs:82-88` | 3 |
| P3-M-001 | MEDIUM | Data Integrity | Non-atomic file writes for settings/transcriptions | `settings.rs:48`, `transcription.rs:41` | 3 |
| P3-M-002 | MEDIUM | Safety | unsafe impl Send for LlmEngine undocumented | `engine.rs:25` | 3 |
| P3-M-003 | MEDIUM | Resource Leak | CF objects leaked on EventTap restart | `keycapture.rs:241-254` | 3 |
| P3-M-004 | MEDIUM | Supply Chain | tauri-nspanel uses unpinned git branch | `Cargo.toml:30` | 3 |
| P3-M-005 | MEDIUM | Security | fix_accessibility_permission runs tccutil via IPC | `permissions.rs:157-178` | 3 |
| P3-L-001 | LOW | Security | Broad capability scope for all windows | `capabilities/default.json` | 3 |
| P3-L-002 | LOW | Dead Code | model_path field unused but persisted | `models.rs:55` | 3 |

**Updated Totals:** 5 Critical, 13 High, 24 Medium, 17 Low = **59 findings**
