# Changelog

All notable changes to SottoASR are documented in this file.

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
