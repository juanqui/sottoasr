# Development Guide

This document tells you how to build, test and debug SottoASR.

## Table of Contents

- [Prerequisites](#prerequisites)
- [Environment Setup](#environment-setup)
- [Project Structure](#project-structure)
- [Architecture Overview](#architecture-overview)
- [Running in Development](#running-in-development)
- [Building for Production](#building-for-production)
- [Testing and Linting](#testing-and-linting)
- [Feature Flags](#feature-flags)
- [Pinned Models and the Cleanup Runtime](#pinned-models-and-the-cleanup-runtime)
- [Known Issues and Workarounds](#known-issues-and-workarounds)
- [Debugging](#debugging)
- [Contributing](#contributing)

## Prerequisites

| Tool | Minimum Version | CI Uses | Install |
|---|---|---|---|
| Rust | 1.80 (`rust-version` in `src-tauri/Cargo.toml`) | 1.94.0 | [rustup.rs](https://rustup.rs/) |
| Node.js | 20.19 or 22.12 (required by Vite 8) | 24 | [nodejs.org](https://nodejs.org/) |
| Xcode or Xcode Command Line Tools | -- | -- | `xcode-select --install` |
| Tauri CLI | 2 | -- | `cargo install tauri-cli --version "^2" --locked` |
| Python 3 | 3.11 (for the Python tests and `scripts/pre-release-check.sh`) | runner `python3` | -- |
| `jq` | -- | runner `jq` | -- |
| macOS | 14 Sonoma | -- | -- |
| Apple Silicon | M1 | -- | -- |

The vendored FluidAudio bridge is Swift code, so the build needs the Swift compiler. `src-tauri/.cargo/config.toml` also adds Swift library paths inside `/Applications/Xcode.app`. The first build resolves the FluidAudio Swift package with SwiftPM, which needs network access.

To run the dependency audit locally, install `cargo-audit`:

```bash
cargo install cargo-audit --version 0.22.2 --locked
```

## Environment Setup

```bash
# 1. Install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"

# 2. Install Xcode Command Line Tools (if not already installed)
xcode-select --install

# 3. Clone the repository
git clone https://github.com/juanqui/sottoasr.git
cd sottoasr

# 4. Install frontend dependencies
npm install

# 5. Verify the setup
cargo tauri dev
```

On first launch, onboarding downloads the speech model, Parakeet Ultra (about 630 MB), to `~/Library/Application Support/FluidAudio/Models/` and verifies it. This happens only once. An installation that already has the verified Parakeet TDT v3 files skips onboarding and changes to Parakeet Ultra in the background (see [Speech Model Download and Upgrade](#speech-model-download-and-upgrade)).

CAUTION: A development build uses the identifier `com.sottoasr.app`, the same as an installed SottoASR. Thus it uses the same settings, history, recordings and global shortcuts. Quit the installed app before you run `cargo tauri dev`.

## Project Structure

```
sottoasr/
├── index.html, overlay.html, history.html,     # Vite page entries, one per window
│   settings.html, onboarding.html,
│   about.html, update.html
├── src/                                  # Svelte 5 frontend
│   ├── main.ts, overlay.ts, history.ts,  # Window entry scripts
│   │   settings.ts, onboarding.ts,
│   │   about.ts, update.ts
│   ├── app.css                           # Shared styles and theme tokens (dark and light)
│   └── lib/
│       ├── components/                   # Views, settings sections and widgets (*.svelte)
│       ├── stores/                       # Svelte 5 rune stores (*.svelte.ts)
│       └── utils/                        # IPC wrappers, log forwarding, formatting
├── src-tauri/                            # Tauri + Rust backend
│   ├── src/                              # Rust source (see the module list below)
│   ├── sidecar/                          # Python cleanup sidecar, its tests and lock file
│   ├── vendor/fluidaudio-rs/             # Swift ASR bridge, SDK patches, model manifest
│   ├── capabilities/                     # Tauri permissions per window
│   ├── examples/asr_fixture.rs           # Non-interactive ASR fixture runner
│   ├── tests/fixtures/                   # Cleanup validation fixture
│   ├── .cargo/                           # Linker flags and cargo-audit configuration
│   ├── Cargo.toml                        # Rust dependencies and feature flags
│   ├── tauri.conf.json                   # Tauri app configuration
│   ├── Info.plist                        # LSUIElement (no Dock icon), microphone usage text
│   └── Entitlements.plist                # Audio input and accessibility entitlements
├── scripts/                              # CI checks, release checks, Swift and model probes
├── benchmarks/                           # asr/, llm/ and ui/ benchmarks
├── training/                             # Earlier fine-tuning work (not used by the app)
├── website/                              # sottoasr.app static site
├── docs/                                 # Specs, research, journals, audits, designs
└── package.json                          # Frontend dependencies and scripts
```

### Windows

Each window has an HTML entry at the project root and a TypeScript entry in `src/`. The keys of `rollupOptions.input` in `vite.config.ts` match the window labels:

- **overlay** -- the floating recording pill
- **history** -- transcription history and recording recovery
- **settings** -- the settings window
- **onboarding** -- first-launch setup
- **about** -- version and acknowledgements
- **update** -- the software update window
- **main** -- `index.html`, built but not opened by the app

Each entry script first calls `forwardConsoleToLog()` from `src/lib/utils/console-log.ts`, then mounts its component.

## Architecture Overview

### Rust Backend (`src-tauri/src/`)

| Module | Purpose |
|---|---|
| `lib.rs` | App setup: plugins, the command list in `generate_handler!`, startup tasks, run events |
| `pipeline.rs` | The only dictation pipeline: start, stop, cancel and the work after a stop. It reports through the `PipelineEvents` trait. |
| `hotkeys/manager.rs` | Global shortcuts, the push-to-talk release poller, the overlay panel, and `TauriEvents`, the production `PipelineEvents` adapter |
| `state.rs` | `AppState`: the state machine, locks, and atomic mirrors of the settings that hot paths read |
| `models.rs` | Shared types: `Settings` (with `normalize` and `validate`), `Transcription`, `AppStateEnum`, `ModelStatus`, `LlmStatus` |
| `persistence.rs` | Atomic, owner-only writes of settings and history files |
| `dictionary.rs` | Exact replacements |
| `process.rs` | Owned child processes, bounded commands, cleanup at exit |
| `audio/capture.rs` | cpal capture, microphone selection, authorization and silence errors |
| `audio/recovery.rs` | The disk writer: private WAV checkpoints and the stall watchdog |
| `audio/wav.rs` | WAV serialization, and deletion of a WAV after the history save |
| `audio/cue.rs` | Optional system sounds for start, stop, cancel, busy and error |
| `audio/resample.rs` | rubato resampling to 16 kHz (`asr-parakeet` only) |
| `asr/engine.rs` | The `AsrEngine` trait and the engine lock |
| `asr/fluidaudio_backend.rs` | FluidAudio backend (default) |
| `asr/parakeet_backend.rs` | parakeet-rs ONNX backend (`asr-parakeet`) |
| `asr/model.rs` | Speech model preparation, download progress, re-download |
| `asr/integrity.rs` | SHA-256 checks of the speech and vocabulary models against `model-manifest.json` |
| `asr/vocabulary.rs` | Vocabulary validation and acoustic candidate selection |
| `paste/backend.rs` | `PasteBackend` trait and `write_text` |
| `paste/macos.rs` | CGEvent `Cmd+V` and target app activation |
| `paste/pasteboard.rs` | `NSPasteboard` snapshot, marked local-only transient write, and write-back |
| `paste/restore.rs` | The delayed clipboard restore, and the wait for it at quit |
| `llm/engine.rs` | The Python sidecar process, runtime checks, the hash-locked install, standalone Python, the model pin |
| `llm/download.rs` | Cleanup model download with progress and a stall check |
| `llm/cleanup.rs`, `llm/correction.rs`, `llm/validation.rs` | Cleanup prewarm, the window planner, and validation of proposed edits |
| `commands/` | IPC handlers: recording, transcription (history and CSV), recovery and retention, settings, audio devices, permissions, key capture, setup, vocabulary, llm, overlay |
| `tray/menu.rs` | Tray icon and menu, window opening, Copy Diagnostics |
| `updater/mod.rs` | App update checks and install |

### Svelte Frontend (`src/`)

- **Components** are in `src/lib/components/`. The overlay pill and the waveform are the most performance-sensitive parts.
- **Stores** in `src/lib/stores/` use the Svelte 5 `$state` and `$derived` runes. They must use the `.svelte.ts` extension.
- **IPC wrappers** in `src/lib/utils/tauri.ts` are the only code that calls `invoke`. Components call the wrappers.
- **Theme tokens** are in `src/app.css`. The light values apply under `@media (prefers-color-scheme: light)`. The overlay pill keeps its own dark palette.

### Capabilities

`src-tauri/capabilities/default.json` lists every window and grants only `core:default`, `core:window:allow-close` and `log:default`. `history.json` lets the history window write text to the clipboard. `settings-close.json` lets the settings window destroy itself. `scripts/ci-checks.sh` fails if a window label is missing from `default.json`.

### Data Flow

```
User presses a hotkey
  → hotkeys/manager.rs calls pipeline::start
    → audio/capture.rs opens the selected or default microphone (cpal)
      → audio/recovery.rs writes the WAV and checkpoints it every second
      → the overlay shows the input level (audio-level event)
User releases the key (or presses toggle again)
  → pipeline::claim_and_finish claims the recording and finalizes the WAV
    → pipeline::complete runs in a spawned task:
      → transcribe_with_deadline (60 s + half the recording length)
        → vocabulary, then exact replacements (dictionary.rs)
          → optional cleanup (llm/), with a hard 60 s limit
            → paste or copy (paste/)
              → add_transcription saves History
                → finish_after_history deletes the WAV
                  → state returns to Idle
```

Cancel follows the same path. It pastes nothing and saves the transcript in History as cancelled.

## Running in Development

### Full Application (Recommended)

```bash
cargo tauri dev
```

This command starts the Vite dev server on port 14517 and the Rust backend. Rust changes cause a recompile. Svelte and TypeScript changes reload in the window.

### Frontend Only

```bash
npm run dev
```

Use this mode to work on the UI without a Rust build. IPC calls to the backend fail, because Tauri is not running.

### Rust Only

```bash
cd src-tauri
cargo build
```

## Building for Production

```bash
cargo tauri build
```

This command produces:
- `src-tauri/target/release/bundle/macos/SottoASR.app` -- the application bundle
- `src-tauri/target/release/bundle/dmg/SottoASR_<version>_aarch64.dmg` -- disk image installer

**Important:** Launch the built app with `open src-tauri/target/release/bundle/macos/SottoASR.app`. Do not run the raw binary. `LSUIElement` (no Dock icon) and the TCC permission checks work correctly only through `open`.

## Testing and Linting

### Rust

```bash
cd src-tauri

# Tests
cargo test --locked

# Lint (warnings are errors)
cargo clippy --locked --all-targets -- -D warnings

# The ONNX backend: type check and resampler tests
cargo check --locked --no-default-features --features custom-protocol,asr-parakeet
cargo test --locked --no-default-features --features custom-protocol,asr-parakeet --lib audio::resample
```

`cargo test` needs `dist/`, because `tauri::generate_context!()` reads it at compile time. Run `npm run build` first.

### Frontend

```bash
npm run check    # svelte-check and tsc
npm test         # Vitest
npm run build    # Production build
```

`src/lib/utils/tauri.test.ts` is the contract test. It parses `generate_handler!` in `src-tauri/src/lib.rs` and the capability files. Then it makes sure that each wrapper calls a registered command, and that each plugin call is allowed for the windows that use it.

### Python

```bash
python3 -m unittest discover -s src-tauri/sidecar -p 'test_*.py'
python3 -m unittest discover -s scripts -p 'test_*.py'
```

The sidecar tests need no model, network or MLX, unless the cleanup model is in the Hugging Face cache. Then three more tests load it, and they need the pinned runtime: run the sidecar tests with a Python that has `src-tauri/sidecar/requirements.lock` installed. The script tests include `scripts/test_workflows.py`, which checks that each workflow action is pinned to a commit SHA and that no secret is declared at workflow or job level.

### Configuration Checks

```bash
bash scripts/ci-checks.sh
```

This script checks that the six version values match, that each window has a capability entry, that each command is registered, that each frontend `invoke` has a handler, that `CHANGELOG.md` has a section for the version, that `src/` holds no hardcoded version, and that the website badge matches. It needs `jq` and an installed `node_modules`.

### Swift Bridge Probes

```bash
python3 scripts/check-asr-cache.py
python3 scripts/check-vocabulary-cache.py
```

These probes test the loaders of the FluidAudio bridge. They need no model and no network. They need a previous `cargo build` or `cargo test`, because they use the Swift build in `src-tauri/target/`. `check-asr-cache.py` checks that a failed load or download keeps every file, that model URLs use the pinned commits, and that the SDK does not pin those repositories itself. CI does not run these probes. Run them after you change the FluidAudio version or a file in `src-tauri/vendor/fluidaudio-rs/`.

### Dependency Audits

```bash
cd src-tauri
cargo tree --locked -i zip --target aarch64-apple-darwin   # must print nothing
cargo audit
cd ..
npm audit --omit=dev
```

`src-tauri/.cargo/audit.toml` ignores six `zip` advisories. These ignores are correct only while `zip` is not built for macOS, so CI fails if the `cargo tree` command prints a dependency. If `zip` is no longer in `Cargo.lock`, CI and `scripts/pre-release-check.sh` pass with a notice to delete the ignores and the `zip` checks.

### All Checks

`./scripts/pre-release-check.sh --auto-only` runs every check of PR CI in the same order, then builds an unsigned `.app` and `.dmg`. `.claude/rules/release.md` describes the script.

## Feature Flags

The features are defined in `src-tauri/Cargo.toml`. The default is `custom-protocol`, `asr-fluidaudio` and `llm-cleanup`.

| Flag | Purpose | Default |
|---|---|---|
| `asr-fluidaudio` | FluidAudio (CoreML/ANE) speech backend, macOS only | Yes |
| `asr-parakeet` | parakeet-rs (ONNX Runtime) speech backend. It also enables `rubato`, `reqwest` and `audio/resample.rs`. | No |
| `llm-cleanup` | The optional AI cleanup feature | Yes |
| `custom-protocol` | Tauri production asset protocol | Yes |

To use parakeet-rs, use one of these commands:

```bash
cargo tauri dev --no-default-features --features custom-protocol,asr-parakeet
# or
cargo tauri build --no-default-features --features custom-protocol,asr-parakeet
```

Note: `custom-protocol` is a Tauri feature that the app needs. Always include it when you override the default features. These commands also leave out `llm-cleanup`.

`asr/engine.rs` defines the `AsrEngine` trait. The active feature flag selects the backend at compile time. Both backends implement the same trait, so the rest of the code does not depend on the backend.

## Pinned Models and the Cleanup Runtime

### Speech and Vocabulary Models

The bridge uses FluidAudio 0.18.3 and downloads the models from Hugging Face at fixed commits. Two checksum-guarded patches in `src-tauri/vendor/fluidaudio-rs/patches/`, `model-registry-revision.patch` and `tree-lister-revision.patch`, make the SDK read these commits instead of `main`. The two earlier patches for vocabulary alignment and CTC tensor access are upstream in 0.18.3. `model-manifest.json` holds the SHA-256 and size of each file that the bridge loads. `asr/integrity.rs` checks the files before CoreML loads them. `src-tauri/vendor/fluidaudio-rs/README.md` gives the full policy.

The manifest has three entries: `speech` (Parakeet Ultra), `previous_speech` (Parakeet TDT v3) and `vocabulary` (CTC 110M). The app never downloads `previous_speech` again. It only verifies and loads it for an installation that does not have Parakeet Ultra yet.

To regenerate the manifest after you select a new commit, use this command:

```bash
python3 scripts/check-model-manifest.py --revision speech=<commit> --revision vocabulary=<commit> --write
```

Then update the commits in `patches/model-registry-revision.patch` and its patched checksum in `build.rs`. A unit test fails if the manifest and the patch do not agree.

### ONNX Backend Model

The `asr-parakeet` backend downloads its model from a pinned commit of `istupakov/parakeet-tdt-0.6b-v3-onnx` on Hugging Face. `asr/model.rs` checks the size and SHA-256 of each file before it accepts the file.

### Cleanup Model and Runtime

`src-tauri/src/llm/engine.rs` pins the cleanup model (`SOTTO_MODEL`, `MODEL_REVISION`). The sidecar `src-tauri/sidecar/llm_cleanup.py` holds the same pin and the SHA-256 of each model file. A Rust test fails if the two pins, the runtime versions or the prompt hash do not agree.

The app builds every new runtime (create, sync and Repair) in `llm-venv.staging` beside the installed one. It renames the new runtime into place only after the install and the runtime probe pass. The next preparation removes an unfinished staging folder and moves back a runtime that an interrupted swap left aside. A preparation that finds the runtime Ready removes leftover aside copies. The host Python must be a native arm64 Python 3.11 to 3.14.

If the runtime differs from the lock, the preparation loads cleanup on it and returns. Then a separate task under its own claim (`Preparation::Sync`, at most once per launch) rebuilds the runtime with short pip network limits (`--timeout 10 --retries 1`). The app does not report this sync as preparing. The old runtime stays aside until the new one loads, and is put back if the new one does not load. If cleanup does not load on the out-of-date runtime, the preparation rebuilds it at once with the normal pip settings.

The app installs the runtime from `src-tauri/sidecar/requirements.lock` with `pip --require-hashes --no-deps`. To change a version, edit `requirements.in` and regenerate the lock with the command at the top of that file:

```bash
cd src-tauri/sidecar
uv pip compile requirements.in --generate-hashes --universal --python-version 3.11 --no-header --no-annotate -o requirements.lock
```

## Known Issues and Workarounds

### Accessibility Permission Invalidation (Development)

**Problem:** macOS ties the Accessibility permission to the code signature of the app. Each development build (`cargo tauri dev` or `cargo tauri build`) creates a new ad-hoc signature. This signature invalidates the Accessibility permission that you granted before.

**Symptoms:** After a rebuild, the paste fails and the overlay shows "Paste failed". The text is on the clipboard.

**Workaround:**

1. Reset the TCC database entry:
   ```bash
   tccutil reset Accessibility com.sottoasr.app
   ```
2. Open **System Settings** > **Privacy & Security** > **Accessibility**
3. If SottoASR is listed, remove it (select and click `-`)
4. Click `+` and navigate to the newly built `SottoASR.app`
5. Relaunch SottoASR via `open` (not the raw binary)

**Tip:** **Settings → Advanced → Permissions** shows the Accessibility status. "Restart needed" means that macOS reports the permission but the functional check fails.

### Must Launch via `open` Command

**Problem:** If you run the binary directly (for example `./src-tauri/target/release/sottoasr`), macOS launch services are not used. Then `LSUIElement` (hide from Dock) has no effect, and the TCC permission checks do not work correctly.

**Fix:** Always launch via:
```bash
open src-tauri/target/release/bundle/macos/SottoASR.app
```

### Development Build Shares Data with the Installed App

The development build and an installed SottoASR use the same data folder and the same global shortcuts. Quit the installed app before you start a development build or the interactive part of `scripts/pre-release-check.sh`.

### Speech Model Download and Upgrade

At first launch, onboarding downloads Parakeet Ultra (about 630 MB), verifies each file against the bundled SHA-256 manifest and shows the progress. If model files are missing at a later launch, or a newly downloaded file failed verification, the download runs again. A dictation that stops while the model downloads or loads fails at once with "Speech model is still loading; your recording was kept." If loading failed, the message gives the load error and says that the recording was kept.

An installation that already has the verified Parakeet TDT v3 files does not show onboarding. It starts and dictates with Parakeet TDT v3 as before. Parakeet Ultra then downloads in the background and is verified. When no dictation is in progress, the app replaces Parakeet TDT v3 with it in memory. Settings shows "Ready" and a notice during this time. If the download fails, for example offline, Parakeet TDT v3 stays in use and the next launch tries again.

If CoreML cannot load the verified Parakeet Ultra files and Parakeet TDT v3 is on disk, the app uses Parakeet TDT v3 and Settings shows a notice. Nothing deletes the old folders `parakeet-tdt-0.6b-v3` and `parakeet-tdt-0.6b-v3-coreml`. You can delete both by hand once Settings shows Parakeet Ultra. An older app version still uses its `parakeet-tdt-0.6b-v3` folder and ignores `parakeet-ultra`.

### Shortcut Recorder Key Listener

The shortcut recorder uses a native key listener (a CGEventTap). A comment in `commands/keycapture.rs` says that macOS Sequoia requires the Input Monitoring permission for this tap. If the listener cannot start, the recorder uses the keyboard events of the Settings window.

### Registered Commands Without a Frontend Caller

`apply_shortcuts`, `fix_accessibility_permission` and `open_input_monitoring_settings` are registered in `lib.rs`, but no frontend code calls them. `src/lib/utils/tauri.test.ts` lists them as known exceptions.

### Tauri Version

Tauri stays on 2.11. The fixed `zip` crate needs `tauri-plugin-updater` 2.14, which needs Tauri 2.12. Until that upgrade, `src-tauri/.cargo/audit.toml` ignores the six `zip` advisories.

## Debugging

### Log Output

The log file is `~/Library/Logs/com.sottoasr.app/SottoASR.log`. It rotates at 2 MB and keeps 5 files. In development mode, the same lines also go to the terminal. The log level is fixed at Info. `RUST_LOG` has no effect.

The frontend sends `console.warn` and `console.error` to the same log. **Copy Diagnostics** in the menu bar menu copies the app version, the macOS version and the last 100 log lines, with your home folder shown as `~`.

### Frontend DevTools

In a debug build (`cargo tauri dev`), right-click a SottoASR window and select **Inspect Element**. This opens the WebView developer tools. Release builds do not enable the developer tools.

### Checking Permissions

**Settings → Advanced → Permissions** shows the Microphone and Accessibility status. `scripts/pre-release-check.sh` also reads the user TCC database with these queries:

```bash
sqlite3 "$HOME/Library/Application Support/com.apple.TCC/TCC.db" \
  "SELECT auth_value FROM access WHERE service='kTCCServiceAccessibility' AND client='com.sottoasr.app';"
sqlite3 "$HOME/Library/Application Support/com.apple.TCC/TCC.db" \
  "SELECT auth_value FROM access WHERE service='kTCCServiceMicrophone' AND client='com.sottoasr.app';"
```

A value of `2` means allowed and `0` means denied. If the query cannot read the database, the script shows a warning.

### Common Issues

| Symptom | Likely Cause | Fix |
|---|---|---|
| "Paste failed" after a rebuild | Accessibility permission lost | Re-add SottoASR in Accessibility settings |
| App appears in Dock | Launched via raw binary | Launch via `open SottoASR.app` |
| "Speech model is still loading; your recording was kept." | The model downloads or loads | Wait for the model, then reprocess the recording from History |
| "SottoASR does not have Microphone access..." | Microphone permission denied | Grant it in System Settings > Privacy & Security > Microphone |
| "The microphone sent only silence..." | Permission off, or the microphone is muted | Grant access, or examine the microphone |
| "The microphone ... is not connected" | The selected microphone is absent | Connect it, or select another microphone in Settings → Dictation |
| Settings opens with "Review Keyboard Shortcuts" at launch | A saved shortcut was not registered | Change the shortcut in Settings → General |

### Useful Paths

| What | Path |
|---|---|
| Speech and vocabulary models | `~/Library/Application Support/FluidAudio/Models/` |
| App data (`settings.json`, `transcriptions.json`, `overlay_positions.json`) | `~/Library/Application Support/com.sottoasr.app/` |
| Recordings | `~/Library/Application Support/com.sottoasr.app/recordings/` |
| Cleanup runtime and standalone Python | `~/Library/Application Support/com.sottoasr.app/llm-venv/` and `python/` |
| Cleanup model | `~/.cache/huggingface/hub/models--openbmb--MiniCPM5-2B-MLX/` |
| Log file | `~/Library/Logs/com.sottoasr.app/SottoASR.log` |
| CSV exports | `~/Downloads/SottoASR-transcriptions-*.csv` |
| Build output | `src-tauri/target/release/bundle/macos/SottoASR.app` |
| DMG output | `src-tauri/target/release/bundle/dmg/` |

## Contributing

### Development Conventions

- **Rust:** `snake_case` for functions/variables, `PascalCase` for types, `SCREAMING_SNAKE_CASE` for constants
- **TypeScript/Svelte:** `camelCase` for functions/variables, `PascalCase` for types and components, `kebab-case` for utility files
- **Commits:** Use [Conventional Commits](https://www.conventionalcommits.org/) -- `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`
- **Branches:** `feature/<slug>` for features, `fix/<slug>` for bug fixes
- **Changelog:** add user-facing entries under `## [Unreleased]` in `CHANGELOG.md`

### Workflow

1. Create a feature branch from `main`
2. Make changes, keeping commits atomic and well-described
3. Run the checks (`./scripts/pre-release-check.sh --auto-only` runs all of them)
4. Open a pull request with a clear description
5. Address review feedback in new commits (do not force-push during review)
6. Squash-merge once approved

### Key Rules

- **All audio processing happens in Rust.** Never add audio handling to the frontend.
- **The app must remain menu-bar-only.** No Dock icon, no main window.
- **Local-only processing is a hard constraint.** Never add features that send audio, transcriptions, or usage data to external services.
- **Never use `npx`.** Use `npm run <script>` or invoke tools directly.
- **Components call the wrappers in `src/lib/utils/tauri.ts`.** Do not call `invoke` directly.

For the complete design document, see [docs/designs/architecture.md](docs/designs/architecture.md).
