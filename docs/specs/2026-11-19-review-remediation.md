# Review Remediation

- **Version:** 1.1
- **Date:** 2026-11-19
- **Status:** Implemented

## Table of Contents

1. [Summary](#1-summary)
2. [Problem Statement](#2-problem-statement)
3. [Design Overview](#3-design-overview)
4. [Detailed Design](#4-detailed-design)
5. [Edge Cases](#5-edge-cases)
6. [File Changes](#6-file-changes)
7. [Testing Strategy](#7-testing-strategy)
8. [Migration Plan](#8-migration-plan)
9. [Security Considerations](#9-security-considerations)
10. [Cost Analysis](#10-cost-analysis)
11. [Implementation Tasks](#11-implementation-tasks)
12. [Implementation Status](#12-implementation-status)
13. [Review Log](#13-review-log)
14. [Deviations](#14-deviations)

## 1. Summary

This specification remediates every finding in the
[November 2026 application review](../audit/2026-11-19-application-review.md).
Finding IDs (R-, L-, P-, S-, G-) are defined there and are not repeated here.
The owner approved all recommendations on 2026-11-19 and made two decisions:
provision a pinned standalone Python when none is installed (G-06), and leave
`training/` tracked.

## 2. Problem Statement

The review found 50 items: user-visible failures (lost hotkeys, undetected
microphone loss, a quit dead end), avoidable paste latency, gaps against the
privacy-first promise, supply-chain exposure in the cleanup runtime and release
job, a test suite that covers a copy of the stop path, and five product gaps.
Dictation users, anyone enabling cleanup, and maintainers are affected.

## 3. Design Overview

Work is cut into slices with one owner per file (section 6). Shared contracts
(section 4.1) land first so slices can proceed independently. Slices do not
build; the integrator builds once after all slices finish. The spike is the
exception: it builds and runs in a scratch directory outside the repository.

The stop path becomes one implementation. Delivery moves ahead of persistence:

```mermaid
flowchart LR
  A[claim end] --> B[finish capture]
  B --> C[ASR with deadline]
  C --> D[dictionary]
  D --> E[cleanup with budget]
  E --> F[paste or copy]
  F --> G[save history]
  G --> H[delete WAV if saved]
  H --> I[Idle]
```

Defaults preserve existing behaviour and data: sound cues are off, history and
audio retention default to "keep until I delete", and the system default
microphone stays selected.

## 4. Detailed Design

### 4.1 Shared contracts (integrator, lands before slices)

**Settings** (`src-tauri/src/models.rs`, `src/lib/utils/tauri.ts`,
`src/lib/stores/settings.svelte.ts`):

| Field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `input_device` | `Option<String>` | `None` | cpal device name; `None` = system default |
| `sound_cues` | `bool` | `false` | Play a system sound on start, stop, cancel, busy, error |
| `history_retention_days` | `u32` | `0` | Delete history older than N days; `0` = keep until deleted |
| `audio_retention_days` | `u32` | `0` | Delete retained recordings older than N days; `0` = keep until deleted |

Removed from the model: `Settings.model_path`, `Settings.language`,
`Settings.max_history` (`LlmStatus.model_path` stays). `Settings` and
`Transcription` derive `PartialEq`. `Settings::normalize` also trims dictionary
fields and drops rows whose two fields are both empty. `Settings::validate`
also bounds both retention values to 3,650 and rejects control characters in
`input_device`. `LlmStatus.update_available` is removed. `ModelStatus` gains
`integrity_warning: Option<String>`.

**AppState** (`src-tauri/src/state.rs`):

- `show_overlay: AtomicBool`, `sound_cues: AtomicBool`,
  `input_device: StdMutex<Option<String>>`: mirrors for synchronous hot paths.
- `apply_runtime_preferences(&self, &Settings)`: sets the three mirrors. Called
  by both constructors and by `update_settings` after a successful save.
- `force_exit: AtomicBool`: set by "Quit Anyway"; `begin_exit` then succeeds.
- `asr_stuck: Arc<AtomicBool>` (added by PIPE): set when a transcription
  exceeds its deadline. The blocking closure clears it when the call finally
  returns. While it is set, transcriptions fail at once with a message to
  quit and reopen SottoASR.
- `settings_load_error: StdMutex<Option<String>>` with accessor
  `settings_load_error(&self) -> Option<String>`.
- `llm_prewarming: AtomicU32` (changed by PIPE from `AtomicBool`): the number
  of prewarms in flight.

`Transcription` gains `saved_at: Option<DateTime<Utc>>`, set by the history
store on first save.

**Cross-slice Rust APIs** (exact signatures; the named slice implements):

| API | Implemented by | Called by |
| --- | --- | --- |
| `crate::audio::cue::play(app: &tauri::AppHandle, cue: crate::audio::cue::Cue)`; `enum Cue { Start, Stop, Cancel, Busy, Error }` | CAP | PIPE |
| `crate::commands::permissions::microphone_authorization() -> MicAuthorization`; `pub(crate) enum MicAuthorization { Authorized, Denied, Restricted, NotDetermined }` | SET | CAP |
| `crate::commands::recovery::enforce_retention(app: &tauri::AppHandle)` (async, infallible, logs) | HIST | SET, integrator |
| `AudioCaptureBackend::set_input_device(&mut self, name: Option<String>)` and `fn is_live_device(&self) -> bool`, both with default bodies (no-op, `false`) | CAP | CAP |
| `crate::commands::keycapture::reset_key_capture(app: &tauri::AppHandle)` | SET | integrator |
| `crate::llm::engine::VenvVerdict` (renamed from `VenkVerdict`); `pub const VENV_UNKNOWN_RETRY_INTERVAL` | LLMRT | integrator |

Unchanged and relied on: `start_recording_capture_with_errors`,
`finish_recording_capture`, `FinishedCapture`, `active_recording_path`,
`write_text`, `ClipboardAction`, `run_cleanup`, `add_transcription`,
`emit_transcription`, `finish_after_history`, `with_engine`,
`LlmBackend::cleanup_batch` and `request_raw`, `ensure_running`,
`register_shortcuts`, `ptt_virtual_key`, `setup_hotkeys(&AppHandle) ->
Result<(), String>` (the error names the shortcuts that were skipped),
`publish_state`, `open_or_focus_window`, `checked_audio_path`,
`recordings_dir`, `recording_id`, and the two permission checks as
`pub async fn() -> Result<bool, String>`.

**IPC commands**:

| Command | Change | Owner |
| --- | --- | --- |
| `list_input_devices() -> Vec<{ name, is_default }>` | New | integrator |
| `delete_recovery_recording(id: String)` | New | HIST |
| `delete_transcription`, `clear_transcriptions` | Also remove retained audio and receipts | HIST |
| `reveal_csv_export(path: String)` | New; reveals an export in Finder | HIST |
| `reset_settings() -> Settings` | New; only when settings failed to load | SET |
| `get_settings_load_error() -> Option<String>` | New | SET |
| `complete_setup` | `SetupResult` gains `error: Option<String>` | ASR |
| `redownload_speech_model()` | New; user-confirmed replacement of files that fail the manifest | ASR |
| `get_model_status` | `ModelStatus` gains `integrity_warning` | ASR |
| `get_update_status` | `UpdateStatus` loses `model_update_available` | SET |
| `check_llm_update`, `update_llm_model`, `download_llm_model`, `cancel_llm_download`, `load_llm_model`, `unload_llm_model` | Deleted; no caller | LLMRT |
| `export_transcriptions_csv`, `get_last_transcription` | Deleted; no caller | HIST |
| `needs_onboarding` | Deleted; no caller. `get_asr_backend` stays: onboarding calls it | ASR |
| `check_microphone_permission`, `check_accessibility_permission` | No longer commands; kept as functions for `complete_setup` | SET |

For each deleted command the owner removes the Rust function, FE2 removes the
wrapper, and the integrator removes the `generate_handler!` entry. FE2 deletes
exactly these wrappers: `getLastTranscription`, `exportTranscriptionsCsv`,
`checkMicrophonePermission`, `checkAccessibilityPermission`,
`needsOnboarding`, `checkLlmUpdate`, `downloadLlmModel`, `updateLlmModel`,
`cancelLlmDownload`, `loadLlmModel`, `unloadLlmModel`. FE2 also removes
`LlmStatus.update_available` and `UpdateStatus.model_update_available` from
`tauri.ts` and its test; FE1 removes them from its components and fixtures.
Components call wrappers, not `invoke`; wrappers for the commands that had
none (`restartApp`, `startKeyCapture`, `stopKeyCapture`, `overlayStartDrag`,
`dismissOverlayError`, `revealRecordingAudio`, `openTranscriptionHistory`,
`revealCsvExport`) are already in `tauri.ts`.

More cross-slice Rust APIs:

| API | Implemented by | Called by |
| --- | --- | --- |
| `crate::pipeline::transcribe_with_deadline(state: &AppState, path: String, vocabulary: Vec<String>, duration_ms: u64) -> Result<AsrResult, String>` | PIPE | PIPE, HIST |
| `crate::llm::cleanup::begin_prewarm(state: &AppState)`: counts a prewarm synchronously; `prewarm_sidecar` releases it | LLMPLAN | PIPE |
| `crate::paste::wait_for_pending_restore(timeout: std::time::Duration)` | PASTE | integrator |
| `#[cfg(test)] crate::commands::transcription::fail_next_save(id: &str)`: the next save of that ID fails once | HIST | PIPE tests |

Capability layout: `capabilities/default.json` stays the file that lists every
window, with core, event and log permissions only. Per-window files grant the
rest (`history.json`: clipboard write; `settings-close.json`: unchanged).

**Events**:

| Event | Payload | Emitter | Listener |
| --- | --- | --- | --- |
| `hotkey-busy` | `{ state }` | PIPE | overlay (FE2) |
| `model-download-progress` | `{ downloaded_bytes, total_bytes }` | ASR | onboarding (FE1) |
| `llm-download-progress` | `{ downloaded_bytes, total_bytes }` | LLMRT | settings (FE1) |
| `update-installed` | `{ version }` | SET | update view (FE1) |

Deleted events and their emitters and listeners: `llm-update-available`,
`llm-download-complete`, `llm-download-error` (`llm/download.rs:47,56`;
`update-view.svelte:74,80`).

**Theme tokens** (`src/app.css`): existing names stay. Added: `--on-accent`,
`--danger`, `--danger-hover`, `--danger-bg`, `--success`, `--success-bg`,
`--warning`, `--warning-bg`, `--scrollbar`, `--scrollbar-hover`,
`--selection-bg`, `--backdrop`, `--diff-added-bg`, `--diff-removed-bg`,
`--code-bg`. `:root` declares `color-scheme: light dark`; light values live under
`@media (prefers-color-scheme: light)`. Components use tokens only. The overlay
pill keeps its own dark palette because it floats over arbitrary content.

**Bundled lock** (`src-tauri/sidecar/requirements.lock`, generated from
`requirements.in`): the 34 package versions of the runtime that ships today
(35 lock entries; `colorama` applies only to Windows and is skipped by checks),
with SHA-256 hashes. Verified 2026-11-19: `pip install --dry-run
--require-hashes --only-binary :all: --no-deps` succeeds on CPython 3.11, 3.12,
3.13 and 3.14 for macOS arm64. Regenerate with uv 0.6.1:
`uv pip compile requirements.in --generate-hashes --universal --python-version 3.11 --no-header --no-annotate -o requirements.lock`.

### 4.2 PIPE: one stop path (S-01, L-02, R-06, R-09, R-14, R-18, R-21, G-02, G-03)

`pipeline.rs` holds the only implementation of start, stop and cancel.
`hotkeys/manager.rs` keeps shortcut registration, the push-to-talk poller, the
overlay panel code, and a `TauriEvents` adapter that implements
`PipelineEvents`.

- **Trait.** `PipelineEvents` carries structured calls, not event names: phase
  change, recording started/stopped/cancelled, `failure(Failure)`, cleanup
  status, transcription saved (with the save result), history error, paste
  complete, recordings changed, and `finished(generation, outcome)`. `Failure`
  has `kind` (`Recording`, `Transcription`, `Paste`, `HistorySave`),
  `generation`, `error`, `audio_path`, `clipboard_available`. The adapter maps
  kinds to today's event names (`recording-error`, `transcription-error`,
  `paste-error`, `history-save-error`) and payload shapes, publishes overlay
  snapshots, applies the badge dwell (800 ms applied, 2,000 ms failure), and
  hides the overlay.
- **Ownership.** Stop is two functions, because a spawned task needs `'static`
  and gets its state from the app handle: `claim_and_finish(&AppState, &dyn
  PipelineEvents, generation) -> Option<StopJob>` claims with
  `claim_recording_end(generation)` and finishes capture; `complete(&AppState,
  &dyn PipelineEvents, StopJob)` runs ASR onward. The adapter spawns
  `complete`; tests await it. IPC `stop_recording` and `cancel_recording`
  return after `claim_and_finish`, as today. Staleness checks compare both the
  job ID and the recording generation.
- **Order (L-02).** Paste or copy first, then `add_transcription`, then
  `finish_after_history`, then the saved event. `last_transcription` is set
  before delivery. A history failure after a successful paste shows
  `history-save-error` with the audio path.
- **ASR readiness and deadline (R-06).** Before `with_engine`, `complete`
  fails at once with "Speech model is still loading; your recording was kept."
  when `asr_initializing` is set or `is_model_loaded` is false, and with a
  restart message when `asr_stuck` is set. `with_engine` is wrapped in a
  timeout of `60 s + duration / 2`. A tokio timeout cannot stop the blocking
  call, which keeps the engine lock; on expiry `asr_stuck` is set and the
  failure is `Failure { kind: Transcription }` with the retained audio path.
  State returns to Idle in every case. This lives in
  `pipeline::transcribe_with_deadline`, which recovery's reprocess also calls.
  `asr_stuck` is cleared when the timed-out call finally returns.
- **Quit Anyway (R-06).** `show_busy_exit_warning` gains a second button. It
  sets `force_exit`. If the state is Recording it calls
  `claim_recording_end(None)` first, so a simultaneous key release or auto-stop
  cannot also take the writer, then finishes capture so the WAV is finalized.
  Then `app.exit(0)`. `begin_exit` returns `Ok` when `force_exit` is set.
  `RecoverySession::clean_exit` still runs: the next launch lists every WAV
  that has no history entry and no receipt, whatever the session marker says.
- **Own-PID guard (R-14).** If the resolved paste target is SottoASR's own PID,
  copy instead of paste and report `Failure { kind: Paste, clipboard_available:
  Some(true) }` with "SottoASR was the active app when dictation started; the
  text was copied instead." The overlay panel gets the non-activating style
  mask so clicking it does not activate the app.
- **Preferences (R-09).** `show_overlay` reads `state.show_overlay`.
- **Busy feedback (G-03).** A start request while Transcribing, CleaningUp or
  Pasting emits `hotkey-busy` and plays `Cue::Busy`.
- **Cues (G-02).** The adapter plays `Start`, `Stop`, `Cancel`, `Error`.
- **Startup (R-01, R-21).** `setup_hotkeys` registers each shortcut from the
  in-memory settings individually. It does not call `Settings::validate`. A
  shortcut that fails to parse or register is skipped and reported; the others
  stay registered. With unreadable settings, the default shortcuts register.
- **State (R-18, S-04).** `set_state` and `get_state` recover a poisoned lock
  with `into_inner`. The two constructors share one private builder.
  `llm_prewarming` becomes an `AtomicU32`; the start path calls
  `llm::cleanup::begin_prewarm` instead of storing a flag.
- **Paths (P-09).** The `open` call in `commands/overlay.rs` uses
  `/usr/bin/open`.
- **Mocks (R-05).** `MockAudioCapture::silence()` returns deterministic
  low-level non-zero noise, because all-zero capture is now an error.

### 4.3 CAP: capture (R-02, R-05, R-19, G-01, G-02)

- **Stall watchdog (R-02).** `RecordingWriter::start` takes `watchdog: bool`,
  set from `capture.is_live_device()`. The writer calls `on_error` once when no
  chunk arrives for 5 s before the first chunk or 2 s after it, while not
  stopping. The cpal backend returns `true`; mocks return `false` so slow test
  runs cannot trip it. The existing error path marks the transcript
  interrupted, keeps the WAV, and pastes nothing. The watchdog uses a
  monotonic clock that does not advance during system sleep, so it fires
  after a wake only if the device does not resume.
- **Authorization (R-05).** `AudioCapture::start` returns an error naming
  Microphone privacy settings when `microphone_authorization()` is `Denied` or
  `Restricted`.
- **Digital silence (R-05).** The writer tracks whether any sample was
  non-zero. `finish_recording_capture` deletes the WAV and returns an error
  when at least 4,000 samples were captured, all were exactly zero, and there
  is no capture error.
- **Queue and sync (R-19).** The channel bound is derived from time (at least
  10 s at the smallest expected callback size). Per-second checkpoints call
  `libc::fsync`; the final write keeps `sync_all`.
- **Device selection (G-01, R-02).** `start_recording_capture_with_errors`
  passes `state.input_device` to `set_input_device`. The cpal backend always
  opens a device from `input_devices()`: the named one, or the entry whose name
  equals the default device's. Such devices get cpal's disconnect listener
  (`is_default` is false); the watchdog stays as a backup. When more than one
  device carries the default's name, the backend falls back to
  `default_input_device()` so it cannot open the wrong one. A selected device
  that is absent is an error; there is no silent fallback to another
  microphone.
- **Cues (G-02).** `audio/cue.rs` plays an `NSSound` system sound through
  `tauri_nspanel::objc2_app_kit` when `state.sound_cues` is set. It never
  blocks the caller.

### 4.4 ASR: engine and bridge (R-06, R-15, P-08, L-03, G-04, S-03)

- **First step.** Resolve the commit of each HuggingFace repository whose
  files match the local cache (compare the API's LFS SHA-256 values), and
  build the manifest from those commits.
- **Download outside the lock (R-06).** Model download and verification run
  before `with_engine`; the engine lock covers load and compile only.
- **Setup error (R-15).** `complete_setup` returns the init error text.
  `get_asr_backend` and `needs_onboarding` are deleted.
- **Pinned, verified models (P-08).** `main` is hard-coded in two SDK files:
  `ModelRegistry.swift:95` (`resolve/main`) and `HFTreeLister.swift:56`
  (`tree/main`). Both are patched to a revision map keyed by repository, with
  one `apply_pinned_patch` call per file; the build also asserts the patched
  hash after applying. Files the bridge loads are checked against a bundled
  SHA-256 manifest before `MLModel(contentsOf:)`, and a verified marker avoids
  rehashing on every launch. Policy: a file is fresh if it was absent when
  preparation started. Fresh and mismatched is an error; the files are kept
  and nothing loads. Present and mismatched loads and sets
  `ModelStatus.integrity_warning`, because blocking adds no protection for
  files already in use. `redownload_speech_model` requires Idle. It moves the
  failing files aside, downloads at the pinned revision, verifies, and only
  then deletes the moved copies; on failure it moves them back. It then
  reloads the engine, emits the existing `asr-init-*` events, and returns.
- **Progress (G-04).** While preparation runs, `init_asr` emits
  `model-download-progress` every 500 ms from the cache directory size against
  the manifest total.
- **Single decode (L-03).** With vocabulary on and a recording of 30 s or less
  (480,000 samples at 16 kHz), the audio is decoded and resampled once and the
  samples go to both consumers. Longer recordings keep the file path: above
  that size the SDK streams from disk through a different converter, so the
  two paths are not numerically identical.
- **Parakeet (S-03).** `transcribe_file` resamples to 16 kHz, trimming
  rubato's `output_delay()` and flushing the tail. The fabricated duration is
  replaced with the real one. `rubato` and `audio/resample.rs` compile only
  with `asr-parakeet`.

### 4.5 PASTE: delivery (L-01, P-01, P-09)

- **Activation (L-01).** `activate_pid` is skipped when the target is already
  frontmost. Otherwise it uses `activateWithOptions: 2` (no "all windows").
- **Marking (P-01).** The transient write that precedes a restoring paste goes
  through `NSPasteboard` directly and adds `org.nspasteboard.TransientType`,
  `org.nspasteboard.ConcealedType` and `org.nspasteboard.AutoGeneratedType`.
  arboard's `exclude_from_history()` adds only the concealed marker. The write
  uses `prepareForNewContentsWithOptions:` with
  `NSPasteboardContentsCurrentHostOnly`. A copy the user keeps (restore off,
  or clipboard-only mode) is written normally.
- **Full restore (P-01).** Before the write, snapshot every pasteboard item as
  `(type, bytes)` pairs, reading one type at a time and abandoning the
  snapshot once the running total passes 64 MB. `NSPasteboardItem` cannot
  report a size, so one oversized representation is still read once. Restore
  writes all items back with the same types, only if the change count still
  equals ours. With no snapshot, the transcript stays on the clipboard.
  Functions take the pasteboard as a parameter so tests use
  `pasteboardWithUniqueName`.
- **Pending restore.** `paste::wait_for_pending_restore(timeout)` blocks until
  a scheduled restore has run or the timeout passes.
- **Paths (P-09).** `osascript` is invoked as `/usr/bin/osascript`.

### 4.6 SET: settings, tray, updater, key capture (R-01, R-03, R-10, R-17, R-21, P-09, S-02)

- **Validation (R-01).** `update_settings` calls `normalize()` then
  `validate()`. After a successful save it calls `apply_runtime_preferences`
  and, when a retention value changed, `enforce_retention`.
- **Rollback-safe file.** `persist_settings` adds `model_path: ""`,
  `language: "auto"` and `max_history: 500` to the written JSON. Builds before
  this one require those keys; without them a rollback fails to parse the file
  and registers no hotkeys.
- **Reset (R-21).** `reset_settings` renames the unreadable file to
  `settings.json.unreadable-<UTC timestamp>`, writes defaults, clears the load
  error, and re-registers shortcuts. It takes `settings_update` and the
  shortcut-update guard, so it refuses unless the app is Idle. It also refuses
  when settings loaded normally.
- **Tray (R-03).** `on_menu_event` is registered once in `setup_tray_menu`.
  Refresh only rebuilds the menu. Model-update state and menu items are removed.
- **Updater (R-17, S-02).** The model-update check, including the
  `check_model_update` call at `updater/mod.rs:242`, is deleted. `update-installed`
  is emitted when an install completes.
- **Key capture (R-10).** The tap callback re-enables the tap on
  `kCGEventTapDisabledByTimeout` and `kCGEventTapDisabledByUserInput`. The tap
  is enabled only while capturing. Capture starts only when the app is Idle
  and sets `shortcuts_updating` until it stops or resets, which also blocks a
  recording from starting. Global shortcuts are unregistered while capturing
  and re-registered afterwards under `settings_update`, as `apply_shortcuts`
  does.
- **Hardening (P-09).** `open`, `tccutil` use absolute paths. `open_url`
  accepts only `https` URLs whose host is `huggingface.co`, `github.com` or
  `sottoasr.app`. Diagnostics replace the home directory with `~`.

### 4.7 HIST: retention (P-02, P-03)

- `delete_transcription` and `clear_transcriptions` remove
  `recordings/sotto_<id>.wav` and its receipt after the durable history save.
- `delete_recovery_recording(id)` validates the ID and removes the WAV and
  receipt. Whenever a legacy copy of a deleted recording still exists in the
  system temporary directory, deletion leaves a tombstone receipt instead of
  removing the receipt, so the scan does not import it again. This applies to
  history deletes too.
- `enforce_retention` removes history entries whose age exceeds
  `history_retention_days`, measured from `saved_at` (or `created_at` on
  entries saved by older builds). It removes an entry's audio and receipt only
  when `capture_error` is `None`; an interrupted entry's audio is still the
  user's recovery copy and follows `audio_retention_days`. It removes
  recordings older than `audio_retention_days` by modification time. It runs
  at startup, every 6 h, and when a retention setting changes.
- Deletion and retention never touch the active recording, and act only when
  `RecoveryClaim::acquire` succeeds, so they cannot remove a file that a
  reprocess is reading. A user delete returns the claim error; retention
  skips silently until its next pass. They emit the existing change events so
  open windows refresh.
- The store sets `saved_at` on first save. Entries trimmed by the unchanged
  5,000-entry cap lose their audio and receipt when `capture_error` is `None`.
- `reprocess` uses `pipeline::transcribe_with_deadline`.
- `reveal_csv_export(path)` requires the parent to be the Downloads folder and
  the name to match the export pattern, then runs `/usr/bin/open -R`.
- Legacy files in the system temporary directory are not deleted.

### 4.8 LLMRT: cleanup runtime (P-04, P-09, R-07, R-12, R-16, G-04, G-06, S-02, S-04, S-06)

- **Hash-locked install (P-04).** `pip --isolated install --require-hashes
  --only-binary :all: --no-deps --disable-pip-version-check -r
  requirements.lock`. The lock is located beside the sidecar script and also
  embedded with `include_str!` for the probe.
- **Probe covers the lock (P-04).** The probe compares the 34 locked versions
  that have no environment marker.
  A runtime whose four top-level pins match but whose other packages differ
  (every 0.11.1 install) is Ready and marked stale: Ensure reinstalls from the
  lock, and if that fails, for example offline, cleanup keeps running on the
  existing runtime and the reinstall is retried at the next Ensure.
- **Repair rebuilds (R-12).** Repair first unloads the resident sidecar, then
  moves the venv to `llm-venv.aside-<UTC>`, creates a new one, installs from
  the lock, and deletes the aside copy on success. On failure it removes the
  partial venv and moves the aside copy back; if that also fails it keeps the
  aside copy and reports its path. Leftover aside directories are swept at
  the next successful Repair. Ensure still only creates a missing venv or
  syncs a stale one.
- **Interpreters.** Only CPython 3.11 through 3.14 are accepted, the range
  the lock was verified on.
- **Standalone Python (G-06).** Discovery checks fixed directories only
  (Homebrew, `/usr/local`, MacPorts, the python.org framework, `~/.local/bin`),
  then the provisioned interpreter. When none exists, provision
  `cpython-3.13.18+20261110-aarch64-apple-darwin-install_only_stripped.tar.gz`
  from the python-build-standalone release `20261110`, SHA-256
  `7a2384641584ab9984221fcb0fc9265df465922af71be71410529e2a9c3f0540`
  (25,479,437 bytes), with `/usr/bin/curl`, `/usr/bin/shasum` and `/usr/bin/tar`.
  The archive's top-level directory is `python/`, so it is extracted into a
  staging directory and renamed to
  `Application Support/com.sottoasr.app/python/cpython-3.13.18/`. A hash
  mismatch deletes the download and fails. `PATH` lookups are removed.
- **Environment (P-09).** `LlmEngine::spawn` takes a role. Both roles start
  the sidecar with `-I` and a cleared environment plus `HOME`, `TMPDIR`,
  `PATH=/usr/bin:/bin`, and an explicit `HF_HUB_CACHE` equal to today's
  location. The inference role also sets `HF_HUB_OFFLINE=1`; the download
  role, already a separate process (`llm/download.rs:33`), does not.
- **Download (R-07, G-04).** No fixed total deadline. Rust polls the model
  cache size every 2 s, emits `llm-download-progress`, and fails after 120 s
  without growth.
- **Races (R-16).** `claim_preparation` records the mode; a Repair requested
  during Ensure runs after it. `llm_preparing` is cleared by an RAII guard.
- **Deletions (S-02).** The sidecar `check_update` action, its Rust callers,
  `check_llm_update`, `update_llm_model`, `LlmEngine::status`, and the
  `llm-download-complete` and `llm-download-error` emits in `llm/download.rs`.
- **Rename (S-04).** `VenkVerdict` becomes `VenvVerdict`.
- **Sync test (S-06).** A Rust test reads the sidecar script and the lock and
  asserts the four runtime pins, model ID, revision and prompt hash agree.

### 4.9 LLMPLAN: planner and validator (R-04, R-11, R-13, R-16, L-04, S-02)

- **Budget (R-04).** Dispatch stops after the first transport-level failure
  (`Unavailable`, `TimedOut`, or a retired sidecar) and when 60 s have elapsed.
  Per-item declines do not stop it. Undispatched regions stay raw.
- **Growth cap (R-11).** `apply_edit_lines` returns `None` once the working
  text exceeds `MAX_CLEANUP_BYTES`.
- **Number words (R-13).** `SINGLE_REPEAT_PROTECTED` gains zero through twelve,
  `oh`, `hundred`, `thousand`. Existing fixture expectations do not change; a
  conflict stops the work and is reported.
- **Prewarm (R-16).** `begin_prewarm` increments `llm_prewarming` before the
  task is spawned; `prewarm_sidecar` decrements it on every exit path and
  never below zero. `correction.rs` treats any non-zero count as a prewarm in
  flight.
- **Cost (L-04).** Protection patterns compile once per cleanup. The dominant
  super-linear step is identified and fixed without changing output.
- **Deletions (S-02).** `llm/edits.rs`, `examples/cleanup_edits.rs`, and the
  unreachable inner timeout in `cleanup.rs`.

### 4.10 FE1: settings, onboarding, update (R-01, R-08, R-15, R-17, R-20, G-01, G-02, G-04, G-05, P-03)

- Dictionary rows are trimmed; an incomplete row blocks Save with an inline
  message. Required shortcuts have no clear button and gain "Reset to default".
- The recorder adds Control whenever `ctrlKey` is set.
- Settings gains a microphone selector, a sound-cue toggle, and two retention
  selectors (Keep until I delete, 1, 7, 30, 90, 365 days). The audio selector
  says that recordings waiting for recovery are deleted too.
- A failed settings load offers "Back up and reset" through `reset_settings`.
- The model dashboard shows `ModelStatus.integrity_warning` with a confirmed
  "Re-download speech model" action that calls `redownload_speech_model`.
- `save()` merges defaults the way `load()` does, so a no-op edit is not dirty.
- Onboarding shows the returned error and retries in place, calls
  `requestAccessibilityPermission`, calls `downloadModel` when
  `getAsrBackend()` reports the model absent, and shows a progress bar from
  `model-download-progress`.
- The update view deletes every model-update step, ignores check events unless
  it is checking, and moves to ready on `update-installed`.
- Cleanup setup shows progress from `llm-download-progress`.
- Its components use theme tokens only.

### 4.11 FE2: history, overlay, shell (P-02, R-20, G-03, G-05, G-07, S-02, S-06)

- History delete and Clear All dialogs state that retained audio is removed.
  Recovery rows gain "Delete audio".
- Relative timestamps refresh every minute. History uses `$state.raw`.
- CSV export offers "Show in Finder" through `reveal_csv_export`. Row buttons
  get per-item accessible names.
- The overlay shows a brief "Still working" state on `hotkey-busy`, and says
  "Cleaning up" during cleanup.
- `src/app.css` finishes moving to tokens; its components use tokens only.
- Entry scripts forward `console.error` and `console.warn` to
  `@tauri-apps/plugin-log`.
- Deleted: `src/*.html`, `stores/recording.svelte.ts` and its test, the unused
  wrappers, unused JS plugin packages, `@types/diff`. `benchmarks/ui/*.mjs`
  drops the removed settings fields and deleted commands and handles the new
  ones.
- The contract test parses `generate_handler!` and the capability files and
  checks every `invoke` name and wrapper against them.

### 4.12 CI: workflows (P-05, P-10, S-03)

Actions are pinned to commit SHAs resolved on 2026-11-19:

| Action | SHA |
| --- | --- |
| `actions/checkout` v5 | `fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09` |
| `actions/setup-node` v5 | `a0853c24544627f65ddf259abe73b1d18a591444` |
| `actions/upload-artifact` v4 | `ea165f8d65b6e75b540449e92b4886f43607fa02` |
| `dtolnay/rust-toolchain` 1.94.0 | `46ce8032f0e3e2d70554c4eb1961fc1ca5fde79f` |
| `Swatinem/rust-cache` v2 | `d1527c8f67ef9af357290475d4af4b0fab8f12e0` |
| `tauri-apps/tauri-action` v0 | `84b9d35b5fc46c1e45415bdb6144030364f7ebc5` |

`dtolnay/rust-toolchain` is given `toolchain: 1.94.0` explicitly, because a
commit pin no longer carries the version in its ref name.

Apple and updater secrets move from job level to the steps that sign.
Checkout sets `persist-credentials: false` where no push follows. `npm audit
--omit=dev` is blocking in both workflows. PR CI adds `cargo check
--no-default-features --features custom-protocol,asr-parakeet` and runs the
resampler tests under that feature. The audit step fails when `cargo tree -i
zip --target aarch64-apple-darwin` prints a dependency, which keeps the
ignores in `src-tauri/.cargo/audit.toml` honest. `scripts/test_workflows.py`
runs with the other Python tests.

### 4.13 Integrator (L-05, P-06, P-07, P-10, S-04, S-05, G-08)

- `lib.rs`: remove `#![deny(warnings)]`, the `store`, `os` and `positioner`
  plugins, deleted commands; register new commands; startup retention and the
  6 h timer; first cleanup retry delay at least `VENV_UNKNOWN_RETRY_INTERVAL`.
- Capabilities follow the 4.1 layout; unused permissions are removed.
- `lib.rs` drops the registrations of the commands 4.1 deletes.
- `RunEvent::Exit` calls `paste::wait_for_pending_restore(800 ms)` so a quit
  right after a paste does not lose the user's clipboard.
- Not done: a per-window allowlist of application commands. Every window is
  first-party content under the same CSP, and a wrong mapping would break a
  window at run time, which cannot be exercised here.
- Advisories (P-10): `rustls`, `rustls-webpki` and `webpki-root-certs` are
  updated. The six `zip` advisories are ignored in `src-tauri/.cargo/audit.toml`
  with the reason: `zip` 4 is a Windows-only dependency of
  `tauri-plugin-updater` and is not compiled for macOS. The fixed `zip` needs
  the updater plugin 2.14, which requires Tauri 2.12; a framework upgrade is
  not made here because tray, focus and panel behaviour cannot be exercised
  without launching the app. Development advisories are cleared by updating
  `vite` and `vitest` (`npm audit fix` crashes in npm itself).
- L-05 is closed without a change: Tauri 2.11.5 runs event JavaScript only in
  webviews that registered a listener (`event/listener.rs:281-288`), so
  `audio-level` already reaches the overlay alone.
- Documentation: README, `DEVELOPMENT.md`, `.claude/rules/project.md`,
  `opencode.json`, `docs/designs/architecture.md`, `SECURITY.md`,
  `CHANGELOG.md` under `[Unreleased]`.
- Spike (G-08): the spike owns
  `docs/research/2026-11-19-in-process-cleanup-spike.md`, which records a
  measured attempt to load the pinned checkpoint with `mlx-swift-lm`. It
  edits no other tracked file.

## 5. Edge Cases

| Case | Handling |
| --- | --- |
| Bluetooth microphone needs over 2 s to deliver its first buffer | First-chunk limit is 5 s |
| Hardware-muted microphone delivers exact zeros | Reported as silence with both causes named |
| Selected microphone unplugged before start | Start fails with the device name; no fallback |
| Quit Anyway during recording | Capture is finalized; the WAV is offered at next launch |
| Paste succeeds, history save fails | Text is delivered; overlay shows the save error and audio path |
| Clipboard holds a multi-gigabyte item | Snapshot abandoned once 64 MB is passed (one representation may be read in full first); transcript stays on the clipboard |
| User copies during the 500 ms restore window | Change count differs; restore is skipped |
| Existing speech-model cache differs from the manifest | Keeps working; Settings shows a warning and a confirmed re-download |
| Freshly downloaded model fails the manifest | Error; files are kept for inspection and nothing loads |
| Existing settings file has `model_path`, `language`, `max_history` | Ignored on load; rewritten with fixed values so older builds can still read the file |
| Key capture requested while recording or transcribing | Refused; shortcuts stay registered |
| Transcription arrives while the speech model is loading | Fails at once with the recording kept |
| Transcription exceeds its deadline | Recording kept; dictations fail at once with a restart message until that call returns |
| Existing invalid dictionary row | Hotkeys still register; Save is blocked until the row is fixed |
| Standalone Python download interrupted | Partial file removed; next attempt restarts |
| Retention set to N days with pending unrecovered recordings older than N | They are deleted; the selector text says so |
| Long healthy dictation exceeds the 60 s cleanup budget | Later windows stay raw; earlier validated edits apply |

## 6. File Changes

One owner per file. A slice that needs a change in another owner's file asks
that owner.

| Slice | Files |
| --- | --- |
| Integrator | `src-tauri/src/lib.rs`, `models.rs`, `commands/mod.rs`, `commands/audio.rs` (new), `Cargo.toml`, `Cargo.lock`, `.cargo/audit.toml` (new), `tauri.conf.json`, `capabilities/*`, `sidecar/requirements.in` (new), `sidecar/requirements.lock` (new), `package-lock.json`, all documentation |
| PIPE | `pipeline.rs`, `hotkeys/*`, `commands/recording.rs`, `commands/overlay.rs`, `state.rs`, `test_support.rs` |
| CAP | `audio/capture.rs`, `audio/recovery.rs`, `audio/wav.rs`, `audio/mod.rs`, `audio/cue.rs` (new) |
| ASR | `asr/*`, `audio/resample.rs`, `commands/setup.rs`, `commands/vocabulary.rs`, `vendor/fluidaudio-rs/**`, `examples/asr_fixture.rs`, `scripts/check-*.py` except `check-release-target.py` |
| PASTE | `paste/*` |
| SET | `commands/settings.rs`, `commands/permissions.rs`, `commands/keycapture.rs`, `tray/*`, `updater/mod.rs`, `dictionary.rs` |
| HIST | `commands/transcription.rs`, `commands/recovery.rs`, `persistence.rs` |
| LLMRT | `llm/engine.rs`, `llm/download.rs`, `commands/llm.rs`, `process.rs`, `sidecar/llm_cleanup.py`, `sidecar/test_llm_cleanup.py` |
| LLMPLAN | `llm/correction.rs`, `llm/cleanup.rs`, `llm/validation.rs`, `llm/edits.rs` (delete), `llm/mod.rs`, `examples/cleanup_edits.rs` (delete), `tests/fixtures/*` |
| FE1 | `src/lib/components/settings-*.svelte`, `settings-panel*`, `settings-panels.css`, `dictionary-settings*`, `shortcut-recorder*`, `onboarding-view*`, `update-view*`, `model-dashboard*`, `cleanup-suggestion.svelte`, `src/lib/stores/settings*`, `cleanup-setup*`, `vocabulary-setup*` |
| FE2 | Everything else under `src/`, root `*.html`, `vite.config.ts`, `vitest.config.ts`, `package.json`, `benchmarks/ui/*` |
| CI | `.github/workflows/*`, `scripts/*` not owned by ASR |

`src/lib/utils/tauri.ts` and `src/app.css` receive the 4.1 contract from the
integrator first, then belong to FE2.

## 7. Testing Strategy

New tests catch consumer-visible failures only. Each slice adds the tests named
here; none are added for wiring.

| Area | Tests |
| --- | --- |
| PIPE | Existing pipeline tests now drive production code. New: stale generation discards delivery; paste happens before the history save; history failure after paste keeps the WAV (through `fail_next_save`); own-PID target copies instead of pasting; ASR deadline (with `start_paused`) returns to Idle with audio kept and later requests fail at once; lenient shortcut plan registers valid entries beside an invalid one |
| CAP | A live backend that stops delivering triggers one interrupted capture; a mock backend never does; all-zero capture is an error and removes the WAV |
| ASR | A fresh mismatched file is an error and stays on disk; a pre-existing mismatched file loads with a warning; resampler output length and tail at 48 kHz to 16 kHz |
| PASTE | Snapshot and restore round-trip two items with multiple types on a unique pasteboard; restore is skipped after a foreign change; a snapshot over the cap is abandoned; the transient write carries the three marker types |
| SET | Save rejects a duplicate shortcut and an invalid dictionary row; the written file still parses with the three legacy keys required; reset backs up the unreadable file and refuses while not Idle; `open_url` rejects other hosts and userinfo; the tray handler goes through a pure once-guard that is tested without an app handle |
| HIST | Delete and clear remove audio and receipts; retention removes only expired entries, measures a recovered entry from its save, keeps an interrupted entry's audio, and never touches the active recording; a deleted legacy recording is not imported again |
| LLMRT | Lock and sidecar pins agree; every lock entry carries a hash and the install arguments include `--require-hashes --no-deps`; a runtime with a floated transitive version is reported stale; Repair replaces a venv whose interpreter link dangles; stalled download fails after the stall limit with an injected clock |
| CI | `scripts/test_workflows.py`: every `uses:` is a 40-character SHA and no secret is declared at job level |
| LLMPLAN | Transport failure stops dispatch; budget stops dispatch; doubled edit lines are rejected; "four four seven" keeps both words |
| FE1 | Incomplete dictionary row blocks Save; Control+Command is recorded; check events are ignored outside checking; onboarding shows the returned error |
| FE2 | Contract test against `generate_handler!`; delete dialog names audio; busy state appears and clears |

Verification after integration: `cargo build`, `cargo clippy --all-targets --
-D warnings`, `cargo test`, `cargo check` for `asr-parakeet`, `npm run check`,
`npm test`, `npm run build`, both Python test suites, `scripts/ci-checks.sh`,
`cargo audit`, `npm audit --omit=dev`, and the pinned Swift regression script.
The native app is not launched: a development build would share the running
production app's data directory and global shortcuts.

## 8. Migration Plan

- Settings files load unchanged. The three legacy keys are still written with
  fixed values, so a build older than this one can read the file after a
  rollback. Without them it would fail to parse and register no hotkeys.
- History and recordings are untouched until the user sets a retention value
  or deletes entries.
- An existing cleanup venv keeps working. The next Ensure that finds stale pins,
  and every Repair, installs from the lock.
- Existing speech-model caches are verified once and marked. A cache that
  fails keeps working with a warning.

## 9. Security Considerations

- New network call site: the python-build-standalone release asset, fetched
  only when the user enables cleanup and no Python 3.11+ exists. It is pinned
  by URL and SHA-256.
- The runtime install executes only hash-matched wheels.
- Speech and vocabulary models are revision-pinned and hash-checked.
- Retention deletes user data only after the user chooses a value or confirms
  a delete dialog that names audio.
- The transient transcript is excluded from clipboard history and written
  with `NSPasteboardContentsCurrentHostOnly`, so it does not reach Universal
  Clipboard. Restored items keep their original types, including sensitivity
  markers.
- The standalone Python is fetched with `/usr/bin/curl -q --fail -L --proto
  =https --connect-timeout 15 --speed-limit 1024 --speed-time 30` as an owned
  child process, so a quit stops it. It is extracted only after its SHA-256
  matches, which fixes the archive's contents.
- Application commands stay callable from every window, as before (4.13).

## 10. Cost Analysis

- Paste: removes a 150 ms sleep in the common case and moves the history write
  after delivery. The clipboard snapshot reads the current contents once
  before the paste; a large representation costs its read time.
- Model verification: one SHA-256 pass over about 560 MB after download.
- Standalone Python: 25 MB download, about 60 MB on disk, only when needed.
- Dependencies removed: three Tauri plugins, six JS packages; `rubato` becomes
  optional. None added.
- Bundle: one lock file of about 60 KB.

## 11. Implementation Tasks

1. Integrator: shared contracts (4.1); tree builds and tests pass.
2. In parallel: PIPE, CAP, ASR, PASTE, SET, HIST, LLMRT, LLMPLAN, FE1, FE2, CI,
   and the spike.
3. Integrator: `lib.rs`, capabilities, command pruning, lockfiles; build.
4. Route build and test failures to the owning slice; repeat until clean.
5. Independent review of the diff; fix findings.
6. Documentation; update this status section and the audit.

## 12. Implementation Status

All 50 findings are closed in the commit that adds this section. The version
stays 0.11.1 in that commit; the changes are listed under `[Unreleased]` in
`CHANGELOG.md`.

### 12.1 Status by finding

| ID | Result |
| --- | --- |
| R-01 | Save runs `normalize` then `validate`. Startup registers each shortcut on its own. The settings form blocks an incomplete dictionary row and cannot clear a required shortcut. |
| R-02 | cpal 0.16 opens the device from the device list, which carries the disconnect listener. A stall watchdog (5 s before the first chunk, 2 s after) backs it up. An interrupted capture pastes nothing and keeps the WAV. |
| R-03 | The tray handler is registered through `register_menu_handler_once`; a refresh only rebuilds the menu. |
| R-04 | Dispatch stops at the first transport failure, and once less than the engine's 15 s answer time is left of the 60 s budget. The total stays within 60 s and a healthy sidecar is not killed. |
| R-05 | Start fails with a message when microphone access is denied or restricted. An all-zero capture is reported as a recording failure. |
| R-06 | Transcription has a deadline of `60 s + duration / 2`. Model download runs outside the engine lock. The busy-exit dialog offers Quit Anyway, which finalizes the recording first. |
| R-07 | The cleanup download has no fixed deadline. It fails after 120 s without growth and reports progress. |
| R-08 | The recorder keeps Control whenever it is held. |
| R-09 | The overlay reads an atomic mirror of the setting. |
| R-10 | The tap is re-enabled after macOS disables it, is enabled only while capturing, and starts only when Idle. |
| R-11 | `apply_edit_lines` rejects text that grows past the cleanup limit. |
| R-12 | Repair builds a new runtime in a staging directory and swaps it in. A failed swap moves the old runtime back. |
| R-13 | Zero through twelve, `oh`, `hundred` and `thousand` are protected from single-repeat deletion. No fixture expectation changed. |
| R-14 | The overlay panel is non-activating and an error card does not take keyboard focus. A paste whose receiver would be SottoASR is copied instead. |
| R-15 | `complete_setup` returns the error. Onboarding shows it, retries in place, requests Accessibility, and handles the ONNX backend. |
| R-16 | Prewarms are counted. The first retry waits at least `VENV_UNKNOWN_RETRY_INTERVAL`. A Repair requested during Ensure runs after it. |
| R-17 | The model-update steps are gone. Check events are ignored outside checking. `update-installed` moves the view to ready. |
| R-18 | `set_state` and `get_state` recover a poisoned lock. |
| R-19 | The capture queue reports an overrun once 10 s of audio is waiting, counted in samples. Checkpoints use `fsync`. |
| R-20 | A no-op edit is not dirty. History timestamps refresh each minute. The overlay says "Cleaning up". |
| R-21 | Default shortcuts register when settings are unreadable. Settings offers "Back up and reset". |
| L-01 | Activation is skipped when the target is frontmost, and no longer raises all windows. |
| L-02 | The paste happens before the history save. |
| L-03 | Recordings of 30 s or less are decoded once for both consumers. Eight probe transcripts were identical before and after. |
| L-04 | Protection patterns compile once per cleanup. The literal check after a deletion is quadratic, not cubic. The Rust suite runs in about 8 s (60 s before). |
| L-05 | Closed without a change: Tauri already delivers an event only to windows that listen for it. |
| P-01 | The transient write carries the three marker types and stays on this Mac. Restore puts back every item and type. A quit waits for a pending restore. |
| P-02 | Delete and Clear All remove retained audio. Recovery rows have "Delete audio". Audio retention is a setting. |
| P-03 | History and audio retention are settings (default: keep). The three inert fields left the model; the file still carries them for older builds. |
| P-04 | The runtime installs from a hash-locked file with `--require-hashes --no-deps`. The probe covers all 34 pins. An older runtime is synced to the lock. |
| P-05 | Actions are pinned to commit SHAs. Signing secrets are step-level. `npm audit --omit=dev` blocks. |
| P-06 | Capabilities grant core, window close and log, plus clipboard write for History. Three unused plugins are removed. |
| P-07 | The architecture document and `SECURITY.md` list every network call. |
| P-08 | Speech, vocabulary and ONNX models download from pinned commits and are checked against SHA-256 manifests. |
| P-09 | The sidecar starts with `-I` and a cleared environment. System tools use absolute paths. `open_url` has a host allowlist. Diagnostics hide the home path. |
| P-10 | `cargo audit` exits 0 and `npm audit` reports 0. Six `zip` advisories are ignored with a CI guard (section 4.13). |
| S-01 | `pipeline.rs` is the only stop path, and the pipeline tests drive it. |
| S-02 | The model-update path, `llm/edits.rs`, `src/*.html`, the unused store and the unused wrappers are deleted. |
| S-03 | PR CI checks the ONNX backend and runs its resampler tests. The backend resamples to 16 kHz. |
| S-04 | `VenvVerdict`, derived `PartialEq`, one `AppState` builder, no crate-level `deny(warnings)`. |
| S-05 | README, `DEVELOPMENT.md`, the project and release rules, `opencode.json`, `AGENTS.md` and the architecture document match the code. |
| S-06 | A Rust test ties the sidecar pins to the lock. The frontend contract test reads `generate_handler!` and the capability files. |
| G-01 | Settings has a microphone selector. A selected device that is absent is an error. |
| G-02 | Sound cues for start, stop, cancel, busy and error; off by default. |
| G-03 | A press while busy shows "Still working" and plays the busy cue. |
| G-04 | Both model downloads show progress. |
| G-05 | A light palette follows the system appearance. |
| G-06 | A pinned, hash-checked standalone CPython is provisioned when no usable Python exists. |
| G-07 | Console errors and warnings reach the log file. CSV export offers "Show in Finder". |
| G-08 | Spike recorded in [the research note](../research/2026-11-19-in-process-cleanup-spike.md). Not adopted. |

### 12.2 Verification

Run on 2026-11-19 against the working tree, macOS arm64, Rust 1.94.0.

| Check | Result |
| --- | --- |
| `cargo build --locked` | Pass |
| `cargo clippy --locked --all-targets -- -D warnings` | Clean in three configurations: default, no ASR backend, `asr-parakeet` |
| `cargo test --locked` | 320 pass (default), 306 (no ASR backend), 314 (`asr-parakeet`, `--lib`); four consecutive default runs passed |
| `cargo audit` | Exit 0 |
| `npm run check`, `npm test`, `npm run build`, `npm audit` | 0 errors, 144 pass, builds, 0 vulnerabilities |
| Sidecar Python tests, in a venv installed from the lock | 35 pass |
| `scripts/` Python tests | 6 pass |
| `scripts/ci-checks.sh` | Pass |
| `check-swift-regressions.py`, `check-model-manifest.py` | Pass |
| `check-asr-cache.py`, `check-vocabulary-cache.py`, `check-asr-tail.py` | Pass (run before the last Rust-only fixes; the Swift code did not change after) |

Not run: the unsigned packaging step of `scripts/pre-release-check.sh`
(a release build did not fit in the free disk space) and both GitHub
workflows.

### 12.3 Not verified: needs a run of the app

The app was not launched (section 7). These behaviours are covered by unit
tests where a seam exists, but not by a native run:

- **Capture:** unplug and Bluetooth-drop detection; the microphone indicator
  clearing after stop; resume after sleep; sound cues leaking into a recording.
- **Paste and overlay:** focus when activation is skipped; clipboard managers
  and Universal Clipboard ignoring the transient write; restore of images and
  files; an error card that leaves typing in the user's app and acts on the
  first click; the Stop and Cancel buttons and dragging on the non-activating
  panel.
- **Lifecycle and settings:** Quit Anyway, also while a stop is finishing the
  file; the tray after refreshes; key capture; the microphone selector; the
  light palette.
- **Downloads and runtime:** a first download from the pinned commits; the
  confirmed re-download while a model is loaded; Repair, the background sync
  and standalone Python on a machine without Python.
- **History:** retention on real data.

## 13. Review Log

### Pass 1: assumption validation (2026-11-19)

Checked against the locked dependency sources. Changes made:

- Quit Anyway keeps `clean_exit` and claims the recording end first. Pending
  recordings are offered from the directory scan, not the session marker.
- Model pinning patches two SDK files, not one, and asserts the patched hash.
- A cache that fails the manifest warns instead of blocking, and gains a
  confirmed re-download; the SDK never re-downloads a file that exists.
- Clipboard snapshot reads type by type; the size cap cannot be known first.
- The transient write adds the transient and auto-generated markers, which
  arboard does not.
- Key capture runs only when Idle and holds `shortcuts_updating`.
- Stop is split into `claim_and_finish` and `complete`.
- Transcription checks readiness first; a timeout marks the engine stuck.
- Single decode is limited to recordings of 30 s or less.
- The default microphone is opened through `input_devices()` to get cpal's
  disconnect listener.
- Standalone Python is staged and renamed; the archive root is `python/`.
- The legacy settings keys are still written so a rollback keeps its hotkeys.
- L-05 closed without change: delivery is already limited to listeners.
- `benchmarks/ui` gains an owner.

### Pass 2: completeness (2026-11-19)

Changes made:

- Retention measures age from a new `saved_at`, not from `created_at`, which
  a recovered recording inherits from its WAV. History retention no longer
  deletes the audio of interrupted entries.
- Deletions stick: a tombstone receipt stops the legacy import, and entries
  trimmed by the cap lose their audio.
- Deletion and retention take the recovery claim and emit change events.
- Reprocess shares the transcription deadline.
- The runtime probe covers all 34 locked versions; an upgraded install is
  synced to the lock instead of staying on floated versions, without breaking
  cleanup offline.
- Repair unloads the sidecar first and has a defined aside directory.
- The prewarm counter has an owner on both sides of the slice boundary.
- `reveal_csv_export` backs "Show in Finder".
- A quit waits for a pending clipboard restore; `reset_settings` requires Idle.
- The transient clipboard write is local-only.
- Light-palette accent and success colours were darkened to pass 4.5:1.
- Capability layout, the command deletion list per owner, and tests for the
  tray handler, the install arguments and the workflow pins were added.
- A per-window command allowlist was considered and not adopted (4.13).

### Pass 3: clarity and actionability (2026-11-19)

Changes made:

- `get_asr_backend` stays; onboarding calls it. Only `needs_onboarding` is
  deleted from the ASR slice.
- The wrappers FE2 deletes are listed by name, and the two status fields
  removed from the frontend types have owners on both sides.
- Components call wrappers; the wrappers that were missing are in `tauri.ts`.
- `asr_stuck` is shared with the blocking call, which clears it on return.
- "Fresh" is defined for the model manifest policy, with a test for each case.
- `redownload_speech_model` requires Idle, reloads, and emits the init events.
- `fail_next_save` takes the entry ID so a parallel test cannot consume it.
- The functions other slices rely on unchanged are listed.
- The lock has 35 entries; checks cover the 34 without a platform marker.
- Deletes leave a tombstone when a legacy copy exists; a user delete reports
  a busy claim, retention skips.
- The tray test goes through a pure once-guard; the deadline test uses a
  paused clock; ASR owns the Swift probe scripts.
- The spike owns its research document and may build outside the repository.

### Independent review of the implementation (2026-11-19)

Six reviewers read the finished slices. Their findings were fixed by the
slice owners and the tree was verified again (section 12.2). Changes made:

- cpal moved to 0.16: with 0.15.3 a device opened from the device list kept
  the microphone running after the stream was dropped.
- The model integrity record gained a snapshot taken before a download, a
  verdict per file, and recovery of an interrupted re-download.
- The cleanup runtime is built in a staging directory for create, sync and
  Repair, so an interrupted setup recovers by itself.
- A paste counts as in flight until its restore is scheduled, and one lock
  serializes the module's pasteboard calls.
- The deletion marker is written before the audio is deleted.
- The capture queue bound counts samples, not channel slots.
- Error cards no longer take keyboard focus; the own-app paste guard no
  longer depends on a setting.
- Tests that passed with their fix removed were rewritten to drive the
  production code.

The reviewers then read the fixes. Changes made after that second pass:

- The start snapshot is saved before a first download creates the model
  folder; a preparation that cannot save it stops before downloading.
- Rejected model files are deleted, not moved aside, and keep their verdict
  across a pinned-commit change.
- The cleanup budget stops dispatching early instead of cutting a chunk
  short, which had killed a healthy sidecar.
- An out-of-date runtime that cannot load is rebuilt at once. The background
  sync runs as its own task, and the old runtime stays until the new one has
  loaded.
- A failed settings save restores only the saved shortcuts.
- Quit Anyway waits up to 2 s for a stop that is already writing the file.
- A failed audio delete removes the deletion marker it wrote.

## 14. Deviations

| Section | Specified | Implemented | Reason |
| --- | --- | --- | --- |
| 2 | 51 items | 50 items | Counting error in the specification |
| 4.1 | Theme token `--backdrop` | Removed; the dialog uses literal `::backdrop` colours. `--accent-text` added for accent-coloured text | WebKit before 17.4 does not pass custom properties to `::backdrop`; accent text fell below 4.5:1 on dark surfaces |
| 4.1 | Four new events | Also `history-changed { removed_ids }`; the model dashboard also listens to `model-download-progress` | Open windows must drop entries that retention removed; a re-download needs progress |
| 4.1 | `llm_preparing` flag; cancel shortcuts as separate fields | `AtomicU8` that records the preparation mode; one parsed list | A Repair during Ensure needs the mode; one list serves the poller |
| 4.2 | Own-PID guard uses the paste target resolved at start | Decided at delivery from the app that would receive the paste, whatever the focus setting is. Message: "SottoASR was the active app, so the text was copied instead of pasted." | With focus restore off, the receiver is only known at delivery |
| 4.2 | Error cards unchanged | Cards do not become the key window when shown, and the overlay accepts the first click | A non-activating key panel would take the user's typing |
| 4.2 | Quit Anyway claims the recording, then exits | When a stop already holds the recording, Quit Anyway waits up to 2 s for it to finish the file | The exit could cut off the last second |
| 4.2 | Readiness failure always says "still loading" | A finished, failed load reports its own error | The old text never ended |
| 4.3 | cpal 0.15 | cpal 0.16; `stop()` pauses the stream before the drop | See the review log |
| 4.3 | Channel bound derived from time | 60,000 slots plus a shared gauge that refuses more than 10 s of samples at the real rate | A slot count is only a time bound for one rate and callback size |
| 4.4 | A verified marker; fresh means absent at the start | A record with a verdict per file and a snapshot saved before the download. Fresh also covers files a preparation replaced or rejected earlier | An interrupted download must not count as an existing cache |
| 4.4, 5 | A rejected fresh file is kept | It is deleted and fetched again at the next preparation | Otherwise onboarding could not retry |
| 4.4 | Policy covers the speech model | The vocabulary model follows the same policy and the same re-download | Same exposure |
| 4.4 | `asr_initializing` set for the whole preparation | Set only around load or reload; a loaded model keeps transcribing while files download. A dictation that starts while the vocabulary files are being replaced runs without the vocabulary boost | Dictation was refused for the whole re-download; the SDK reads the vocabulary tokenizer from disk at each dictation |
| 4.4 | ONNX model unchanged | Pinned to a commit and checked with `sha2` (optional, `asr-parakeet` only) | P-08 applies to that backend too |
| 4.5 | Transient write is local-only | The restore write is local-only too. A second paste inside the restore window reuses the first snapshot. Restore bookkeeping lives in `paste/restore.rs` | The restored contents would otherwise reach Universal Clipboard; a second snapshot would capture the transcript |
| 4.7 | Deletes act when the claim succeeds | Also: history deletes are refused while a dictation is in flight, the deletion marker is written before the audio is removed, and audio is left alone while another instance owns the recordings | Found in review |
| 4.8 | Environment list | The download role also sets `HF_HUB_DISABLE_XET=1` | With xet the cache size stayed flat for about 62 s, which would trip the stall limit on slow links |
| 4.8 | Repair moves the venv aside, then creates in place; Ensure syncs in place | Create, sync and Repair build in `llm-venv.staging` and rename it into place after a passing probe. An out-of-date runtime is loaded first, then synced by a separate background task with `--timeout 10 --retries 1`; the old runtime is kept until the synced one has loaded. If the out-of-date runtime cannot load, it is rebuilt at once | An interrupted setup left a runtime that Ensure would not fix; the sync blocked cleanup at every launch |
| 4.8 | Interpreter check: version range | Also requires arm64; an interpreter that cannot be started counts as unusable | An Intel Python under Rosetta cannot install the arm64 wheels |
| 4.9 | Dispatch stops after 60 s | No chunk is dispatched once less than the engine's 15 s deadline is left of the budget | Checking only between chunks allowed about 90 s; cutting a chunk short would kill a healthy sidecar |
| 4.12 | `toolchain: 1.94.0` input | No input | The pinned commit is the 1.94.0 branch of the action and declares no such input |
| 4.12 | The zip guard fails when `cargo tree` prints a dependent | It also passes with a notice when `zip` is no longer in the lockfile | `cargo tree -i` fails for an absent package |
| 10 | No dependency added | `sha2` (optional, already in the lockfile). cpal 0.16 replaces `coreaudio-sys` and `bindgen` with `objc2-core-audio` | See 4.3 and 4.4 |
| 4.13 | Documentation list | Also `THIRD_PARTY_LICENSES`, `AGENTS.md`, `.claude/rules/release.md` and the release notes template | They carried the same stale facts |

Old journals still link to `llm/edits.rs` and `examples/cleanup_edits.rs`,
which are deleted. Journals are point-in-time records and were not edited.
