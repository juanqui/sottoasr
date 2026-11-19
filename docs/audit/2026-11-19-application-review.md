# Application Review, November 2026

- **Version:** 1.1
- **Date:** 2026-11-19
- **Status:** Implemented

## Table of Contents

1. [Method](#1-method)
2. [Baseline](#2-baseline)
3. [Findings](#3-findings)
4. [What to keep](#4-what-to-keep)
5. [Limits](#5-limits)
6. [Remediation](#6-remediation)

## 1. Method

Source review of v0.11.1 (`05572e8`). Five read-only reviewers covered audio/ASR,
cleanup, macOS integration, frontend, and security. The primary reviewer covered
build, CI, release, and documentation, then re-read the cited lines for each finding.

A finding marked **V** was confirmed by reading the cited code. A finding marked **R**
was reported by one reviewer and not re-read by the primary reviewer. Nothing was
launched; native microphone, focus, and paste behaviour were not exercised.

## 2. Baseline

| Check | Result |
| --- | --- |
| `cargo clippy --all-targets --locked -- -D warnings` | Clean |
| `cargo test --locked` | 243 pass in 60.5 s; one cleanup-planner test takes about 60 s |
| `npm run check` / `npm test` | 0 errors / 168 pass |
| `cargo audit` | Fails with 12 advisories (rustls ×4, rustls-webpki, webpki-root-certs, zip ×6) |
| `npm audit` | 6 advisories in development dependencies, 0 in production dependencies |

Only `rustls` 0.23.45 is compiled for `aarch64-apple-darwin`, through
`tauri-plugin-updater`. The CI audit step is blocking regardless of target.

## 3. Findings

Severity: H = user-visible failure or supply-chain exposure, M = degraded behaviour,
L = minor. Paths are under `src-tauri/src/` unless rooted.

### 3.1 Reliability

| ID | Sev | Finding | Evidence | |
| --- | --- | --- | --- | --- |
| R-01 | H | A saved setting can disable every hotkey at next launch. An empty dictionary row or a cleared cancel shortcut saves, then startup validation fails and registers nothing. | `commands/settings.rs:107` calls only `normalize()`; `hotkeys/manager.rs:58` runs `validate()?` | V |
| R-02 | H | A mic unplug or AirPods drop is not detected. Truncated text is pasted as complete. | cpal 0.15.3 `macos/mod.rs:616` skips its disconnect listener for the default device; `audio/capture.rs:204` | V |
| R-03 | H | Tray menu clicks run once per past refresh. | `tray/menu.rs:157` re-registers `on_menu_event`; Tauri appends listeners; `updater/mod.rs:127` refreshes every 4 h | V |
| R-04 | H | Cleanup has no total time budget and keeps dispatching after a sidecar failure. | `llm/correction.rs:555-563` | V |
| R-05 | M | A denied microphone permission produces no text and no message. | Start path never checks authorization; `hotkeys/manager.rs:568` skips empty text | V |
| R-06 | M | A busy app cannot be quit. ASR has no deadline, and model download holds the engine lock. | `lib.rs:316`; `commands/overlay.rs:145`; `commands/setup.rs:52` | V |
| R-07 | M | Cleanup model download has a fixed 900 s deadline for 1,427 MB and no progress. | `llm/engine.rs:92` | V |
| R-08 | M | The shortcut recorder drops Control when Command is held. | `src/lib/components/shortcut-recorder.svelte:203` | V |
| R-09 | L | The overlay stays hidden for a recording if the settings mutex is contended at start. | `hotkeys/manager.rs:875` | V |
| R-10 | M | The key-capture event tap is never re-enabled after macOS disables it. | `commands/keycapture.rs` has one `CGEventTapEnable` call | V |
| R-11 | M | Replace-mode edit lines can grow the text without bound. | `llm/correction.rs:182-210` | V |
| R-12 | H | Repair cannot rebuild a broken cleanup runtime. | `llm/engine.rs:1136-1160` | R |
| R-13 | M | The validator lets one of a repeated pair of number words be deleted. | `llm/validation.rs:37-62` | R |
| R-14 | M | Clicking the error overlay can make the next paste target SottoASR itself. | Panel lacks the non-activating mask; `audio/capture.rs:159` | R |
| R-15 | M | Onboarding overwrites the real ASR error, never shows the Accessibility prompt, and loops on a parakeet build. | `src/lib/components/onboarding-view.svelte:128,157-185`; `commands/setup.rs:124` | R |
| R-16 | L | Preparation races: overlapping prewarms clear each other's flag, the first startup retry reads a cached verdict, and a Repair during Ensure is dropped. | `llm/cleanup.rs:70,83`; `lib.rs:223`; `commands/llm.rs:98` | R |
| R-17 | M | The update window can stick or be overridden by background checks. The model-update path shows a false stall. | `src/lib/components/update-view.svelte:38-64,243-257` | V |
| R-18 | L | `set_state` and `get_state` ignore a poisoned mutex; every other lock user recovers it. | `state.rs:276,287` | V |
| R-19 | M | Capture queue headroom is counted in callbacks, and every checkpoint is a full device flush. | `audio/recovery.rs:84,142` | V |
| R-20 | L | "Unsaved changes" appears for a no-op edit, history timestamps never update, and the overlay says "Preparing suggestion". | `src/lib/stores/settings.svelte.ts:66`; `history-item.svelte:24`; `overlay-pill.svelte:317` | R |
| R-21 | M | An unreadable `settings.json` leaves no hotkeys and no in-app repair. | `commands/settings.rs:31-35` | R |

### 3.2 Latency

| ID | Sev | Finding | Evidence | |
| --- | --- | --- | --- | --- |
| L-01 | M | Every paste re-activates the target app and sleeps 150 ms, even when it is already frontmost. Activation raises all of its windows. | `hotkeys/manager.rs:575`; `paste/macos.rs:57-61,198,241` | V |
| L-02 | M | History is serialized and synced before the paste. | `hotkeys/manager.rs:557` precedes `:586`; `commands/transcription.rs:36-47` | V |
| L-03 | L | The vocabulary path decodes and resamples the recording twice. | `vendor/fluidaudio-rs/swift/FluidAudioBridge.swift:79-82` | R |
| L-04 | M | The cleanup planner is super-linear on long input. | One mock-backend test takes about 60 s; `llm/validation.rs:533` compiles a regex per term per call | V |
| L-05 | L | The audio thread emits the level to every window. | `hotkeys/manager.rs:294` | R |

### 3.3 Privacy and supply chain

| ID | Sev | Finding | Evidence | |
| --- | --- | --- | --- | --- |
| P-01 | M | Transcripts reach the pasteboard unmarked, and restore is text-only. | `paste/macos.rs:36-45` | V |
| P-02 | M | Retained recordings are never purged; "Clear All" removes text only. | `commands/transcription.rs:172-184`; `commands/recovery.rs:489` | V |
| P-03 | M | History keeps 5,000 plaintext entries with no retention control. `max_history`, `language`, and `model_path` are inert. | `models.rs:126-128`; `commands/transcription.rs:43` | V |
| P-04 | H | The cleanup runtime installs four pinned packages with no hashes; transitive dependencies float. | `llm/engine.rs:779-784,1169-1182` | V |
| P-05 | H | The release job gives signing keys to actions referenced by mutable tags, and exposes Apple credentials to every step. `npm audit` cannot fail the build. | `.github/workflows/build-release.yml:31-34,38,54,60,106,145` | V |
| P-06 | L | Every window is granted permissions the frontend never uses. Three plugins are registered and unused. | `src-tauri/capabilities/default.json`; `lib.rs:41-44` | V |
| P-07 | L | The design document says the only network access is the first model download. | `docs/designs/architecture.md:1282`; `updater/mod.rs:98-108` | V |
| P-08 | M | Speech and vocabulary models download from a mutable branch with no hash check. | `vendor/fluidaudio-rs/swift/AsrCache.swift:14` | R |
| P-09 | L | The sidecar inherits the full environment, system tools resolve through `PATH`, `open_url` accepts any HTTPS URL, and diagnostics include the home path. | `llm/engine.rs:157-163`; `commands/permissions.rs:229-240`; `tray/menu.rs:277` | V |
| P-10 | H | The dependency audit gate is red. | Section 2 | V |

### 3.4 Structure

| ID | Sev | Finding | Evidence | |
| --- | --- | --- | --- | --- |
| S-01 | H | The pipeline tests exercise a copy of the stop path. Production code has one test. | `pipeline.rs` is referenced only by `test_support.rs`; `hotkeys/manager.rs:391-670` | V |
| S-02 | M | Dead paths ship: the model-update check, `llm/edits.rs`, five `src/*.html` files, an unused store, and 15 unused IPC wrappers. | `sidecar/llm_cleanup.py:495`; `vite.config.ts` | V |
| S-03 | M | The `asr-parakeet` feature is not built in CI and rejects 48 kHz recordings. | `asr/parakeet_backend.rs:69` | R |
| S-04 | L | `VenkVerdict` typo, hand-written `PartialEq`, duplicated `AppState` constructors, crate-level `deny(warnings)`. | `llm/engine.rs:618`; `models.rs:219`; `state.rs:104,166`; `lib.rs:1` | V |
| S-05 | M | Documentation drift: README model size and clone path, `DEVELOPMENT.md`, `.claude/rules/project.md`, `opencode.json`. | README says 227 MB; `llm/engine.rs:1369` says 1,427 | V |
| S-06 | L | No test ties the Rust pins to the Python sidecar pins. The frontend IPC test checks wrappers against themselves. | `llm/engine.rs:779`; `sidecar/llm_cleanup.py:36` | R |

### 3.5 Product gaps

| ID | Gap | Evidence | |
| --- | --- | --- | --- |
| G-01 | No microphone selection. | `audio/capture.rs:204` opens only the default device | V |
| G-02 | No audio cue on start or stop. | No sound code in `src-tauri/src` | V |
| G-03 | Hotkey presses while transcribing are dropped silently. | `hotkeys/manager.rs:112,168` | R |
| G-04 | No progress for the speech or cleanup model downloads. | `onboarding-view.svelte` shows a spinner | R |
| G-05 | Dark mode only. | `src/app.css` sets `color-scheme: dark` | V |
| G-06 | Cleanup requires the user to install Python 3.11+. | `llm/engine.rs:1003-1030` | V |
| G-07 | Frontend console output never reaches the log file. CSV export has no "Show in Finder". | `src/` never imports `@tauri-apps/plugin-log` | R |
| G-08 | Running cleanup in-process is unexplored. | `mlx-swift-lm` registers the checkpoint's `llama` model type | V |

## 4. What to keep

- Atomic 0600 writes that never replace an unreadable original.
- One `current_state` lock for start, stop, cancel, exit, and shortcut updates, with generation checks on timers.
- Cleanup output rebuilt only from source bytes, with a final whole-transcript check.
- A revision-pinned, per-file hashed cleanup model with `trust_remote_code` off.
- Owned child processes that are killed as a group and never signalled by raw PID.
- No transcript text in any log statement.

## 5. Limits

Timing claims come from sleeps and call order in the code, not from measurements.
Native behaviour that depends on focus, TCC, or hardware is unverified. Findings
marked R carry the reviewer's confidence only.

## 6. Remediation

All 50 findings were closed on 2026-11-19 on branch `feature/review-remediation`,
following the
[review remediation specification](../specs/2026-11-19-review-remediation.md).
Section 12 of that document records the result for each ID, the verification
runs, and the behaviour that still needs a run of the app.

| Group | Findings | Result |
| --- | --- | --- |
| Reliability | R-01 to R-21 | Fixed |
| Latency | L-01 to L-04 | Fixed |
| Latency | L-05 | Closed without a change: Tauri already delivers an event only to windows that listen for it |
| Privacy and supply chain | P-01 to P-09 | Fixed |
| Privacy and supply chain | P-10 | Fixed for `rustls`, `rustls-webpki`, `webpki-root-certs` and the development packages. The six `zip` advisories are ignored with a CI guard, because `zip` is not compiled for macOS and its fix needs a Tauri upgrade |
| Structure | S-01 to S-06 | Fixed |
| Product gaps | G-01 to G-07 | Added |
| Product gaps | G-08 | Explored and not adopted; see the [spike](../research/2026-11-19-in-process-cleanup-spike.md) |

Six independent reviewers read the implementation, and five of them read the
fixes a second time. The remediation is not committed and the app was not
launched, so the limits in section 5 still apply to the new code.
