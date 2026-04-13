# Changelog

All notable changes to SottoASR are documented in this file.

## [0.7.1] — 2026-04-12

Fix LLM transcript cleanup silently failing on machines where the Python venv or model cache is broken.

### Fixed

- **"Failed to load model" with no detail** — The Python sidecar's real exception (ImportError, missing weights, MLX crash, etc.) was being swallowed and replaced with a generic error string, and its stderr was routed to `/dev/null` for windowed app launches. The exception, type, and traceback now flow through the JSON protocol, and sidecar stderr is streamed line-by-line into `SottoASR.log` under the `[llm-sidecar]` tag.
- **Stale or broken venv not detected** — `is_venv_ready()` used to only check that `bin/python3` existed as a file. It now actually exec's the venv Python with `import mlx_lm; import huggingface_hub` and caches the result, so a venv whose underlying system Python was upgraded or removed is correctly reported as broken.
- **Incomplete model downloads treated as ready** — `is_model_downloaded()` used to accept the mere existence of the `snapshots/` directory, which is true even after an interrupted download with zero weight files. It now requires at least one `.safetensors` file under `snapshots/*/`.
- **Model deletion pointed at the wrong cache path** — `delete_model()` was looking under `~/Library/Caches/huggingface/hub/` (what `dirs::cache_dir()` returns on macOS) while HuggingFace actually writes to `~/.cache/huggingface/hub/`, so uninstalling the model did nothing. Fixed to match.

## [0.7.0] — 2026-04-12

Recording duration extended to 20 minutes, LLM cleanup reliability overhaul with status badges, multi-monitor overlay positioning fix, and a comprehensive automated test suite.

### Added

- **20-minute recordings** — Maximum recording duration raised from 12 to 20 minutes for longer dictations. The countdown warning now appears at 19 min instead of 11 min, and the audio buffer cap was raised accordingly.
- **LLM cleanup status badges** — The overlay shows a brief outcome badge after cleanup completes (e.g. "Cleaned", "Cleanup timed out", "Cleanup unavailable"). History entries that fell back to the raw transcript now show a warning badge with a tooltip explaining why.
- **Native overlay drag on NSPanel** — The recording pill is now draggable on macOS via a native NSPanel drag. The previous CSS-based `-webkit-app-region: drag` heuristic did not fire on the overlay's panel configuration; a mousedown on the pill background now invokes a native `overlay_start_drag` command.

### Changed

- **LLM cleanup outer timeout 120s → 300s** — The outer timeout for the cleanup sidecar was raised to handle 15+ min recordings comfortably. The sidecar respawns automatically on hangs and orphaned subprocesses are SIGKILL'd to prevent accumulation.
- **Cleanup output token budget rewritten** — Max output tokens changed from `max(256, 1.5×input_words)` to `min(16384, max(4096, 2.5×input_words))`, fixing truncated output on long transcripts and allowing proper paragraph formatting.
- **Detailed LLM cleanup outcome tracking** — Each transcription now carries an `LlmCleanupStatus` (Applied / SkippedTooShort / Disabled / Unavailable / Failed / TimedOut / Idle) instead of just a boolean flag, surfacing exactly why cleanup didn't apply when the user sees the raw transcript in history.

### Fixed

- **Overlay straddling display boundaries on multi-monitor setups** — Saved overlay positions are now discarded when the display arrangement changes, and a valid position is recomputed on each show. Spec: `docs/specs/2026-04-11-overlay-positioning-multi-monitor-fix.md`.
- **LLM sidecar zombie processes** — The cleanup engine now detects zombie/orphan sidecar states and SIGKILLs them rather than leaving them running, preventing unified-memory pressure from old processes.

### Infrastructure

- **Full automated test suite** — 78 Rust unit tests, 90 frontend tests via Vitest, plus a pipeline integration harness with mock audio / ASR / LLM / paste backends so the recording flow can be exercised end-to-end without real hardware, models, or accessibility permissions.
- **Trait-based backend architecture** — Audio capture, ASR, LLM cleanup, and paste operations were refactored to trait objects (`src-tauri/src/test_support.rs`, new `pipeline.rs`, `paste/backend.rs`, `commands/overlay.rs`, `llm/cleanup.rs`), enabling integration tests and a clean split between production and test code paths.
- **CI assertions script** — `scripts/ci-checks.sh` runs in the release workflow before the build to verify version consistency across all five files, capability completeness, CHANGELOG coverage, hardcoded version strings, and command registration.
- **Pre-release smoke test script** — `scripts/pre-release-check.sh` runs 10 automated checks plus 5 guided manual checks for developer QA before tagging a release.
- **Updated build workflow** — `.github/workflows/build-release.yml` now runs Rust tests and frontend type checks before the build step.
- **Training data augmentation for paragraph formatting** — 4,012 new multi-paragraph samples generated to address a 0.14% gap in the original training set; the benchmark dataset was expanded with 12 hand-crafted multi-paragraph examples plus 25 recovered rows (147 total / 13 categories).
- **Specs:** `docs/specs/2026-04-04-{ci-assertions,frontend-tests,integration-tests,rust-unit-tests,smoke-tests}.md`, `docs/specs/2026-04-11-{llm-cleanup-reliability,overlay-positioning-multi-monitor-fix}.md`.
- **Journals:** `docs/journals/2026-04-12-{llm-reliability-fix,retrain-with-paragraphs}.md`.

## [0.6.3] — 2026-04-04

Fix auto-update modal getting permanently stuck on "Checking for Updates."

### Fixed

- **Update check hangs indefinitely** — The Tauri updater HTTP request had no timeout, causing the modal to freeze if the network was slow or unreachable. Added a 30-second server-side timeout and a 35-second frontend safety net.
- **"Update available" relies solely on event** — If the backend event was delayed, the modal stayed on "checking" even though the check succeeded. The modal now transitions directly from the command response.
- **Stale update state after upgrading** — After updating to the latest version, the modal could still show "Update Available" because the previous check's cached state was never cleared.
- **Double auto-close timer** — Opening the "up to date" screen could create two overlapping countdown timers, closing the window faster than expected.
- **No download stall detection** — If a download stalled mid-progress, the modal had no way to recover. Added a 60-second stall timeout.

## [0.6.2] — 2026-04-03

Fix app crash on launch caused by missing Swift runtime library path.

### Fixed

- **App fails to launch after update** — The v0.6.1 CI build was missing LC_RPATH entries for the Swift runtime, causing `Library not loaded: @rpath/libswift_Concurrency.dylib` on launch. Root cause: `RUSTFLAGS` environment variable in CI overrode the rpath flags in `.cargo/config.toml`. Fix: moved the Swift compatibility library search path into `.cargo/config.toml` and removed the `RUSTFLAGS` override.

### Infrastructure

- **Swift compat library path in `.cargo/config.toml`** — Added `-L` linker search path for Swift compatibility stubs (Xcode 26+), co-located with the existing `-rpath` flags so they can't be accidentally overridden.

## [0.6.1] — 2026-04-02

Fix critical memory issue where the LLM cleanup sidecar could exhaust system RAM and hang the machine.

### Fixed

- **LLM sidecar memory limits** — Set MLX Metal memory limit to 1 GB and cache limit to 128 MB, preventing the default behavior of wiring up to 75% of system RAM as non-swappable memory (see [ml-explore/mlx-lm#883](https://github.com/ml-explore/mlx-lm/issues/883)).
- **Metal buffer cache cleanup** — Call `mx.clear_cache()` after warmup inference and before/after each cleanup inference to prevent stale Metal buffer accumulation (see [ml-explore/mlx-lm#1015](https://github.com/ml-explore/mlx-lm/issues/1015)).
- **Duplicate sidecar processes** — Update check now reuses the existing LLM sidecar instead of spawning a separate Python process that competes for unified memory.
- **Sidecar replacement race** — Post-download preload now explicitly shuts down the old sidecar before replacing it.
- **Cleanup timeout log message** — Corrected misleading "30s" to the actual 120s timeout value.

### Added

- **Launch at login** — The app can now start automatically when you log in. Toggle in Settings; synced with macOS login items via `tauri-plugin-autostart`.

## [0.6.0] — 2026-04-02

Tray icon reliability improvements, dedicated update window, and upgraded cleanup model.

### Added

- **Global hotkey to open Settings** — Press `⌘⇧,` (Cmd+Shift+Comma) to open the Settings window from anywhere, even if the tray icon is hidden. Configurable in settings via `open_settings_shortcut`.
- **Tray icon occlusion detection** — The app monitors whether its menu bar icon is hidden behind the MacBook notch or pushed out by other icons. When detected, a macOS notification warns the user and mentions the Settings hotkey as a fallback.
- **Dedicated update window** — "Check for Updates" in the tray menu now opens a dedicated update window with download progress, replacing the inline update UI in the About window.

### Changed

- **Tray icon created in RunEvent::Ready** — The system tray icon is now created after the event loop is fully initialized, fixing the ghost/duplicate icon timing bug on macOS ([tauri#9480](https://github.com/tauri-apps/tauri/issues/9480)). Removed the `trayIcon` config from `tauri.conf.json` to ensure a single creation path.
- **Tray icon uses proper template image** — The programmatic tray builder now uses the dedicated template PNG instead of the default window icon, ensuring correct appearance in light/dark mode.
- **Simplified tray menu** — Consolidated update-related menu items into a single "Check for Updates" / "Update Available" / "Restart to Update" item that adapts its label to the current state. Menu event handler is now wired before setting the menu, fixing a first-click race condition.
- **LLM sidecar pre-loads at startup** — When transcript cleanup is enabled and the model is downloaded, the LLM sidecar spawns and loads the model in the background at launch.
- **LLM sidecar warmup inference** — The Python sidecar runs a short warmup inference after loading the model to trigger MLX lazy graph compilation, eliminating first-request latency.
- **Upgraded cleanup model attribution** — Licenses, About window, and website updated from Qwen 3.5 to SottoASR's fine-tuned LFM2.5-350M model.
- **Refactored shortcut registration** — `register_shortcuts` now accepts a `&Settings` struct instead of 7+ individual parameters.

### Fixed

- **First tray menu click ignored on macOS** — The menu event handler is now registered before the menu is attached to the tray icon, preventing a race where the first click was lost.

## [0.5.3] — 2026-04-02

Eliminate cold-start latency for LLM transcript cleanup.

### Changed

- **LLM sidecar pre-loads at startup** — When transcript cleanup is enabled and the model is downloaded, the LLM sidecar now spawns and loads the model in the background at app launch. The first cleanup no longer pays the cold-start penalty.

## [0.5.2] — 2026-04-02

Auto-update support so users are notified of new versions without checking GitHub.

### Added

- **Auto-update mechanism** — The app now checks for updates on launch and every 4 hours. When a new version is available, a badge dot appears on the tray icon and an "Update Available" item appears at the top of the tray menu. One click to download, install, and restart.
- **"Check for Updates" menu item** — Always available in the tray menu for manual checks.
- **Update status in About window** — Shows current version status with a "Download & Install" button when an update is available.
- **"Auto-check for updates" setting** — Toggle in Settings > Behavior to disable automatic checks.
- **Ed25519 signature verification** — All update artifacts are cryptographically signed. The app verifies signatures before installing, preventing tampered updates.
- **App Translocation detection** — Warns users running from a quarantined path to move the app to /Applications.

### Infrastructure

- **Updater signing keys** — CI pipeline now signs update artifacts with a Tauri Ed25519 keypair.
- **`latest.json` manifest** — Automatically generated and uploaded to GitHub Releases by the CI workflow.

## [0.5.1] — 2026-04-01

Fix model update detection and add HuggingFace model link in settings.

### Fixed

- **Model update button stuck in loop** — After clicking "Update Available — Install", the button would reappear immediately because the local revision check picked an arbitrary cached revision from a non-deterministic set instead of the most recent one.

### Added

- **HuggingFace model link** — The AI Transcript Cleanup section in Settings now shows a "View on HuggingFace" link to the cleanup model page.

## [0.5.0] — 2026-04-01

Reliable multi-monitor overlay, draggable positioning, and a switch to SottoASR's own fine-tuned cleanup model.

### Added

- **Multi-monitor overlay reliability** — The overlay now reliably appears on the correct monitor by bypassing Tauri's buggy monitor APIs (tauri#10980, #7890, #14825) in favor of native macOS APIs (`NSScreen`, `CGWindowListCopyWindowInfo`, `setFrameOrigin:`).
- **Overlay follows focused app** — The overlay appears on the monitor containing the focused application's window (like Spotlight/Raycast), not the mouse cursor. Falls back to mouse → primary screen.
- **Draggable overlay** — The recording pill can be dragged to any position on screen.
- **Per-monitor position memory** — Dragged positions are saved per monitor (keyed by `CGDirectDisplayID`) and restored on subsequent recordings. Positions are clamped to the current visible frame on restore.
- **LLM model update support** — Settings panel can check for and apply updates to the cleanup model.
- **Multi-monitor diagnostic logging** — Screen configuration, target selection reasoning, and final position are logged for debugging.

### Changed

- **Overlay position lowered** — Default position is now just above the Dock (8pt padding) instead of 100pt above the bottom, making better use of screen space.
- **Switched to fine-tuned SottoASR cleanup model** — Replaced generic Qwen3.5 models (0.8B/2B/4B) with a purpose-built `sotto-cleanup-lfm25-350m` model (233 MB, 5-bit quantized). Faster inference, better transcript cleanup quality (ROUGE-L 0.926, 99% filler-free).
- **Removed model size selector** — Single optimized model replaces the previous three-size choice. Settings fields `llm_model_size` and `llm_markdown_mode` removed.
- **LLM cleanup timeout extended** — Increased from 30s to 120s to handle long transcriptions.
- **Feature flag renamed** — `llm-qwen` → `llm-cleanup` to reflect the model change.

### Fixed

- **Overlay appearing off-screen on multi-monitor setups** — Coordinate system mismatch between `NSEvent.mouseLocation` (Cocoa logical points) and Tauri's physical pixel monitor bounds caused wrong monitor detection on mixed-DPI setups.
- **Overlay left-aligned instead of centered** — Tauri's `set_position()` applied broken coordinate transforms on secondary monitors. Replaced with native `setFrameOrigin:` in Cocoa coordinates.

### Infrastructure

- **Spec:** `docs/specs/2026-04-01-multi-monitor-overlay-reliability.md` — full investigation, root cause analysis, and implementation record.
- **Research:** Fine-tuned model training documentation and synthetic data generation research.

## [0.4.0] — 2026-03-30

LLM transcript cleanup quality improvements and comprehensive audit remediation.

### Added

- **Dictation command support** — Spoken punctuation ("period", "comma", "slash", "question mark", "exclamation point") is now converted to actual punctuation marks during LLM cleanup.
- **Contributors section** in the About page (Ian Scofield, Young Park).
- **Dependency vulnerability scanning** in CI — `cargo audit` and `npm audit` now run before every release build.
- **Settings validation** — Shortcut fields cannot be empty, `max_history` is bounded (10–10,000), and duplicate shortcuts are rejected.
- **Clipboard change detection** — Clipboard restore after paste now checks `NSPasteboard.changeCount` to avoid overwriting content the user copied during the 500ms restore delay.
- **Full application audit** — 4-pass, 59-finding audit covering security, reliability, correctness, and code quality (`docs/audit/2026-03-30-full-audit.md`).

### Fixed

- **LLM cleanup rewriting user's words** — Rewrote the system prompt with structured rules, examples, and explicit "do not paraphrase" instructions. Emphasis words ("really", "very", "definitely") and phrases ("go ahead and", "a lot of") are now preserved.
- **Thinking tags leaking into output** — Qwen3/3.5 models sometimes emit `<think>...</think>` blocks even when thinking is disabled. These are now stripped from output in both the sidecar and benchmarks.
- **Recording commands broken** — `start_recording` IPC command set state to Recording without starting audio capture, overlay, or auto-stop timer, permanently corrupting the state machine. Now delegates to the hotkey manager's full recording lifecycle. `stop_recording` and `cancel_recording` were similarly fixed in 0.3.4.
- **State machine stuck after rapid re-record** — If a user started a new recording before the previous transcription completed, the stale job was discarded but state remained in Transcribing/CleaningUp. Both stale-job checks now reset state to Idle.
- **CSV export corrupted** — Two bugs on the same format string: an extra dot between `created_at` and `duration_ms`, and unescaped newlines in text fields breaking row boundaries. Both fixed.
- **Push-to-talk polling thread could run forever** — Added a 12m30s timeout to the CGEventSourceKeyState polling loop for key release detection.
- **Mutex poisoning crashes** — All 8 instances of `.lock().unwrap()` in the hotkey manager replaced with `.unwrap_or_else(|e| e.into_inner())` to recover from poisoned mutexes instead of cascading panics.
- **Audio callback heap allocations** — Pre-allocated mono buffer in the cpal audio callback to avoid `Vec::collect` and `.to_vec()` on the real-time audio thread, reducing risk of glitches.
- **Null pointer risk in CGEventTap** — Added null check for `user_info` before unsafe dereference in the key capture callback.
- **Settings persistence error silent** — `update_settings` now persists to disk before updating in-memory state and propagates write errors to the frontend.
- **Onboarding event listener leak** — Four Tauri event listeners were never cleaned up on component destroy. Now stored and unlistened in `onDestroy`.
- **LLM status check spawning Python** — `get_llm_status` no longer spawns and kills a full Python sidecar just to check if the model is downloaded. Now checks the HuggingFace cache directory directly.

### Changed

- **LLM generation parameters tuned** — Temperature 0.3, top_p 0.9, repetition_penalty 1.10 for more consistent cleanup output.
- **Output ratio bounds relaxed** — Changed from 0.4–2.0 to 0.3–2.5 with fallback reason reporting when bounds are exceeded.
- **CSP tightened** — Removed `'unsafe-inline'` from `style-src` Content Security Policy directive.
- **Removed dead code** — Unused LLM prompt constants removed from `prompts.rs` (prompts live in the Python sidecar).
- **LLM sidecar timeout handling** — Clear logging when 30s cleanup timeout fires; sidecar is respawned on next use.

### Infrastructure

- LLM benchmark dataset expanded to 135 samples (+25 preserve_wording, dictation_commands categories).
- Parameter sweep script (`sweep_params.py`) for systematic LLM tuning.
- Prompt experiment variants (few-shot, must-preserve) for A/B testing.
- Added `journals/` and `audit/` doc categories to project rules.
- Documented App Sandbox rationale in `Entitlements.plist`.

## [0.3.4] — 2026-03-29

Fix overlay reliability on subsequent recordings.

### Fixed

- **Overlay not showing on second recording** — Fixed known Tauri issue #13530 where `always_on_top` is lost after calling `hide()` then `show()` on a window. The overlay now reliably appears on all recordings by re-applying the floating window level after each show.
- **Handle panel conversion fallback** — Added fallback to check for regular webview window if panel doesn't exist, handling the rare case where panel conversion failed and a plain window was used instead.

## [0.3.3] — 2025-07-07

Bug fixes from comprehensive code audit.

### Critical

- **Memory leak** — Clean up `window.__resetOverlay` global on component destroy
- **Date handling** — Handle invalid dates gracefully in `formatRelativeTime` (was showing "NaN seconds ago")
- **Audio buffer** — Add max audio buffer limit (15 min) to prevent memory exhaustion
- **Security** — Tighten CSP for better XSS protection

### Fixed

- Clean up setTimeout calls in settings-panel.svelte, history-item.svelte, onboarding-view.svelte, shortcut-recorder.svelte
- Deep merge settings with defaults on load
- Add timeout to LLM engine quit (3s)
- Add HTTP timeout for model downloads (5min)

## [0.3.2] — 2026-03-23

Critical CPU usage fix.

### Fixed

- **34% idle CPU usage** — the CGEventTap run loop was calling `CFRunLoopRunInMode` with a 2-second timeout in a tight loop, generating continuous mach messages to the window server. Replaced with `CFRunLoopRun()` which blocks until the tap is invalidated. CPU at idle is now 0.0%.

## [0.3.1] — 2026-03-23

Multi-monitor, multi-space, and paste target improvements.

### Added

- **Stop-and-transcribe button** in recording overlay — green checkmark alongside the cancel button so users can stop recording without remembering the keyboard shortcut.

### Fixed

- **Overlay now visible on all macOS Spaces** — the recording pill follows the user across virtual desktops, matching the behavior of macOS's screenshot toolbar (Cmd+Shift+5).
- **Overlay positions on the active monitor** — on multi-monitor setups, the pill now appears on the screen where the mouse cursor is, not always the primary monitor.
- **Smart paste target when switching apps** — if the user switches to a different app during recording, the transcription now pastes into the current app instead of jumping back to the original one.

## [0.3.0] — 2026-03-23

AI transcript cleanup model selection and shortcut improvements.

### Added

- **LLM model size selection** — Choose between Qwen3.5 0.8B (fast, ~570 MB), 2B (balanced, ~1.4 GB), or 4B (best quality, ~3 GB) in Settings. Default changed to 2B based on benchmarking showing best quality-to-speed tradeoff (ROUGE-L 0.880).
- **Alt shortcut support** — Push-to-talk, toggle, and cancel shortcuts each support an optional alternate binding.
- **Improved shortcut recorder** — Better modifier key tracking and key capture via CGEventTap.
- **Multi-model benchmark tooling** — `compare_models.py` for running the LLM benchmark suite across model sizes.

### Fixed

- **Recording timer starting at ~0:30 on first recording** — Timer now correctly initializes at 0:00 by setting `isRecording` to false at overlay creation and resetting overlay state on hide.
- **LLM summarizing long/complex inputs** — The 0.8B model was too small to follow "preserve all content" instructions. Upgrading default to 2B resolves this (user-reported long_06 example: ROUGE-L improved from 0.454 to 0.976).

### Changed

- Default LLM model changed from 0.8B to 2B for better instruction following.
- LLM sidecar accepts `--model` CLI argument and auto-respawns when model selection changes.

## [0.2.4] — 2026-03-22

Paste reliability improvements.

### Fixed

- **Paste failing in non-terminal apps (Claude Desktop, browsers, Electron apps)** — The per-paste accessibility functional check (`AXFocusedApplication` query) produced false negatives when called from background threads or during focus transitions, blocking paste even though accessibility was granted. Replaced with a one-time startup check; paste now relies solely on `AXIsProcessTrusted()`.
- **Recording timer starting at ~0:30 on first recording** — The overlay's `startTime` was initialized to `Date.now()` at component mount (app startup) instead of at recording start. Now initializes to 0 and only sets when recording begins.

## [0.2.3] — 2026-03-22

Paste reliability, diagnostics, and polish.

### Fixed

- **Paste-at-cursor focus race condition** — When transcription completed quickly (short recordings, no LLM), the simulated Cmd+V could land in the wrong app or be silently dropped. Now captures the frontmost app PID when recording starts, re-activates it before pasting, and falls back to AppleScript activation if needed.
- **First-recording focus theft** — The overlay window creation on the first recording after launch stole focus from the user's app (Tauri bug #9065). The overlay is now pre-created at startup so the first recording behaves identically to subsequent ones.
- **About page version blank** — The About window was missing from the Tauri capabilities config, so `getVersion()` had no permission to call the API. Added `"about"` to the capabilities window list.
- **Duplicate log lines** — Every log line was written twice because `tauri_plugin_log` defaults already include Stdout + LogDir targets, and we were appending two more. Switched from `.target()` (appends) to `.targets()` (replaces).

### Added

- **"Paste in original app" setting** — New toggle in Settings (default: ON). When enabled, restores focus to the app that was active when recording started before pasting. When disabled, pastes into whatever app is currently focused (legacy behavior).
- **"Copy Diagnostics" tray menu item** — Copies app version, macOS version, and the last 100 log lines to the clipboard for easy bug reporting.

## [0.2.2] — 2026-03-22

Code signing and notarization for macOS.

### Added

- **Apple code signing** — App is now signed with a Developer ID Application certificate. Accessibility permissions persist across updates.
- **Apple notarization** — App is notarized and stapled by Apple. No more Gatekeeper warnings on first launch.
- **CI/CD signing pipeline** — GitHub Actions workflow now imports the signing certificate, signs the build, and submits for notarization automatically.

### Changed

- **Website** — Replaced the "Unsigned build" Gatekeeper warning with a "Signed & Notarized" badge.

## [0.2.1] — 2026-03-22

A polish release: rebrand to SottoASR, new app icon, macOS-native tray icon, and a recording timer bug fix.

### Changed

- **Rebrand from "Sotto" to "SottoASR"** — Updated product name, bundle identifier (`com.sottoasr.app`), Rust crate name, package name, all UI strings (tray menu, onboarding, settings, about, window titles), and log file name.
- **New app icon** — Redesigned icon across all platform sizes with an SVG source.
- **macOS template tray icon** — Proper `tray-iconTemplate.png` with `iconAsTemplate: true` so the menu bar icon adapts to light/dark mode automatically.
- **About page** — More complete dependency attribution (added cpal, hound, rubato, huggingface_hub, Tokio, serde). Updated license notice referencing 660+ dependencies and the new THIRD_PARTY_LICENSES file.

### Fixed

- **Stale auto-stop timer across recordings** — Added a recording generation counter so auto-stop timers from a previous session are discarded instead of interfering with a new recording.

### Added

- **THIRD_PARTY_LICENSES** file listing all 660+ transitive dependencies and their licenses.
- **Static landing page** (`website/`) deployed via Cloudflare Pages.

### Infrastructure

- Updated GitHub Actions release workflow with improved release body template.
- Updated LICENSE, README, DEVELOPMENT.md, and architecture docs to reflect the SottoASR rebrand.

## [0.2.0] — 2026-03-21

A major update adding AI-powered transcript cleanup, customizable shortcuts, recording cancellation, persistent storage, and a fully reworked permissions and onboarding flow.

### Added

- **AI Transcript Cleanup** — On-device LLM post-processing using Qwen3.5-0.8B (4-bit quantized, ~570 MB) running via Apple MLX on Metal GPU. Removes filler words, fixes grammar, and optionally formats output as Markdown. Enabled per-user in Settings; requires Python 3.10+.
- **Interactive Shortcut Recorder** — Click-to-record keyboard shortcut input in Settings. Displays macOS-native symbols (⌘, ⇧, ⌥, ⌃, ␣). Supports media keys and function keys via CGEventTap system-wide key capture.
- **CGEventTap Key Capture** — Background system-wide key event listener that captures all keys including media/function keys that browsers cannot detect. Used by the shortcut recorder and for push-to-talk key release detection.
- **Recording Cancellation** — Cancel an in-progress recording via shortcut or the overlay's cancel button. Cancelled recordings are saved to history with a "Cancelled" badge but are not pasted.
- **About Window** — Accessible from the tray menu. Shows app version and credits all major dependencies with license information.
- **CSV Export** — Export transcription history as CSV from the history view. Falls back to clipboard copy if file save is unavailable.
- **Recording Duration Limit** — Recordings auto-stop at 12 minutes. A red pulsing countdown warning appears at 11 minutes.
- **History Diff View** — For LLM-cleaned transcriptions, view a word-level inline diff showing exactly what changed (red strikethrough for removed text, green for additions).
- **"AI Cleaned" and "Cancelled" badges** on history items for quick visual identification.

### Changed

- **Overlay is now an NSPanel** — Uses `tauri-nspanel` for a proper macOS floating panel that never steals keyboard focus from the active app. Fully transparent background with no visual artifacts.
- **Permissions system overhauled** — New `check_all_permissions` command returns structured status for microphone, accessibility (API + functional test), and input monitoring. Detects the macOS Sequoia issue where accessibility is granted but not yet functional, and guides the user to restart.
- **Push-to-talk uses CGEventSource polling** — Key release is now detected via `CGEventSourceKeyState` at ~30 Hz instead of relying on `tauri-plugin-global-shortcut`'s release event, which was unreliable.
- **Shortcuts are customizable and persistent** — All three shortcuts (push-to-talk, toggle, cancel) are saved to disk and loaded on startup. The settings panel uses the new shortcut recorder for configuration.
- **Settings and transcriptions are persisted** — Settings saved to `~/Library/Application Support/com.sottoasr.app/settings.json`, transcriptions to `transcriptions.json`. Both loaded on startup.
- **Clipboard handling uses `arboard`** — Replaced `pbcopy` shell command with direct NSPasteboard access via the `arboard` crate. Clipboard is saved before paste and restored after 500 ms.
- **CGEvent pipeline warmup** — A no-op CGEvent is posted at startup to prevent the first real Cmd+V paste from being silently dropped on macOS 15 Sequoia.
- **Settings panel UI** — Save/Cancel buttons moved to a sticky header. Dirty detection disables Save when no changes exist. Richer permission status display with "Fix Permission" action for stale accessibility grants.
- **History search** now covers both cleaned and raw text.
- **Overlay pill** shows a "Cleaning up..." state with spinner during LLM processing, with a slow-processing message after 5 seconds.
- **Max transcription history** raised from 500 to 5,000 items.
- **Vite dev server** port changed to 14517 to avoid conflicts.

### Fixed

- **Waveform and timer not resetting** between consecutive recordings — ring buffer, write index, sample count, and rolling max are now properly cleared.
- **`app.exit()` silently prevented** — Exit handler now distinguishes window-close events from explicit exit calls.
- **Aggressive accessibility prompt at startup** removed — The app now checks permission status and logs a warning instead of triggering a raw FFI prompt. Onboarding handles the user-facing flow.
- **First Cmd+V paste dropped on macOS Sequoia** — Mitigated by CGEvent pipeline warmup and retry-on-first-failure with 50 ms delay.

### Infrastructure

- MIT License added with third-party notices (NVIDIA Parakeet, Qwen, FluidAudio, MLX, Tauri, Svelte).
- GitHub Actions release workflow now uses `__VERSION__` placeholder for DMG filename.
- Added `diff` (v8.0.3) npm dependency for word-level history diffs.
- Added `arboard` (v3) and `tauri-nspanel` Rust dependencies.

## [0.1.0] — 2026-03-21

Initial release.

- Global push-to-talk and toggle hotkeys for hands-free recording
- On-device speech recognition via FluidAudio (CoreML/Apple Neural Engine)
- Automatic paste-at-cursor via simulated Cmd+V
- Floating overlay with real-time waveform visualization
- Transcription history with search and copy
- Menu bar app (no Dock icon)
- Onboarding flow for permissions setup
- macOS 14+ / Apple Silicon only
