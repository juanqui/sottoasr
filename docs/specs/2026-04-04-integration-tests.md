# Phase 2: Trait Boundaries and Integration Tests

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

Introduce trait abstractions at every system boundary in the SottoASR recording pipeline (audio capture, ASR transcription, LLM cleanup, paste/clipboard) so the full recording flow can be tested end-to-end in-process without real hardware, models, or macOS Accessibility permissions. This is Phase 2 of a 5-phase testing initiative; Phase 1 covers pure-logic unit tests.

The core idea: define a trait for each I/O boundary, change `AppState` to hold trait objects (`Box<dyn Trait>`) instead of concrete types, create mock implementations, and write integration tests that exercise the pipeline from "hotkey pressed" through "text pasted" using only mocks.

---

## 2. Problem Statement

The recording pipeline in `src-tauri/src/hotkeys/manager.rs` (1360 lines) orchestrates four system boundaries:

1. **Audio capture** (cpal microphone) -- requires a physical mic and audio hardware.
2. **ASR transcription** (FluidAudio/parakeet-rs) -- requires 500+ MB model files and 2-10 seconds of inference time.
3. **LLM cleanup** (Python sidecar via stdin/stdout) -- requires a Python venv, 237 MB model, and a running child process.
4. **Paste** (CGEvent Cmd+V, arboard clipboard) -- requires macOS Accessibility permissions and mutates the system clipboard.

Because these boundaries use concrete types, the pipeline cannot be tested without real hardware and permissions. Bugs in the orchestration logic (state transitions, error handling, job staleness, cancellation) are discovered only through manual testing.

**What we lose without these tests:**

- State machine correctness: does `Recording -> Transcribing -> CleaningUp -> Pasting -> Idle` always hold?
- Error propagation: does an ASR failure return to Idle without pasting?
- Job staleness: does a superseded job ID discard stale results?
- Cancellation: does cancel mid-recording skip paste and save a cancelled transcription?
- LLM bypass: when `llm_cleanup_enabled = false`, does raw ASR text go directly to paste?

---

## 3. Design Overview

```
                    Integration Test Harness
                    ========================

    TestAppState
    +-------------------------------------------------+
    | audio_capture: Box<dyn AudioCaptureBackend>      |  <-- MockAudioCapture
    | asr_engine:    Box<dyn AsrEngine>                |  <-- MockAsrEngine
    | llm_engine:    Option<Box<dyn LlmBackend>>       |  <-- MockLlmBackend
    | paste_backend: Box<dyn PasteBackend>             |  <-- MockPasteBackend
    | audio_sender/receiver: mpsc channel              |
    | settings, state, job_id, ...                     |
    +-------------------------------------------------+
                        |
                        v
            Pipeline functions extracted from
            hotkeys/manager.rs, operating on
            trait objects instead of concrete types
                        |
        +-------+-------+-------+-------+
        |       |       |       |       |
     start   stop   cancel   paste   save
```

### Concurrency Model: `tokio::spawn` Stays in the Caller

**Important architectural note:** The real `handle_stop_recording` in `manager.rs` (line 475) uses `tokio::spawn` to fire-and-forget the transcription/LLM/paste work. The function returns immediately while the actual pipeline logic runs in a detached task.

The extracted `pipeline_stop_recording` is an `async fn` that tests can `await` directly. This is **not a behavioral change** -- the pipeline performs the same work (drain audio, transcribe, LLM cleanup, paste, save). The concurrency wrapper stays in the caller:

```rust
// In hotkeys/manager.rs (production code):
pub async fn handle_stop_recording(app: &AppHandle) {
    // ... stop capture, set state, unregister shortcuts ...

    // The spawn stays HERE in the caller -- pipeline_stop_recording
    // is the awaitable function that does the actual work.
    let app_clone = app.clone();
    tokio::spawn(async move {
        let state: tauri::State<'_, AppState> = app_clone.state();
        let events = TauriEvents(&app_clone);
        pipeline_stop_recording(&state, &events).await;
    });
}

// In tests:
pipeline_stop_recording(&state, &events).await;  // directly awaitable
```

This separation means tests exercise the exact same orchestration logic without needing `tokio::spawn` indirection. The spawn boundary is a Tauri-specific concern (non-blocking hotkey handler), not pipeline logic.

**Approach: trait objects (`Box<dyn ...>`) for all backends.**

- `AsrEngine` is already a trait with `Box<dyn AsrEngine>` in `AppState`. No change needed.
- `AudioCapture` becomes `Box<dyn AudioCaptureBackend>`.
- `LlmEngine` becomes `Option<Box<dyn LlmBackend>>`.
- Paste functions become a `Box<dyn PasteBackend>` added to `AppState`.

This avoids generics infecting the entire codebase (every function that touches `AppState` would need type parameters). Trait objects have negligible runtime cost for operations that take milliseconds (transcription, paste).

---

## 4. Detailed Design

### 4.1 Trait: `AudioCaptureBackend`

**Location:** `src-tauri/src/audio/capture.rs`

The trait abstracts the two operations the pipeline uses: start streaming audio and stop streaming.

```rust
/// Trait for audio capture backends.
/// Production: wraps cpal. Tests: sends pre-recorded samples.
pub trait AudioCaptureBackend: Send {
    /// Start capturing audio.
    ///
    /// - `sender`: channel to send PCM chunks (mono f32) to the consumer.
    /// - `is_recording`: shared flag; the backend should stop sending when false.
    /// - `level_callback`: called with RMS level (~30 Hz) for waveform UI.
    fn start(
        &mut self,
        sender: std::sync::mpsc::Sender<Vec<f32>>,
        is_recording: std::sync::Arc<std::sync::atomic::AtomicBool>,
        level_callback: Box<dyn Fn(f32) + Send + 'static>,
    ) -> Result<(), String>;

    /// Stop capturing. Must be idempotent (calling stop when not started is a no-op).
    fn stop(&mut self);
}
```

**Production implementation:** The existing `AudioCapture` struct implements `AudioCaptureBackend` by delegating to its current `start`/`stop` methods. The implementation is trivial because the existing method signatures already match.

```rust
impl AudioCaptureBackend for AudioCapture {
    fn start(
        &mut self,
        sender: std::sync::mpsc::Sender<Vec<f32>>,
        is_recording: std::sync::Arc<std::sync::atomic::AtomicBool>,
        level_callback: Box<dyn Fn(f32) + Send + 'static>,
    ) -> Result<(), String> {
        // Existing AudioCapture::start body -- no change needed,
        // the current method already has this exact signature.
        self.start(sender, is_recording, level_callback)
    }

    fn stop(&mut self) {
        self.stop();
    }
}
```

Wait -- that would cause infinite recursion. Because the method names collide, the trait impl must call the struct's inherent methods explicitly. The cleanest approach: **rename the inherent methods** to `start_capture` / `stop_capture`, then implement the trait by calling those. Alternatively, since the signatures already match exactly, simply add `impl AudioCaptureBackend for AudioCapture` and remove the inherent methods, making the trait the sole interface. This is the cleanest path:

```rust
// Remove the inherent impl block for AudioCapture.
// The trait IS the interface now.
impl AudioCaptureBackend for AudioCapture {
    fn start(
        &mut self,
        sender: Sender<Vec<f32>>,
        is_recording: Arc<AtomicBool>,
        level_callback: Box<dyn Fn(f32) + Send + 'static>,
    ) -> Result<(), String> {
        // ... existing start() body moves here verbatim ...
    }

    fn stop(&mut self) {
        // ... existing stop() body moves here verbatim ...
    }
}
```

All call sites already call `capture.start(...)` and `capture.stop()` -- they continue to work because the method names and signatures are identical. The only change at call sites is that `audio/capture.rs` must be imported with `use crate::audio::capture::AudioCaptureBackend` wherever the methods are called through a `Box<dyn AudioCaptureBackend>`.

### 4.2 Trait: `LlmBackend`

**Location:** `src-tauri/src/llm/engine.rs`

The trait covers the methods used by the recording pipeline (`cleanup`) and by commands that operate on `state.llm_engine` (`request_raw`, `shutdown`). Full lifecycle methods (`spawn`, `load_model`) remain on the concrete `LlmEngine` struct because they are used in setup/teardown code that constructs the engine before storing it as `Box<dyn LlmBackend>`.

```rust
/// Trait for LLM transcript cleanup backends.
/// Production: Python sidecar via stdin/stdout JSON protocol.
/// Tests: returns canned or transformed text.
pub trait LlmBackend: Send {
    /// Clean up a raw transcript.
    /// Returns the cleaned text, or an error.
    fn cleanup(&mut self, text: &str) -> Result<String, String>;

    /// Send a raw JSON request and return the raw JSON response.
    /// Used by `commands/llm.rs` for protocol-level operations like
    /// `check_update` (line 69) that bypass the typed `cleanup()` API.
    fn request_raw(&mut self, req: &serde_json::Value) -> Result<serde_json::Value, String>;
}
```

**Why `request_raw` is on the trait:** `commands/llm.rs` line 69 calls `llm.request_raw()` on the engine taken from `state.llm_engine` (which will be `Box<dyn LlmBackend>` after this refactoring). Without it on the trait, that code path would fail to compile. The mock implementation returns a default `{"ok": true}` response.

**Production implementation:**

```rust
impl LlmBackend for LlmEngine {
    fn cleanup(&mut self, text: &str) -> Result<String, String> {
        // Existing LlmEngine::cleanup body -- the inherent method
        // is kept for backward compatibility, and the trait delegates to it.
        self.cleanup(text)
    }
}
```

Same recursion concern: since the inherent method and trait method have the same name, calling `self.cleanup(text)` inside the trait impl resolves to the trait method (infinite recursion). Fix: rename the inherent method to `cleanup_impl` and have both the trait impl and any remaining direct callers go through it. Or, as with AudioCapture, **move the body into the trait impl and remove the inherent method**:

```rust
impl LlmBackend for LlmEngine {
    fn cleanup(&mut self, text: &str) -> Result<String, String> {
        let resp = self.request(&serde_json::json!({
            "action": "cleanup",
            "text": text,
        }))?;
        // ... existing cleanup body ...
    }

    fn request_raw(&mut self, req: &serde_json::Value) -> Result<serde_json::Value, String> {
        self.request(req)  // delegates to the private inherent method
    }
}
```

The `request` method stays as an inherent method on `LlmEngine` (it is not part of the trait).

**AppState change:**

```rust
// Before:
pub llm_engine: TokioMutex<Option<LlmEngine>>,

// After:
pub llm_engine: TokioMutex<Option<Box<dyn LlmBackend>>>,
```

All call sites that do `llm.cleanup(text)` continue to work because `Box<dyn LlmBackend>` has the same `.cleanup()` method.

Call sites that access `LlmEngine`-specific methods (like `load_model`, `quit`, `status`, `request_raw`) need adjustment. These exist in:
- `src-tauri/src/hotkeys/manager.rs` (line ~524): spawns sidecar and calls `load_model()`, then stores as `Box<dyn LlmBackend>`.
- `src-tauri/src/commands/llm.rs`: various LLM management commands.
- `src-tauri/src/lib.rs` (line ~194): pre-loads the sidecar at startup.

For the spawn-and-store pattern, the fix is straightforward:
```rust
// Before:
*llm_guard = Some(engine);

// After:
*llm_guard = Some(Box::new(engine) as Box<dyn LlmBackend>);
```

For `commands/llm.rs`, which needs `LlmEngine`-specific methods (download, load, unload, status), the cleanest approach is to keep a **separate** `TokioMutex<Option<LlmEngine>>` for management operations, while the `Box<dyn LlmBackend>` in `AppState` is the one used by the pipeline. However, this creates two sources of truth.

**Simpler approach:** Extend the `LlmBackend` trait to include the management methods needed by `commands/llm.rs`, or use `Any` downcasting. But this pollutes the trait with lifecycle concerns that mocks don't need.

**Recommended approach:** The `commands/llm.rs` functions that manage the sidecar lifecycle do NOT go through `AppState.llm_engine`. Instead they have their own local `LlmEngine` instances for downloads/status checks. The only shared state is the pipeline's `llm_engine` field. Looking at the actual code:

- `handle_stop_recording` spawns `LlmEngine::spawn()` and stores it in `state.llm_engine`.
- `commands/llm.rs:load_llm_model` also spawns and stores in `state.llm_engine`.
- `commands/llm.rs:unload_llm_model` takes from `state.llm_engine` and calls `.quit()`.

The `quit()` call is problematic because `Box<dyn LlmBackend>` does not expose `quit()`. Fix: add a `shutdown` method to `LlmBackend` with a default no-op implementation:

```rust
pub trait LlmBackend: Send {
    fn cleanup(&mut self, text: &str) -> Result<String, String>;

    fn request_raw(&mut self, req: &serde_json::Value) -> Result<serde_json::Value, String>;

    /// Shut down the backend. Default is a no-op.
    /// Production impl kills the sidecar process.
    fn shutdown(&mut self) {}
}
```

Then `unload_llm_model` calls `llm.shutdown()` instead of `llm.quit()`, and `LlmEngine::shutdown()` calls `self.quit()`.

### 4.3 Trait: `PasteBackend`

**Location:** new file `src-tauri/src/paste/backend.rs`

Currently paste is implemented as free functions in `src-tauri/src/paste/macos.rs`. The pipeline calls two functions:
- `paste::paste_text(text, target_pid)` (from `handle_stop_recording`)
- `paste::paste_text_and_restore(text, target_pid)` (from `handle_stop_recording`)
- `paste::copy_to_clipboard(text)` (fallback in `handle_stop_recording`)
- `paste::get_frontmost_pid()` (from `handle_start_recording`)

```rust
/// Trait for paste/clipboard operations.
/// Production: CGEvent Cmd+V + arboard clipboard on macOS.
/// Tests: records pasted text for assertion.
pub trait PasteBackend: Send + Sync {
    /// Paste text at the cursor position in the target app.
    fn paste_text(&self, text: &str, target_pid: i32) -> Result<(), String>;

    /// Paste text and restore the original clipboard contents.
    fn paste_text_and_restore(&self, text: &str, target_pid: i32) -> Result<(), String>;

    /// Copy text to the clipboard (without pasting).
    fn copy_to_clipboard(&self, text: &str) -> Result<(), String>;

    /// Get the PID of the frontmost application. Returns 0 if unknown.
    fn get_frontmost_pid(&self) -> i32;

    /// Check if accessibility permission is granted.
    fn is_accessibility_trusted(&self) -> bool;
}
```

**Production implementations:**

```rust
/// Production paste backend using macOS CGEvent + arboard.
#[cfg(target_os = "macos")]
pub struct MacOsPasteBackend;

#[cfg(target_os = "macos")]
impl PasteBackend for MacOsPasteBackend {
    fn paste_text(&self, text: &str, target_pid: i32) -> Result<(), String> {
        super::macos::paste_text(text, target_pid)
    }

    fn paste_text_and_restore(&self, text: &str, target_pid: i32) -> Result<(), String> {
        super::macos::paste_text_and_restore(text, target_pid)
    }

    fn copy_to_clipboard(&self, text: &str) -> Result<(), String> {
        super::macos::copy_to_clipboard(text)
    }

    fn get_frontmost_pid(&self) -> i32 {
        super::macos::get_frontmost_pid()
    }

    fn is_accessibility_trusted(&self) -> bool {
        super::macos::is_accessibility_trusted()
    }
}
```

```rust
/// Stub paste backend for non-macOS platforms. All operations return errors.
#[cfg(not(target_os = "macos"))]
pub struct StubPasteBackend;

#[cfg(not(target_os = "macos"))]
impl PasteBackend for StubPasteBackend {
    fn paste_text(&self, _text: &str, _target_pid: i32) -> Result<(), String> {
        Err("Paste not supported on this platform".into())
    }
    fn paste_text_and_restore(&self, _text: &str, _target_pid: i32) -> Result<(), String> {
        Err("Paste not supported on this platform".into())
    }
    fn copy_to_clipboard(&self, _text: &str) -> Result<(), String> {
        Err("Clipboard not supported on this platform".into())
    }
    fn get_frontmost_pid(&self) -> i32 { 0 }
    fn is_accessibility_trusted(&self) -> bool { true }
}
```

**Note on setup-only functions:** `test_accessibility_functional()` and `warmup_cgevent_pipeline()` in `paste/macos.rs` are NOT on the `PasteBackend` trait. They are one-time setup functions called from `lib.rs` during app initialization, not pipeline operations. They remain as free functions called directly from the production setup code.

**AppState change:**

```rust
// New field:
pub paste_backend: Box<dyn PasteBackend>,
```

In production `AppState::new()`:
```rust
#[cfg(target_os = "macos")]
let paste: Box<dyn PasteBackend> = Box::new(crate::paste::MacOsPasteBackend);
#[cfg(not(target_os = "macos"))]
let paste: Box<dyn PasteBackend> = Box::new(crate::paste::StubPasteBackend);

// ...
paste_backend: paste,
```

Call sites in `hotkeys/manager.rs` change from:
```rust
crate::paste::paste_text_and_restore(&final_text, target_pid)
```
to:
```rust
state.paste_backend.paste_text_and_restore(&final_text, target_pid)
```

The `PasteBackend` trait uses `&self` (not `&mut self`) because paste operations are stateless. This means the field does not need a `Mutex`; it can be stored directly as `Box<dyn PasteBackend>`. The `Send + Sync` bounds are required because `AppState` is shared across threads.

### 4.4 Existing Trait: `AsrEngine`

**Location:** `src-tauri/src/asr/engine.rs` -- already a trait.

No structural changes needed. `AppState` already holds `Box<dyn AsrEngine>`. We only need to create a `MockAsrEngine` for tests.

### 4.5 `AppState` Changes

**Location:** `src-tauri/src/state.rs`

```rust
use crate::audio::capture::AudioCaptureBackend;
use crate::llm::engine::LlmBackend;
use crate::paste::PasteBackend;

pub struct AppState {
    pub current_state: StdMutex<AppStateEnum>,
    pub settings: TokioMutex<Settings>,
    pub last_transcription: TokioMutex<Option<Transcription>>,
    pub is_recording: AtomicBool,
    pub is_model_loaded: AtomicBool,

    // ---- Changed fields ----
    pub audio_capture: StdMutex<Box<dyn AudioCaptureBackend>>,       // was: StdMutex<AudioCapture>
    pub asr_engine: TokioMutex<Box<dyn AsrEngine>>,                  // unchanged
    pub llm_engine: TokioMutex<Option<Box<dyn LlmBackend>>>,         // was: TokioMutex<Option<LlmEngine>>
    pub paste_backend: Box<dyn PasteBackend>,                         // new field

    // ---- Unchanged fields ----
    pub audio_sender: StdMutex<std::sync::mpsc::Sender<Vec<f32>>>,
    pub audio_receiver: StdMutex<std::sync::mpsc::Receiver<Vec<f32>>>,
    pub current_job_id: AtomicU64,
    pub cancel_shortcut: StdMutex<String>,
    pub cancel_shortcut_alt: StdMutex<Option<String>>,
    pub recording_generation: AtomicU64,
    pub target_pid: AtomicI32,
}
```

The `AppState::new()` constructor initializes the new trait objects with production implementations:

```rust
impl AppState {
    pub fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let settings = crate::commands::settings::load_persisted_settings();
        let cancel = settings.cancel_shortcut.clone();
        let cancel_alt = settings.cancel_shortcut_alt.clone();
        Self {
            // ... unchanged fields ...
            audio_capture: StdMutex::new(Box::new(crate::audio::capture::AudioCapture::new())),
            asr_engine: TokioMutex::new(crate::asr::engine::create_engine()),
            llm_engine: TokioMutex::new(None),
            paste_backend: Box::new(crate::paste::MacOsPasteBackend),
            // ... rest unchanged ...
        }
    }
}
```

A new `AppState::new_with_backends()` constructor is added for tests:

```rust
impl AppState {
    /// Construct AppState with injected backends. Used by integration tests.
    pub fn new_with_backends(
        audio: Box<dyn AudioCaptureBackend>,
        asr: Box<dyn AsrEngine>,
        llm: Option<Box<dyn LlmBackend>>,
        paste: Box<dyn PasteBackend>,
        settings: Settings,
    ) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let cancel = settings.cancel_shortcut.clone();
        let cancel_alt = settings.cancel_shortcut_alt.clone();
        Self {
            current_state: StdMutex::new(AppStateEnum::Idle),
            settings: TokioMutex::new(settings),
            last_transcription: TokioMutex::new(None),
            is_recording: AtomicBool::new(false),
            is_model_loaded: AtomicBool::new(true),
            audio_capture: StdMutex::new(audio),
            audio_sender: StdMutex::new(tx),
            audio_receiver: StdMutex::new(rx),
            asr_engine: TokioMutex::new(asr),
            llm_engine: TokioMutex::new(llm),
            paste_backend: paste,
            current_job_id: AtomicU64::new(0),
            cancel_shortcut: StdMutex::new(cancel),
            cancel_shortcut_alt: StdMutex::new(cancel_alt),
            recording_generation: AtomicU64::new(0),
            target_pid: AtomicI32::new(0),
        }
    }
}
```

### 4.6 Pipeline Extraction

The recording pipeline logic in `hotkeys/manager.rs` is tightly coupled to Tauri's `AppHandle` for:
- Accessing `AppState` via `app.state::<AppState>()`
- Emitting events via `app.emit("event-name", payload)`
- Showing/hiding the overlay window
- Registering/unregistering cancel shortcuts

For integration tests, we do NOT need the Tauri runtime. We need to extract the pipeline logic into functions that take `&AppState` directly (plus a callback or trait for side-effects like event emission).

**New module:** `src-tauri/src/pipeline.rs`

This module contains the pure orchestration logic, separated from Tauri-specific concerns:

```rust
use crate::asr::engine::AsrResult;
use crate::models::{AppStateEnum, Settings, Transcription};
use crate::state::AppState;

/// Events emitted during pipeline execution.
/// In production these map to Tauri app.emit() calls.
/// In tests they are collected for assertion.
pub trait PipelineEvents: Send {
    fn emit_state_changed(&self, state: &AppStateEnum);
    fn emit_recording_started(&self);
    fn emit_recording_stopped(&self);
    fn emit_recording_cancelled(&self);
    fn emit_transcription_complete(&self, transcription: &Transcription);
    fn emit_transcription_error(&self, error: &str);
    fn emit_paste_complete(&self, id: &str);
    fn emit_paste_error(&self, error: &str, text: &str);
    fn emit_audio_level(&self, level: f32);
    fn emit_recording_error(&self, error: &str);
}

/// Start recording: acquire mic, set state.
/// Returns Ok(()) if recording started, Err if mic failed or wrong state.
///
/// The `level_callback` is invoked by the audio backend at ~30 Hz with the
/// current RMS level. In production, the caller creates a callback that
/// emits Tauri events (`app.emit("audio-level", ...)`). In tests, pass a
/// no-op: `Box::new(|_| {})`.
///
/// Note: this does NOT handle overlay show/hide or cancel shortcut
/// registration -- those are Tauri-specific concerns handled by the caller.
pub fn pipeline_start_recording(
    state: &AppState,
    events: &dyn PipelineEvents,
    level_callback: Box<dyn Fn(f32) + Send + 'static>,
) -> Result<(), String> {
    let current = state.get_state();
    if current != AppStateEnum::Idle {
        return Err(format!("Cannot start recording: currently in {:?} state", current));
    }

    // Drain leftover samples
    if let Ok(rx) = state.audio_receiver.lock() {
        while rx.try_recv().is_ok() {}
    }

    // Start audio capture
    let sender = state.audio_sender.lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();

    let is_recording = std::sync::Arc::new(
        std::sync::atomic::AtomicBool::new(true)
    );
    state.is_recording.store(true, std::sync::atomic::Ordering::SeqCst);

    let is_recording_clone = is_recording.clone();

    let start_result = {
        let mut capture = state.audio_capture.lock()
            .unwrap_or_else(|e| e.into_inner());
        capture.start(sender, is_recording_clone, level_callback)
    };

    match start_result {
        Ok(()) => {
            let target_pid = state.paste_backend.get_frontmost_pid();
            state.target_pid.store(target_pid, std::sync::atomic::Ordering::SeqCst);

            state.set_state(AppStateEnum::Recording);
            events.emit_recording_started();
            events.emit_state_changed(&AppStateEnum::Recording);
            Ok(())
        }
        Err(e) => {
            state.is_recording.store(false, std::sync::atomic::Ordering::SeqCst);
            events.emit_recording_error(&e);
            Err(e)
        }
    }
}

/// Stop recording: collect samples, transcribe, optionally clean up, paste.
///
/// All intermediate results (transcription, paste status) are communicated
/// via the `events` trait and `state.last_transcription`. The return value
/// signals only whether the pipeline completed without a fatal error.
/// Tests assert on `state.last_transcription`, `events.state_changes`,
/// and the mock paste backend rather than a return struct.
pub async fn pipeline_stop_recording(
    state: &AppState,
    events: &dyn PipelineEvents,
) -> Result<(), String> {
    // --- Pseudocode: extracted from handle_stop_recording (manager.rs lines 353-676) ---

    // 1. Guard: check state is Recording (manager.rs:357-360)
    let current = state.get_state();
    if current != AppStateEnum::Recording {
        return Err(format!("Cannot stop recording: currently in {:?} state", current));
    }

    // 2. Stop capture (manager.rs:364-367)
    {
        let mut capture = state.audio_capture.lock().unwrap_or_else(|e| e.into_inner());
        capture.stop();
    }
    state.is_recording.store(false, std::sync::atomic::Ordering::SeqCst);

    // 3. Transition to Transcribing (manager.rs:376-378)
    state.set_state(AppStateEnum::Transcribing);
    events.emit_recording_stopped();
    events.emit_state_changed(&AppStateEnum::Transcribing);

    // 4. Collect samples from channel (manager.rs:382-405)
    let samples = {
        let rx = state.audio_receiver.lock().unwrap_or_else(|e| e.into_inner());
        let mut all = Vec::new();
        while let Ok(chunk) = rx.try_recv() {
            if all.len().saturating_add(chunk.len()) > MAX_AUDIO_BUFFER_SAMPLES {
                state.set_state(AppStateEnum::Idle);
                events.emit_state_changed(&AppStateEnum::Idle);
                return Err("Recording too long".into());
            }
            all.extend(chunk);
        }
        all
    };

    // 5. Short recording check (manager.rs:412-417)
    if samples.len() < 4000 {
        state.set_state(AppStateEnum::Idle);
        events.emit_state_changed(&AppStateEnum::Idle);
        return Ok(());
    }

    // 6. Get sample rate from AudioCaptureBackend trait (replaces cpal query, manager.rs:424-432)
    let sample_rate = {
        let capture = state.audio_capture.lock().unwrap_or_else(|e| e.into_inner());
        capture.sample_rate()
    };

    // 7. Write WAV to temp file (manager.rs:434-466)
    //    Write samples + 750ms silence padding, finalize with hound.
    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join(format!("sotto_{}.wav", std::process::id()));
    // ... write samples to WAV via hound::WavWriter ...
    let temp_path_str = temp_path.to_string_lossy().to_string();

    // 8. Assign job ID (manager.rs:473)
    let job_id = state.new_job();

    // 9. Transcribe via ASR engine (manager.rs:477-481)
    let mut engine = state.asr_engine.lock().await;
    let result = engine.transcribe_file(&temp_path_str);
    drop(engine);

    // 10. Handle ASR result (manager.rs:487-672)
    match result {
        Ok(asr_result) => {
            // 10a. Check job staleness (manager.rs:492-498)
            if !state.is_current_job(job_id) {
                state.set_state(AppStateEnum::Idle);
                events.emit_state_changed(&AppStateEnum::Idle);
                return Ok(());
            }

            let raw_asr_text = asr_result.text.clone();
            let mut final_text = asr_result.text.clone();
            let mut llm_was_applied = false;

            // 10b. Read settings (manager.rs:505-510)
            let settings = state.settings.lock().await;
            let llm_enabled = settings.llm_cleanup_enabled;
            let auto_paste = settings.auto_paste;
            let restore_clipboard = settings.restore_clipboard;
            let restore_focus_before_paste = settings.restore_focus_before_paste;
            drop(settings);

            // 10c. LLM cleanup if enabled and text >= 5 words (manager.rs:512-575)
            //      NOTE: pipeline does NOT auto-spawn the sidecar.
            //      If llm_engine is None, cleanup is skipped with a warning.
            if llm_enabled && final_text.split_whitespace().count() >= 5 {
                state.set_state(AppStateEnum::CleaningUp);
                events.emit_state_changed(&AppStateEnum::CleaningUp);

                let mut llm_guard = state.llm_engine.lock().await;
                if llm_guard.is_none() {
                    log::warn!("LLM cleanup enabled but no engine available, skipping");
                } else if let Some(mut llm) = llm_guard.take() {
                    // Take/put pattern: take the engine out of the mutex so we can
                    // pass it to spawn_blocking (which requires 'static).
                    // llm.cleanup() is blocking I/O (stdin/stdout to sidecar),
                    // so it MUST run on a blocking thread.
                    let text_for_cleanup = final_text.clone();
                    let cleanup_result = tokio::time::timeout(
                        std::time::Duration::from_secs(120),
                        tokio::task::spawn_blocking(move || {
                            let result = llm.cleanup(&text_for_cleanup);
                            (llm, result)
                        }),
                    ).await;

                    match cleanup_result {
                        Ok(Ok((returned_llm, Ok(cleaned)))) => {
                            final_text = cleaned;
                            llm_was_applied = true;
                            *llm_guard = Some(returned_llm);
                        }
                        Ok(Ok((returned_llm, Err(e)))) => {
                            log::warn!("LLM cleanup failed: {}", e);
                            *llm_guard = Some(returned_llm);
                        }
                        Ok(Err(e)) => {
                            log::error!("LLM cleanup task panicked: {}", e);
                            // Engine is lost (consumed by panicked task)
                        }
                        Err(_) => {
                            log::warn!("LLM cleanup timed out after 120s");
                            // Engine is lost (still held by timed-out task)
                        }
                    }
                }
            }

            // 10d. Second staleness check (manager.rs:577-584)
            if !state.is_current_job(job_id) {
                state.set_state(AppStateEnum::Idle);
                events.emit_state_changed(&AppStateEnum::Idle);
                return Ok(());
            }

            // 10e. Build Transcription struct (manager.rs:586-595)
            // 10f. Save to state.last_transcription + add_transcription (manager.rs:598-603)
            // 10g. Emit transcription-complete event (manager.rs:604)

            // 10h. Paste or copy to clipboard (manager.rs:607-660)
            //      Uses state.paste_backend instead of free functions.

            // 10i. Set state to Idle (manager.rs:671-672)
        }
        Err(e) => {
            events.emit_transcription_error(&e);
        }
    }

    state.set_state(AppStateEnum::Idle);
    events.emit_state_changed(&AppStateEnum::Idle);
    Ok(())
}

/// Cancel recording: stop mic, optionally transcribe, save as cancelled.
pub async fn pipeline_cancel_recording(
    state: &AppState,
    events: &dyn PipelineEvents,
) -> Option<Transcription> {
    // ... orchestration logic extracted from handle_cancel_recording ...
    todo!()
}
```

The key principle: `hotkeys/manager.rs` continues to be the Tauri-facing entry point. It calls `pipeline_start_recording(state, &TauriEvents(app), level_cb)` internally, where `level_cb` emits Tauri audio-level events. The integration tests call `pipeline_start_recording(state, &CollectingEvents::new(), Box::new(|_| {}))` with mock backends and a no-op level callback.

### 4.7 `handle_stop_recording` Pipeline Extraction Details

The existing `handle_stop_recording` in `hotkeys/manager.rs` does the following (annotated with what changes):

1. **Guard:** Check state is `Recording` -- pure logic, extracts directly.
2. **Stop capture:** `capture.stop()` -- now calls trait method, works unchanged.
3. **Set `is_recording` to false** -- pure state mutation.
4. **Unregister cancel shortcut** -- Tauri-specific, stays in `manager.rs`.
5. **Set state to `Transcribing`**, emit events -- uses `PipelineEvents` trait.
6. **Collect samples from channel** -- pure logic, extracts directly.
7. **Buffer limit check** -- pure logic.
8. **Short recording check (<4000 samples)** -- pure logic.
9. **Get sample rate from cpal** -- this queries the cpal host for the default device's sample rate. For tests, we need to make this configurable. Add a `sample_rate` field to `AppState` or pass it as a parameter.
10. **Write WAV to temp file** -- I/O but deterministic. Can extract, uses hound directly.
11. **Call `asr_engine.transcribe_file()`** -- goes through trait, works unchanged.
12. **Delete temp file** -- I/O cleanup.
13. **Check job staleness** -- pure logic.
14. **LLM cleanup** -- goes through `LlmBackend` trait, works unchanged. The sidecar spawn logic (calling `LlmEngine::spawn()` and `load_model()`) only executes when `llm_engine` is `None`. In tests, we pre-populate it with a mock, so the spawn path is never hit.
15. **Create `Transcription`** -- pure struct construction.
16. **Save transcription** -- calls `add_transcription()`. For tests, we can either mock the store or let it write to disk (it is idempotent). Better: extract into a `TranscriptionStore` trait or just assert on `state.last_transcription`.
17. **Paste text** -- now calls `state.paste_backend.paste_text()` or `paste_text_and_restore()`.
18. **Set state to Idle** -- pure logic.

**Sample rate concern (item 9):** The current code queries `cpal::default_host()` inline to get the sample rate (manager.rs lines 424-432). This is a hardware dependency that cannot be used in tests.

**Fix:** Add a `sample_rate() -> u32` method to `AudioCaptureBackend`:

```rust
pub trait AudioCaptureBackend: Send {
    fn start(...) -> Result<(), String>;
    fn stop(&mut self);

    /// The sample rate of the captured audio. Valid after start() succeeds.
    /// Returns the rate used by the cpal stream (production) or a fixed value (tests).
    fn sample_rate(&self) -> u32;
}
```

The production `AudioCapture` stores the sample rate when `start()` configures the cpal stream (it already knows it from `default_input_config()`). Before `start()` is called, it returns a default of 48000 Hz. The mock returns a fixed 48000 Hz.

The pipeline calls `capture.sample_rate()` instead of querying `cpal::default_host()` inline. This replaces the inline cpal query entirely -- there is no separate `AtomicU32` field on `AppState`.

### 4.8 Mock Implementations

All mocks live in `src-tauri/src/test_support.rs`, gated by `#[cfg(test)]`. This file is a module declared in `lib.rs` with `#[cfg(test)] mod test_support;`.

#### MockAudioCapture

```rust
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

/// Mock audio capture that sends pre-loaded PCM samples when started.
pub struct MockAudioCapture {
    /// Samples to send when start() is called.
    samples: Vec<f32>,
    /// Sample rate to report.
    sample_rate: u32,
}

impl MockAudioCapture {
    /// Create a mock that sends the given samples as a single chunk.
    pub fn new(samples: Vec<f32>, sample_rate: u32) -> Self {
        Self { samples, sample_rate }
    }

    /// Create a mock with 1 second of silence at 48kHz.
    pub fn silence() -> Self {
        Self::new(vec![0.0f32; 48_000], 48_000)
    }

    /// Create a mock with a synthetic 440Hz sine wave (1 second at 48kHz).
    pub fn sine_wave() -> Self {
        let sample_rate = 48_000u32;
        let samples: Vec<f32> = (0..sample_rate)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / sample_rate as f32).sin())
            .collect();
        Self::new(samples, sample_rate)
    }
}

impl crate::audio::capture::AudioCaptureBackend for MockAudioCapture {
    fn start(
        &mut self,
        sender: Sender<Vec<f32>>,
        _is_recording: Arc<AtomicBool>,
        _level_callback: Box<dyn Fn(f32) + Send + 'static>,
    ) -> Result<(), String> {
        // Send all samples immediately as a single chunk
        sender.send(self.samples.clone())
            .map_err(|e| format!("MockAudioCapture: channel send failed: {}", e))?;
        Ok(())
    }

    fn stop(&mut self) {
        // No-op: mock has no stream to stop
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}
```

#### MockAsrEngine

```rust
use crate::asr::engine::{AsrEngine, AsrResult};

/// Mock ASR engine that returns a canned transcription.
pub struct MockAsrEngine {
    /// Text to return from transcribe_file/transcribe_samples.
    response: Result<String, String>,
    /// Whether init() has been called.
    ready: bool,
}

impl MockAsrEngine {
    /// Create a mock that returns the given text.
    pub fn with_text(text: &str) -> Self {
        Self {
            response: Ok(text.to_string()),
            ready: true,
        }
    }

    /// Create a mock that returns an error.
    pub fn with_error(error: &str) -> Self {
        Self {
            response: Err(error.to_string()),
            ready: true,
        }
    }
}

impl AsrEngine for MockAsrEngine {
    fn init(&mut self) -> Result<(), String> {
        self.ready = true;
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.ready
    }

    fn transcribe_file(&mut self, _path: &str) -> Result<AsrResult, String> {
        match &self.response {
            Ok(text) => Ok(AsrResult {
                text: text.clone(),
                duration_secs: 1.0,
                processing_time_secs: 0.01,
                rtfx: 100.0,
            }),
            Err(e) => Err(e.clone()),
        }
    }

    fn transcribe_samples(&mut self, _samples: &[f32], _sample_rate: u32) -> Result<AsrResult, String> {
        self.transcribe_file("")  // Delegate to same logic
    }

    fn is_model_available(&self) -> bool {
        true
    }

    fn backend_name(&self) -> &'static str {
        "mock"
    }
}
```

#### MockLlmBackend

```rust
use crate::llm::engine::LlmBackend;

/// Mock LLM backend that applies a canned transformation.
pub struct MockLlmBackend {
    /// The transformation to apply. None means return text unchanged.
    transform: Option<Box<dyn Fn(&str) -> Result<String, String> + Send>>,
}

impl MockLlmBackend {
    /// Create a mock that returns the given fixed text regardless of input.
    pub fn fixed(output: &str) -> Self {
        let output = output.to_string();
        Self {
            transform: Some(Box::new(move |_| Ok(output.clone()))),
        }
    }

    /// Create a mock that returns an error.
    pub fn failing(error: &str) -> Self {
        let error = error.to_string();
        Self {
            transform: Some(Box::new(move |_| Err(error.clone()))),
        }
    }

    /// Create a mock that passes text through unchanged.
    pub fn passthrough() -> Self {
        Self { transform: None }
    }
}

impl LlmBackend for MockLlmBackend {
    fn cleanup(&mut self, text: &str) -> Result<String, String> {
        match &self.transform {
            Some(f) => f(text),
            None => Ok(text.to_string()),
        }
    }

    fn request_raw(&mut self, _req: &serde_json::Value) -> Result<serde_json::Value, String> {
        // Return a generic success response for protocol-level operations
        Ok(serde_json::json!({"ok": true}))
    }
}
```

#### MockPasteBackend

```rust
use crate::paste::PasteBackend;
use std::sync::Mutex;

/// Mock paste backend that records operations for assertion.
pub struct MockPasteBackend {
    /// All texts that were "pasted" (via paste_text or paste_text_and_restore).
    pub pasted_texts: Mutex<Vec<PasteRecord>>,
    /// All texts that were "copied" to clipboard.
    pub copied_texts: Mutex<Vec<String>>,
    /// PID to return from get_frontmost_pid.
    pub frontmost_pid: i32,
    /// Whether to report accessibility as trusted.
    pub accessibility_trusted: bool,
    /// If set, paste operations return this error.
    pub paste_error: Mutex<Option<String>>,
}

#[derive(Debug, Clone)]
pub struct PasteRecord {
    pub text: String,
    pub target_pid: i32,
    pub restore_clipboard: bool,
}

impl MockPasteBackend {
    pub fn new() -> Self {
        Self {
            pasted_texts: Mutex::new(Vec::new()),
            copied_texts: Mutex::new(Vec::new()),
            frontmost_pid: 12345,
            accessibility_trusted: true,
            paste_error: Mutex::new(None),
        }
    }

    /// Get the most recently pasted text.
    pub fn last_pasted(&self) -> Option<PasteRecord> {
        self.pasted_texts.lock().unwrap().last().cloned()
    }
}

impl PasteBackend for MockPasteBackend {
    fn paste_text(&self, text: &str, target_pid: i32) -> Result<(), String> {
        if let Some(err) = self.paste_error.lock().unwrap().as_ref() {
            return Err(err.clone());
        }
        self.pasted_texts.lock().unwrap().push(PasteRecord {
            text: text.to_string(),
            target_pid,
            restore_clipboard: false,
        });
        Ok(())
    }

    fn paste_text_and_restore(&self, text: &str, target_pid: i32) -> Result<(), String> {
        if let Some(err) = self.paste_error.lock().unwrap().as_ref() {
            return Err(err.clone());
        }
        self.pasted_texts.lock().unwrap().push(PasteRecord {
            text: text.to_string(),
            target_pid,
            restore_clipboard: true,
        });
        Ok(())
    }

    fn copy_to_clipboard(&self, text: &str) -> Result<(), String> {
        self.copied_texts.lock().unwrap().push(text.to_string());
        Ok(())
    }

    fn get_frontmost_pid(&self) -> i32 {
        self.frontmost_pid
    }

    fn is_accessibility_trusted(&self) -> bool {
        self.accessibility_trusted
    }
}
```

#### CollectingEvents (mock PipelineEvents)

```rust
use crate::models::{AppStateEnum, Transcription};
use crate::pipeline::PipelineEvents;
use std::sync::Mutex;

/// Collects all pipeline events for assertion in tests.
pub struct CollectingEvents {
    pub state_changes: Mutex<Vec<AppStateEnum>>,
    pub recording_started: Mutex<bool>,
    pub recording_stopped: Mutex<bool>,
    pub recording_cancelled: Mutex<bool>,
    pub transcriptions: Mutex<Vec<Transcription>>,
    pub errors: Mutex<Vec<String>>,
    pub paste_ids: Mutex<Vec<String>>,
    pub paste_errors: Mutex<Vec<(String, String)>>,
}

impl CollectingEvents {
    pub fn new() -> Self {
        Self {
            state_changes: Mutex::new(Vec::new()),
            recording_started: Mutex::new(false),
            recording_stopped: Mutex::new(false),
            recording_cancelled: Mutex::new(false),
            transcriptions: Mutex::new(Vec::new()),
            errors: Mutex::new(Vec::new()),
            paste_ids: Mutex::new(Vec::new()),
            paste_errors: Mutex::new(Vec::new()),
        }
    }
}

impl PipelineEvents for CollectingEvents {
    fn emit_state_changed(&self, state: &AppStateEnum) {
        self.state_changes.lock().unwrap().push(state.clone());
    }
    fn emit_recording_started(&self) {
        *self.recording_started.lock().unwrap() = true;
    }
    fn emit_recording_stopped(&self) {
        *self.recording_stopped.lock().unwrap() = true;
    }
    fn emit_recording_cancelled(&self) {
        *self.recording_cancelled.lock().unwrap() = true;
    }
    fn emit_transcription_complete(&self, t: &Transcription) {
        self.transcriptions.lock().unwrap().push(t.clone());
    }
    fn emit_transcription_error(&self, error: &str) {
        self.errors.lock().unwrap().push(error.to_string());
    }
    fn emit_paste_complete(&self, id: &str) {
        self.paste_ids.lock().unwrap().push(id.to_string());
    }
    fn emit_paste_error(&self, error: &str, text: &str) {
        self.paste_errors.lock().unwrap()
            .push((error.to_string(), text.to_string()));
    }
    fn emit_audio_level(&self, _level: f32) {}
    fn emit_recording_error(&self, error: &str) {
        self.errors.lock().unwrap().push(error.to_string());
    }
}
```

### 4.9 Integration Test Structure

Integration tests live in `src-tauri/tests/pipeline_integration.rs` (Rust convention: files in `tests/` are integration tests that can only access the crate's public API).

However, our test targets (`test_support`, `pipeline`) are internal modules. For integration tests to access mocks and pipeline functions, those modules must be public (or `pub(crate)` won't work from `tests/`).

**Decision:** Make the tests **unit-style integration tests** inside `src-tauri/src/pipeline.rs` as `#[cfg(test)] mod tests { ... }`. This gives access to all crate internals while still testing the full pipeline.

Why this is still an "integration test": it exercises multiple modules working together (audio, ASR, LLM, paste, state) through the pipeline orchestration, unlike Phase 1 unit tests that test individual functions in isolation.

```rust
// At bottom of src-tauri/src/pipeline.rs

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use crate::models::{AppStateEnum, Settings};

    /// Helper: create an AppState with standard mock backends.
    ///
    /// Accepts `Box<dyn PasteBackend>` (not concrete `MockPasteBackend`)
    /// so callers can pass `Box::new(SharedMockPaste(...))` for tests
    /// that need to inspect paste results via an `Arc<MockPasteBackend>`.
    fn test_state(
        audio: MockAudioCapture,
        asr: MockAsrEngine,
        llm: Option<MockLlmBackend>,
        paste: Box<dyn crate::paste::PasteBackend>,
    ) -> crate::state::AppState {
        let mut settings = Settings::default();
        settings.auto_paste = true;
        settings.restore_clipboard = true;
        settings.restore_focus_before_paste = true;
        settings.llm_cleanup_enabled = llm.is_some();

        crate::state::AppState::new_with_backends(
            Box::new(audio),
            Box::new(asr),
            llm.map(|l| Box::new(l) as Box<dyn crate::llm::engine::LlmBackend>),
            paste,
            settings,
        )
    }

    // --- Tests follow ---
}
```

---

## 5. Edge Cases

### 5.1 AudioCapture Send Safety

`AudioCapture` currently has `unsafe impl Send for AudioCapture` because `cpal::Stream` is not `Send`. The trait `AudioCaptureBackend: Send` requires all implementations to be `Send`. The production `AudioCapture` keeps its unsafe `Send` impl. Mocks are naturally `Send` (no cpal types).

### 5.2 LlmEngine Send Safety

`LlmEngine` has `unsafe impl Send for LlmEngine` because it holds `Child`, `BufWriter<ChildStdin>`, and `BufReader<ChildStdout>`, which are `Send` in practice but not auto-derived due to raw pointers in the process types. The `LlmBackend: Send` trait bound is satisfied by this unsafe impl. Mocks are naturally `Send`.

### 5.3 Trait Object Dispatch and Performance

Using `Box<dyn Trait>` introduces virtual dispatch (vtable lookup) on every call. For the recording pipeline, this is negligible:
- `start()`/`stop()` are called once per recording.
- `transcribe_file()` takes 100ms-10s -- one vtable lookup is immeasurable.
- `cleanup()` takes 50-500ms.
- `paste_text()` takes 30-200ms.

No performance concern.

### 5.4 Concurrent Test Execution

Rust runs `#[test]` functions in parallel by default. Our tests share no global state (each creates its own `AppState`), so parallel execution is safe. The `TRANSCRIPTIONS` static in `commands/transcription.rs` IS global, but the pipeline tests will assert on `state.last_transcription` rather than the global store, avoiding cross-test interference.

If the pipeline calls `add_transcription()` (which writes to the global store and disk at `~/Library/Application Support/com.sottoasr.app/transcriptions.json`), tests could interfere with each other and pollute the user's real transcription history.

Mitigations (applied together):
1. **`#[serial_test::serial]`** for pipeline tests that produce transcriptions, preventing race conditions on the global `TRANSCRIPTIONS` static.
2. **Assert on `state.last_transcription`** rather than the global store, so test correctness does not depend on disk writes.
3. **Accept the disk side effect for now.** The `add_transcription` call writes to the user's real data directory. This is a known impurity. A future Phase could introduce a `TranscriptionStore` trait with a temp-directory-backed implementation for tests, but that is out of scope for Phase 2.

**Decision:** Use `#[serial_test::serial]` and assert on `state.last_transcription`. Accept the disk write as a tolerable side effect -- it appends a test transcription that the user can delete, and tests are idempotent. Add `serial_test` as a dev dependency.

### 5.5 The Sidecar Spawn Path

In `handle_stop_recording`, when `llm_engine` is `None` and LLM cleanup is enabled, the code spawns a real `LlmEngine::spawn()`. In tests, `llm_engine` is pre-populated with a `MockLlmBackend`, so the spawn path is never hit. If a test needs to verify the "LLM not available" path, it sets `llm_engine` to `None` and `llm_cleanup_enabled` to `true`. The pipeline will attempt to spawn -- this path should be refactored to go through a factory trait, but for Phase 2 we simply skip the spawn by checking if the `llm_engine` is `None` and there is no factory available (mocked as always-None).

**Concrete fix:** The spawn logic in `pipeline_stop_recording` should be:
```rust
if llm_guard.is_none() {
    // In production, the caller (hotkeys/manager.rs) handles spawning.
    // The pipeline function does NOT spawn -- it only uses what is provided.
    log::warn!("LLM cleanup enabled but no engine available, skipping cleanup");
}
```

This is a design improvement: the pipeline function should not be responsible for spawning infrastructure. The spawn-on-demand logic stays in `hotkeys/manager.rs` which calls `pipeline_stop_recording` after ensuring the engine is available.

**Caller pseudocode for sidecar spawn-on-demand (in `handle_stop_recording`):**

```rust
// In hotkeys/manager.rs, BEFORE calling pipeline_stop_recording:
let settings = state.settings.lock().await;
let llm_enabled = settings.llm_cleanup_enabled;
drop(settings);

if llm_enabled {
    let mut llm_guard = state.llm_engine.lock().await;
    if llm_guard.is_none() {
        log::info!("Spawning LLM sidecar (on-demand)...");
        match tokio::task::spawn_blocking(move || {
            let mut engine = crate::llm::engine::LlmEngine::spawn()?;
            engine.load_model()?;
            Ok::<_, String>(engine)
        }).await {
            Ok(Ok(engine)) => {
                *llm_guard = Some(Box::new(engine) as Box<dyn LlmBackend>);
            }
            Ok(Err(e)) => {
                log::warn!("Failed to spawn LLM sidecar: {}", e);
                // Pipeline will skip LLM cleanup -- llm_guard remains None
            }
            Err(e) => {
                log::error!("LLM sidecar spawn panicked: {}", e);
            }
        }
    }
    drop(llm_guard);
}

// Now call the pipeline -- it uses whatever is in state.llm_engine
pipeline_stop_recording(&state, &events).await;
```

In tests, `state.llm_engine` is pre-populated with a `MockLlmBackend`, so the spawn path is never hit. For testing the "LLM not available" path, set `llm_cleanup_enabled = true` and `llm_engine = None`; the pipeline will log the warning and skip cleanup.

### 5.6 WAV File I/O in Tests

The pipeline writes a WAV file to a temp directory, then passes the path to `asr_engine.transcribe_file()`. In tests, `MockAsrEngine.transcribe_file()` ignores the path. The WAV file is still written and deleted. This is acceptable:
- Temp file I/O is fast (~1ms for a few hundred KB).
- It tests the WAV writing logic (hound) for free.
- No cleanup concerns (temp dir is ephemeral).

### 5.7 Tokio Runtime in Tests

`pipeline_stop_recording` is `async`. Tests need a Tokio runtime. Use `#[tokio::test]` attribute:

```rust
#[tokio::test]
async fn test_happy_path() {
    let state = test_state(...);
    let events = CollectingEvents::new();
    pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
    pipeline_stop_recording(&state, &events).await;
    // assertions...
}
```

---

## 6. File Changes

| File | Action | Description |
|------|--------|-------------|
| `src-tauri/src/audio/capture.rs` | Modify | Add `AudioCaptureBackend` trait. Move `AudioCapture` method bodies into trait impl. Add `sample_rate()` method to trait. |
| `src-tauri/src/llm/engine.rs` | Modify | Add `LlmBackend` trait with `cleanup()`, `request_raw()`, and `shutdown()`. Implement for `LlmEngine`. Remove inherent `cleanup()` method (moved to trait impl). `request_raw()` delegates to the private `request()` method. |
| `src-tauri/src/paste/backend.rs` | Create | Define `PasteBackend` trait. Implement `MacOsPasteBackend` (`#[cfg(target_os = "macos")]`) and `StubPasteBackend` (`#[cfg(not(target_os = "macos"))]`). |
| `src-tauri/src/paste/mod.rs` | Modify | Add `pub mod backend;` and re-export `PasteBackend`, `MacOsPasteBackend`, `StubPasteBackend`. |
| `src-tauri/src/state.rs` | Modify | Change `audio_capture` to `Box<dyn AudioCaptureBackend>`, `llm_engine` to `Option<Box<dyn LlmBackend>>`. Add `paste_backend: Box<dyn PasteBackend>`. Add `new_with_backends()` constructor. |
| `src-tauri/src/pipeline.rs` | Create | Define `PipelineEvents` trait. Extract `pipeline_start_recording`, `pipeline_stop_recording`, `pipeline_cancel_recording` from `hotkeys/manager.rs`. Integration tests as `#[cfg(test)] mod tests`. |
| `src-tauri/src/test_support.rs` | Create | Mock implementations: `MockAudioCapture`, `MockAsrEngine`, `MockLlmBackend`, `MockPasteBackend`, `CollectingEvents`. Gated by `#[cfg(test)]`. |
| `src-tauri/src/hotkeys/manager.rs` | Modify | Replace inline pipeline logic with calls to `pipeline::pipeline_start_recording` / `pipeline_stop_recording` / `pipeline_cancel_recording`. Keep Tauri-specific overlay/shortcut logic as wrapper. Update paste calls to use `state.paste_backend`. Update `cpal::default_host()` sample rate query to use `capture.sample_rate()`. |
| `src-tauri/src/commands/llm.rs` | Modify | Update `state.llm_engine` usage to work with `Box<dyn LlmBackend>`. Replace the 3 `.quit()` calls on engines taken from `state.llm_engine` (in `update_llm_model`, `delete_llm_model`, and `unload_llm_model`) with `.shutdown()`. The `.quit()` call in `check_llm_update` on the locally-spawned temporary sidecar (line 96) stays as `.quit()` since it operates on a concrete `LlmEngine`. Update `request_raw()` calls to go through trait method. Box `LlmEngine` as `Box<dyn LlmBackend>` in `load_llm_model` when storing into `state.llm_engine`. |
| `src-tauri/src/commands/recording.rs` | Modify | Update any direct `AudioCapture` usage to go through `AudioCaptureBackend` trait. |
| `src-tauri/src/llm/download.rs` | Modify | Update `LlmEngine` storage to `Box<dyn LlmBackend>` when storing into `state.llm_engine` (line 49: `*guard = Some(engine)` becomes `*guard = Some(Box::new(engine) as Box<dyn LlmBackend>)`). Replace the `.quit()` call on the engine taken from `state.llm_engine` with `.shutdown()`. The `.quit()` call on the locally-spawned download sidecar stays as-is. |
| `src-tauri/src/lib.rs` | Modify | Add `pub mod pipeline;` and `#[cfg(test)] mod test_support;`. Update LLM pre-load (line 202) to store as `Box<dyn LlmBackend>`. |
| `src-tauri/Cargo.toml` | Modify | Add `serial_test` as a dev dependency. |

---

## 7. Testing Strategy

### 7.1 Integration Tests (this spec -- the primary deliverable)

All tests in `src-tauri/src/pipeline.rs` under `#[cfg(test)] mod tests`.

#### Test 1: Happy path -- full recording pipeline with LLM

```rust
#[tokio::test]
#[serial_test::serial]
async fn test_full_pipeline_happy_path() {
    let mock_paste = Arc::new(MockPasteBackend::new());
    let state = test_state(
        MockAudioCapture::sine_wave(),
        MockAsrEngine::with_text("hello world"),
        Some(MockLlmBackend::fixed("Hello, world.")),
        Box::new(SharedMockPaste(mock_paste.clone())),
    );
    let events = CollectingEvents::new();

    // Start recording
    pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
    assert_eq!(state.get_state(), AppStateEnum::Recording);

    // Stop recording -- triggers transcription + LLM + paste
    pipeline_stop_recording(&state, &events).await.unwrap();

    // Assert final state
    assert_eq!(state.get_state(), AppStateEnum::Idle);

    // Assert transcription was saved
    let last = state.last_transcription.lock().await;
    let t = last.as_ref().expect("transcription should be saved");
    assert_eq!(t.text, "Hello, world.");
    assert_eq!(t.raw_text.as_deref(), Some("hello world"));
    assert!(t.llm_applied);
    assert!(!t.cancelled);

    // Assert paste occurred with cleaned text (via Arc<MockPasteBackend>)
    let pasted = mock_paste.last_pasted().expect("should have pasted");
    assert_eq!(pasted.text, "Hello, world.");
    assert!(pasted.restore_clipboard);
    assert!(!events.paste_ids.lock().unwrap().is_empty());

    // Assert state transitions
    let states = events.state_changes.lock().unwrap();
    assert!(states.contains(&AppStateEnum::Recording));
    assert!(states.contains(&AppStateEnum::Transcribing));
    assert!(states.contains(&AppStateEnum::CleaningUp));
    assert!(states.contains(&AppStateEnum::Idle));
}
```

**Note on paste assertion:** Since `paste_backend` is `Box<dyn PasteBackend>`, we cannot downcast it to `MockPasteBackend` without `Any`. Two options:
1. Add `as_any()` to the trait (common pattern but slightly ugly).
2. Use `Arc<MockPasteBackend>` and pass a clone to the state while keeping one for assertions.

**Decision:** Use `Arc<MockPasteBackend>`. The `PasteBackend` trait requires `Send + Sync`, which `Arc` satisfies. Wrap the mock in `Arc` and implement `PasteBackend for Arc<MockPasteBackend>`:

```rust
// PasteBackend is already implemented for MockPasteBackend.
// Since MockPasteBackend uses interior mutability (Mutex), &self methods work.
// We pass Arc<MockPasteBackend> to AppState (via Box) and keep a clone.

let paste = Arc::new(MockPasteBackend::new());
let paste_clone = paste.clone();
// state gets Box::new(ArcPasteWrapper(paste.clone()))
// or: implement PasteBackend for Arc<MockPasteBackend> by delegating to &*self
```

Simpler: store the `MockPasteBackend` behind an `Arc` and have the test builder wrap it in a newtype that implements `PasteBackend`:

```rust
pub struct SharedMockPaste(pub Arc<MockPasteBackend>);

impl PasteBackend for SharedMockPaste {
    fn paste_text(&self, text: &str, pid: i32) -> Result<(), String> {
        self.0.paste_text(text, pid)
    }
    // ... delegate all methods ...
}
```

Then in tests:
```rust
let mock_paste = Arc::new(MockPasteBackend::new());
let state = test_state(
    MockAudioCapture::sine_wave(),
    MockAsrEngine::with_text("hello world"),
    Some(MockLlmBackend::fixed("Hello, world.")),
    Box::new(SharedMockPaste(mock_paste.clone())),
);
// ... run pipeline ...
let pasted = mock_paste.last_pasted().expect("should have pasted");
assert_eq!(pasted.text, "Hello, world.");
assert!(pasted.restore_clipboard);
```

This pattern is clean and avoids `Any` downcasting.

#### Test 2: Pipeline without LLM

```rust
#[tokio::test]
#[serial_test::serial]
async fn test_pipeline_without_llm() {
    let mock_paste = Arc::new(MockPasteBackend::new());
    let state = test_state(
        MockAudioCapture::sine_wave(),
        MockAsrEngine::with_text("hello world"),
        None,  // No LLM
        Box::new(SharedMockPaste(mock_paste.clone())),
    );
    let events = CollectingEvents::new();

    pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
    pipeline_stop_recording(&state, &events).await.unwrap();

    // Assert raw ASR text was pasted (no LLM cleanup)
    let pasted = mock_paste.last_pasted().expect("should have pasted");
    assert_eq!(pasted.text, "hello world");

    // Assert transcription has no raw_text (LLM was not used)
    let last = state.last_transcription.lock().await;
    let t = last.as_ref().unwrap();
    assert!(!t.llm_applied);
    assert!(t.raw_text.is_none());
}
```

#### Test 3: ASR error

```rust
#[tokio::test]
#[serial_test::serial]
async fn test_pipeline_asr_error() {
    let mock_paste = Arc::new(MockPasteBackend::new());
    let state = test_state(
        MockAudioCapture::sine_wave(),
        MockAsrEngine::with_error("Model failed to load"),
        None,
        Box::new(SharedMockPaste(mock_paste.clone())),
    );
    let events = CollectingEvents::new();

    pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
    pipeline_stop_recording(&state, &events).await.unwrap();

    // Assert state returned to Idle
    assert_eq!(state.get_state(), AppStateEnum::Idle);

    // Assert no paste occurred
    assert!(mock_paste.last_pasted().is_none());
    assert!(mock_paste.copied_texts.lock().unwrap().is_empty());

    // Assert error was emitted via PipelineEvents (not via return value)
    let errors = events.errors.lock().unwrap();
    assert!(errors.iter().any(|e| e.contains("Model failed to load")));
}
```

#### Test 4: Recording cancellation

```rust
#[tokio::test]
#[serial_test::serial]
async fn test_pipeline_cancel() {
    let mock_paste = Arc::new(MockPasteBackend::new());
    let state = test_state(
        MockAudioCapture::sine_wave(),
        MockAsrEngine::with_text("hello world"),
        None,
        Box::new(SharedMockPaste(mock_paste.clone())),
    );
    let events = CollectingEvents::new();

    pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
    assert_eq!(state.get_state(), AppStateEnum::Recording);

    // Cancel instead of stop
    pipeline_cancel_recording(&state, &events).await;

    // Assert state returned to Idle
    assert_eq!(state.get_state(), AppStateEnum::Idle);

    // Assert no paste occurred
    assert!(mock_paste.last_pasted().is_none());

    // Assert cancellation event was emitted
    assert!(*events.recording_cancelled.lock().unwrap());

    // Assert transcription was saved as cancelled (if enough samples)
    // MockAudioCapture::sine_wave() provides 48000 samples (>4000),
    // so a cancelled transcription should be saved.
    let transcriptions = events.transcriptions.lock().unwrap();
    if !transcriptions.is_empty() {
        assert!(transcriptions[0].cancelled);
    }
}
```

#### Test 5: State machine guards

```rust
#[tokio::test]
async fn test_start_while_recording_fails() {
    let state = test_state(
        MockAudioCapture::sine_wave(),
        MockAsrEngine::with_text("hello"),
        None,
        Box::new(MockPasteBackend::new()),
    );
    let events = CollectingEvents::new();

    // Start recording
    pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
    assert_eq!(state.get_state(), AppStateEnum::Recording);

    // Attempt to start again -- should fail
    let result = pipeline_start_recording(&state, &events, Box::new(|_| {}));
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Cannot start recording"));
}

#[tokio::test]
async fn test_stop_while_idle_is_noop() {
    let state = test_state(
        MockAudioCapture::sine_wave(),
        MockAsrEngine::with_text("hello"),
        None,
        Box::new(MockPasteBackend::new()),
    );
    let events = CollectingEvents::new();

    // State is Idle, stopping should return an error
    assert_eq!(state.get_state(), AppStateEnum::Idle);
    let result = pipeline_stop_recording(&state, &events).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Cannot stop recording"));
    assert_eq!(state.get_state(), AppStateEnum::Idle);
}
```

#### Test 6: Job ID staleness

The pipeline checks job staleness at two points: after ASR transcription (manager.rs:492) and after LLM cleanup (manager.rs:577). To test the first check, we need the job ID to change *during* ASR transcription. Since the pipeline is now synchronous (no `tokio::spawn` wrapper), we use a mock ASR engine that calls `state.new_job()` as a side effect during `transcribe_file()`, simulating another recording starting while transcription is in progress.

```rust
/// Mock ASR that bumps the job ID as a side effect (simulates a new recording starting).
struct StaleJobAsrEngine {
    state: Arc<AppState>,
}

impl AsrEngine for StaleJobAsrEngine {
    fn init(&mut self) -> Result<(), String> { Ok(()) }
    fn is_ready(&self) -> bool { true }
    fn transcribe_file(&mut self, _path: &str) -> Result<AsrResult, String> {
        // Simulate a new recording starting: this makes the current job stale
        self.state.new_job();
        Ok(AsrResult {
            text: "stale result".to_string(),
            duration_secs: 1.0,
            processing_time_secs: 0.01,
            rtfx: 100.0,
        })
    }
    fn transcribe_samples(&mut self, _: &[f32], _: u32) -> Result<AsrResult, String> {
        self.transcribe_file("")
    }
    fn is_model_available(&self) -> bool { true }
    fn backend_name(&self) -> &'static str { "stale-job-mock" }
}

#[tokio::test]
#[serial_test::serial]
async fn test_stale_job_discarded() {
    let mock_paste = Arc::new(MockPasteBackend::new());

    let mut settings = Settings::default();
    settings.auto_paste = true;
    settings.llm_cleanup_enabled = false;

    // Build state first, then create the ASR mock that references it via Arc
    let (tx, rx) = std::sync::mpsc::channel();
    let state = Arc::new(AppState::new_with_backends(
        Box::new(MockAudioCapture::sine_wave()),
        Box::new(MockAsrEngine::with_text("placeholder")),  // temporary
        None,
        Box::new(SharedMockPaste(mock_paste.clone())),
        settings,
    ));

    // Replace ASR engine with the side-effect mock
    {
        let mut engine = state.asr_engine.lock().await;
        *engine = Box::new(StaleJobAsrEngine { state: state.clone() });
    }

    let events = CollectingEvents::new();

    pipeline_start_recording(&*state, &events, Box::new(|_| {})).unwrap();
    pipeline_stop_recording(&*state, &events).await.unwrap();

    // Assert no paste occurred (stale job discarded after ASR returned)
    assert!(mock_paste.last_pasted().is_none());
    assert_eq!(state.get_state(), AppStateEnum::Idle);
}
```

#### Test 7: LLM cleanup failure falls back to raw text

```rust
#[tokio::test]
#[serial_test::serial]
async fn test_llm_failure_uses_raw_text() {
    let mock_paste = Arc::new(MockPasteBackend::new());
    let state = test_state(
        MockAudioCapture::sine_wave(),
        MockAsrEngine::with_text("hello world this is a test sentence"),
        Some(MockLlmBackend::failing("sidecar crashed")),
        Box::new(SharedMockPaste(mock_paste.clone())),
    );
    let events = CollectingEvents::new();

    pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
    pipeline_stop_recording(&state, &events).await.unwrap();

    // Assert raw text was pasted (LLM failed, graceful fallback)
    let pasted = mock_paste.last_pasted().expect("should have pasted");
    assert_eq!(pasted.text, "hello world this is a test sentence");

    // Assert transcription records the failure
    let last = state.last_transcription.lock().await;
    let t = last.as_ref().unwrap();
    assert!(!t.llm_applied);
}
```

#### Test 8: Short recording discarded

```rust
#[tokio::test]
async fn test_short_recording_discarded() {
    let mock_paste = Arc::new(MockPasteBackend::new());
    // Only 100 samples -- below the 4000-sample minimum
    let state = test_state(
        MockAudioCapture::new(vec![0.0f32; 100], 48_000),
        MockAsrEngine::with_text("should not reach ASR"),
        None,
        Box::new(SharedMockPaste(mock_paste.clone())),
    );
    let events = CollectingEvents::new();

    pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
    pipeline_stop_recording(&state, &events).await.unwrap();

    // Assert no paste, no transcription (short recording silently discarded)
    assert!(mock_paste.last_pasted().is_none());
    assert_eq!(state.get_state(), AppStateEnum::Idle);
}
```

#### Test 9: Paste failure copies to clipboard as fallback

```rust
#[tokio::test]
#[serial_test::serial]
async fn test_paste_failure_clipboard_fallback() {
    let mock_paste = Arc::new(MockPasteBackend::new());
    *mock_paste.paste_error.lock().unwrap() = Some("Accessibility denied".into());

    let state = test_state(
        MockAudioCapture::sine_wave(),
        MockAsrEngine::with_text("hello world"),
        None,
        Box::new(SharedMockPaste(mock_paste.clone())),
    );
    let events = CollectingEvents::new();

    pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
    pipeline_stop_recording(&state, &events).await.unwrap();

    // Assert paste was attempted but failed
    let paste_errors = events.paste_errors.lock().unwrap();
    assert!(!paste_errors.is_empty());

    // Assert text was copied to clipboard as fallback
    let copied = mock_paste.copied_texts.lock().unwrap();
    assert_eq!(copied.len(), 1);
    assert_eq!(copied[0], "hello world");
}
```

#### Test 10: Auto-paste disabled -- copy to clipboard only

```rust
#[tokio::test]
#[serial_test::serial]
async fn test_auto_paste_disabled() {
    let mock_paste = Arc::new(MockPasteBackend::new());

    let mut settings = Settings::default();
    settings.auto_paste = false;
    settings.llm_cleanup_enabled = false;

    let state = AppState::new_with_backends(
        Box::new(MockAudioCapture::sine_wave()),
        Box::new(MockAsrEngine::with_text("hello world")),
        None,
        Box::new(SharedMockPaste(mock_paste.clone())),
        settings,
    );
    let events = CollectingEvents::new();

    pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
    pipeline_stop_recording(&state, &events).await.unwrap();

    // Assert no paste (paste_text was never called)
    assert!(mock_paste.pasted_texts.lock().unwrap().is_empty());

    // Assert text was copied to clipboard instead
    let copied = mock_paste.copied_texts.lock().unwrap();
    assert_eq!(copied[0], "hello world");
}
```

### 7.2 Regression Tests for Existing Code

After the trait refactoring, run the existing codebase to verify no regressions:

```bash
cargo build 2>&1 | tee /tmp/verify-build.txt
cargo clippy -- -D warnings 2>&1 | tee /tmp/verify-clippy.txt
cargo test 2>&1 | tee /tmp/verify-test.txt
```

### 7.3 Manual Verification

After the refactoring, manually test the full recording flow:
1. Press hotkey, speak, verify transcription appears at cursor.
2. Enable LLM cleanup, record, verify cleaned text.
3. Cancel mid-recording, verify no paste.
4. Verify overlay shows/hides correctly (not affected by pipeline extraction).

---

## 8. Security Considerations

### 8.1 No New Attack Surface

The trait abstractions do not change the security model:
- Mock implementations are gated by `#[cfg(test)]` and are never compiled into release builds.
- The `PasteBackend` trait does not bypass accessibility permissions; the production impl still calls `AXIsProcessTrusted()`.
- No new network calls, no new file access patterns.

### 8.2 Test Isolation

Tests create isolated `AppState` instances with no access to the real clipboard, microphone, or filesystem (beyond WAV temp files that are cleaned up). There is no risk of tests interfering with the user's system state.

### 8.3 cfg(test) Boundary

All mock types are in `src-tauri/src/test_support.rs`, which is declared as `#[cfg(test)] mod test_support;` in `lib.rs`. The compiler guarantees these types are not included in release builds.

---

## 9. Cost Analysis

### 9.1 Runtime Performance

| Concern | Impact |
|---------|--------|
| Trait object dispatch (vtable) | Negligible. Each call adds ~1ns. Pipeline operations take ms-seconds. |
| `Box<dyn Trait>` heap allocation | One allocation per backend at startup. Never reallocated. |
| Additional `AppState` field (paste_backend) | 16 bytes (fat pointer). |
| `sample_rate()` method on AudioCaptureBackend | Replaces an inline cpal query. Equivalent cost. |

**Total runtime impact: undetectable.**

### 9.2 Compile-Time Impact

| Concern | Impact |
|---------|--------|
| New trait definitions (4 files) | Trivial -- traits are zero-cost at compile time. |
| Mock implementations (test_support.rs) | Only compiled with `cargo test`. No impact on release builds. |
| `serial_test` dev dependency | Adds ~2s to first `cargo test` compilation. |
| Pipeline extraction (pipeline.rs) | Moves code, does not add code. Net compile time unchanged. |

### 9.3 Code Complexity

| Concern | Impact |
|---------|--------|
| Lines of code added | ~400 (traits + mocks + tests). ~200 moved from manager.rs to pipeline.rs. |
| Lines of code removed | ~0 (hotkeys/manager.rs still exists, but delegates to pipeline.rs). |
| Abstraction overhead | 3 new traits, 1 existing trait, 1 events trait. All are small (2-5 methods). |
| Cognitive load | Developers must understand that `AppState` holds trait objects. This is standard Rust. |

### 9.4 Dependencies Added

| Dependency | Version | Size | Purpose |
|------------|---------|------|---------|
| `serial_test` | latest | ~50KB | `#[serial]` attribute for tests that share global state |

No new production dependencies.

---

## 10. Implementation Tasks

Ordered by dependency. Each task is independently committable and buildable.

### Task 1: Define `AudioCaptureBackend` trait

- [ ] Add `AudioCaptureBackend` trait to `src-tauri/src/audio/capture.rs`.
- [ ] Add `sample_rate(&self) -> u32` to the trait.
- [ ] Move `AudioCapture` method bodies from inherent impl to `impl AudioCaptureBackend for AudioCapture`.
- [ ] Add a `sample_rate` field to `AudioCapture` (set during `start()`, defaults to 48000).
- [ ] Verify: `cargo build` passes.

### Task 2: Define `LlmBackend` trait

- [ ] Add `LlmBackend` trait to `src-tauri/src/llm/engine.rs` with `cleanup()`, `request_raw()`, and `shutdown()` methods.
- [ ] Implement `LlmBackend` for `LlmEngine` (move `cleanup` body, add `shutdown` delegating to `quit`).
- [ ] Verify: `cargo build` passes.

### Task 3: Define `PasteBackend` trait

- [ ] Create `src-tauri/src/paste/backend.rs` with `PasteBackend` trait, `MacOsPasteBackend` (`#[cfg(target_os = "macos")]`), and `StubPasteBackend` (`#[cfg(not(target_os = "macos"))]`).
- [ ] Update `src-tauri/src/paste/mod.rs` to declare and re-export the module.
- [ ] Verify: `cargo build` passes.

### Task 4: Update `AppState` to use trait objects

- [ ] Change `audio_capture` type to `StdMutex<Box<dyn AudioCaptureBackend>>`.
- [ ] Change `llm_engine` type to `TokioMutex<Option<Box<dyn LlmBackend>>>`.
- [ ] Add `paste_backend: Box<dyn PasteBackend>` field.
- [ ] Update `AppState::new()` to construct production implementations (use `MacOsPasteBackend` on macOS, `StubPasteBackend` on other platforms).
- [ ] Add `AppState::new_with_backends()` constructor.
- [ ] Fix all compilation errors in files that access these fields (`hotkeys/manager.rs`, `commands/llm.rs`, `commands/recording.rs`, `llm/download.rs`, `lib.rs`).
- [ ] Verify: `cargo build` passes.

### Task 5: Update `hotkeys/manager.rs` call sites

- [ ] Replace `crate::paste::paste_text()` / `paste_text_and_restore()` / `copy_to_clipboard()` calls with `state.paste_backend.*`.
- [ ] Replace `crate::paste::get_frontmost_pid()` with `state.paste_backend.get_frontmost_pid()`.
- [ ] Replace inline `cpal::default_host()` sample rate query with `capture.sample_rate()`.
- [ ] Update LLM spawn-and-store to box as `Box<dyn LlmBackend>`.
- [ ] Verify: `cargo build` and `cargo clippy -- -D warnings` pass.

### Task 6: Extract pipeline functions

- [ ] Create `src-tauri/src/pipeline.rs` with `PipelineEvents` trait.
- [ ] Extract `pipeline_start_recording()` from `handle_start_recording()`.
- [ ] Extract `pipeline_stop_recording()` from `handle_stop_recording()`.
- [ ] Extract `pipeline_cancel_recording()` from `handle_cancel_recording()`.
- [ ] Add `pub mod pipeline;` to `lib.rs`.
- [ ] Update `hotkeys/manager.rs` to call pipeline functions, keeping Tauri-specific code (overlay, shortcuts, auto-stop timer) as wrapper.
- [ ] Verify: `cargo build` and `cargo clippy -- -D warnings` pass.

### Task 7: Create mock implementations

- [ ] Create `src-tauri/src/test_support.rs` with all mock structs: `MockAudioCapture`, `MockAsrEngine`, `MockLlmBackend`, `MockPasteBackend`, `SharedMockPaste`, `CollectingEvents`.
- [ ] Add `#[cfg(test)] mod test_support;` to `lib.rs`.
- [ ] Add `serial_test` to `[dev-dependencies]` in `Cargo.toml`.
- [ ] Verify: `cargo test` compiles (no tests yet, just compilation check).

### Task 8: Write integration tests

- [ ] Add `#[cfg(test)] mod tests` to `src-tauri/src/pipeline.rs`.
- [ ] Implement Test 1: Happy path with LLM.
- [ ] Implement Test 2: Pipeline without LLM.
- [ ] Implement Test 3: ASR error.
- [ ] Implement Test 4: Recording cancellation.
- [ ] Implement Test 5: State machine guards.
- [ ] Implement Test 6: Job ID staleness.
- [ ] Implement Test 7: LLM failure fallback.
- [ ] Implement Test 8: Short recording discarded.
- [ ] Implement Test 9: Paste failure clipboard fallback.
- [ ] Implement Test 10: Auto-paste disabled.
- [ ] Verify: `cargo test` -- all tests pass.

### Task 9: Final verification

- [ ] `cargo build 2>&1 | tee /tmp/verify-build.txt` -- clean build.
- [ ] `cargo clippy -- -D warnings 2>&1 | tee /tmp/verify-clippy.txt` -- no warnings.
- [ ] `cargo test 2>&1 | tee /tmp/verify-test.txt` -- all tests pass.
- [ ] Manual test: full recording pipeline works in the running app.
- [ ] Manual test: LLM cleanup works end-to-end.
- [ ] Manual test: cancel recording works.
