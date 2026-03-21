# Development Guide

This document covers everything needed to develop, build, test, and debug Sotto.

## Table of Contents

- [Prerequisites](#prerequisites)
- [Environment Setup](#environment-setup)
- [Project Structure](#project-structure)
- [Running in Development](#running-in-development)
- [Building for Production](#building-for-production)
- [Testing and Linting](#testing-and-linting)
- [Feature Flags](#feature-flags)
- [Architecture Overview](#architecture-overview)
- [Known Issues and Workarounds](#known-issues-and-workarounds)
- [Debugging](#debugging)
- [Contributing](#contributing)

## Prerequisites

| Tool | Minimum Version | Tested With | Install |
|---|---|---|---|
| Rust | 1.77 | 1.94 | [rustup.rs](https://rustup.rs/) |
| Node.js | 20 | 25.4 | [nodejs.org](https://nodejs.org/) |
| Xcode CLI Tools | -- | -- | `xcode-select --install` |
| macOS | 14 Sonoma | -- | -- |
| Apple Silicon | M1+ | M4 | -- |

Xcode Command Line Tools are required for the Swift compiler, which FluidAudio's bridge code depends on.

## Environment Setup

```bash
# 1. Install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"

# 2. Install Xcode Command Line Tools (if not already installed)
xcode-select --install

# 3. Clone the repository
git clone https://github.com/juanqui/sotto.git
cd sotto

# 4. Install frontend dependencies
npm install

# 5. Verify the setup
cargo tauri dev
```

On first launch, FluidAudio will download the CoreML model (~500 MB) to `~/Library/Application Support/FluidAudio/Models/`. This takes 1--2 minutes and only happens once.

## Project Structure

```
sotto/
├── src/                              # Svelte 5 frontend
│   ├── lib/
│   │   ├── components/               # UI components
│   │   │   ├── overlay-pill.svelte       # Main recording overlay
│   │   │   ├── waveform.svelte           # Canvas-based waveform visualization
│   │   │   ├── recording-timer.svelte    # Elapsed time display
│   │   │   ├── history-view.svelte       # Transcription history list
│   │   │   ├── history-item.svelte       # Single history entry
│   │   │   ├── settings-panel.svelte     # Settings UI
│   │   │   └── onboarding-view.svelte    # First-launch setup
│   │   ├── stores/                   # Svelte 5 rune-based stores (.svelte.ts)
│   │   │   ├── recording.svelte.ts       # Recording state (active, RMS level)
│   │   │   ├── transcriptions.svelte.ts  # Transcription history
│   │   │   └── settings.svelte.ts        # User settings
│   │   └── utils/
│   │       ├── tauri.ts                  # Typed IPC wrapper functions
│   │       └── format.ts                 # Formatting utilities
│   ├── overlay.ts                    # Overlay window entry point
│   ├── history.ts                    # History window entry point
│   ├── settings.ts                   # Settings window entry point
│   ├── onboarding.ts                 # Onboarding window entry point
│   └── app.css                       # Global dark theme styles
├── *.html                            # Vite multi-page HTML entries (root level)
├── src-tauri/                        # Rust backend
│   ├── src/
│   │   ├── main.rs                       # Entry point
│   │   ├── lib.rs                        # App setup, plugin registration, lifecycle
│   │   ├── state.rs                      # AppState (Mutex/AtomicBool fields)
│   │   ├── models.rs                     # Data structs: Transcription, Settings, ModelStatus
│   │   ├── commands/                     # Tauri IPC command handlers
│   │   │   ├── recording.rs                  # start/stop/cancel recording
│   │   │   ├── transcription.rs              # History CRUD operations
│   │   │   ├── settings.rs                   # Settings read/write
│   │   │   ├── permissions.rs                # macOS permission checks (AXIsProcessTrusted)
│   │   │   └── setup.rs                      # Onboarding, ASR init, model download
│   │   ├── audio/
│   │   │   ├── capture.rs                    # cpal microphone capture with RMS metering
│   │   │   └── resample.rs                   # rubato resampling to 16 kHz
│   │   ├── asr/
│   │   │   ├── engine.rs                     # AsrEngine trait + create_engine() factory
│   │   │   ├── fluidaudio_backend.rs         # FluidAudio CoreML/ANE backend
│   │   │   ├── parakeet_backend.rs           # parakeet-rs ONNX backend
│   │   │   └── model.rs                      # Model status tracking, download (parakeet)
│   │   ├── paste/
│   │   │   └── macos.rs                      # CGEvent-based Cmd+V simulation
│   │   ├── hotkeys/
│   │   │   └── manager.rs                    # Global shortcut registration + recording flow
│   │   └── tray/
│   │       └── menu.rs                       # System tray icon and context menu
│   ├── Cargo.toml                    # Rust dependencies and feature flags
│   ├── tauri.conf.json               # Tauri app configuration
│   ├── capabilities/default.json     # Tauri v2 capability permissions
│   ├── Info.plist                    # LSUIElement (no Dock icon), mic usage description
│   └── Entitlements.plist            # macOS entitlements
├── docs/designs/architecture.md      # Architecture design document
├── .claude/                          # Claude Code steering rules
└── package.json                      # Frontend dependencies
```

### Multi-Window Architecture

Sotto uses Tauri's multi-window support with Vite multi-page entries. Each window has its own HTML entry point at the project root and a corresponding TypeScript entry in `src/`:

- **Overlay** -- floating pill shown during recording
- **History** -- transcription history browser
- **Settings** -- configuration panel
- **Onboarding** -- first-launch setup wizard

## Running in Development

### Full Application (Recommended)

```bash
cargo tauri dev
```

This starts the Rust backend and the Vite dev server with hot reload for the frontend. Rust changes trigger a recompile; Svelte/TypeScript changes hot-reload instantly.

### Frontend Only

```bash
npm run dev
```

Useful for iterating on UI without waiting for Rust compilation. IPC calls to the backend will fail since Tauri is not running.

### Rust Only

```bash
cd src-tauri
cargo build
```

Builds the Rust backend without the frontend. Useful for testing backend changes in isolation.

## Building for Production

```bash
cargo tauri build
```

This produces:
- `src-tauri/target/release/bundle/macos/Sotto.app` -- the application bundle
- `src-tauri/target/release/bundle/dmg/Sotto_<version>_aarch64.dmg` -- disk image installer

**Important:** You must launch the built app via `open src-tauri/target/release/bundle/macos/Sotto.app`, not by running the raw binary directly. The `open` command is required for `LSUIElement` (no Dock icon) and TCC permission checks to work correctly.

## Testing and Linting

### Rust

```bash
cd src-tauri

# Run tests
cargo test

# Run linter (treats warnings as errors)
cargo clippy -- -D warnings
```

### Frontend

```bash
# Type checking
npm run check

# Production build (also validates)
npm run build
```

### Pre-PR Checklist

Run all checks and capture output for review:

```bash
cd src-tauri
cargo build 2>&1 | tee /tmp/cargo-build.txt
cargo clippy -- -D warnings 2>&1 | tee /tmp/cargo-clippy.txt
cargo test 2>&1 | tee /tmp/cargo-test.txt
cd ..
npm run build 2>&1 | tee /tmp/npm-build.txt
```

## Feature Flags

Sotto supports multiple ASR backends via Cargo feature flags, defined in `src-tauri/Cargo.toml`.

| Flag | Backend | Platform | Default | Description |
|---|---|---|---|---|
| `asr-fluidaudio` | FluidAudio (CoreML/ANE) | macOS only | Yes | Uses Apple Neural Engine for fast, efficient inference |
| `asr-parakeet` | parakeet-rs (ONNX Runtime) | Cross-platform | No | CPU-based inference via ONNX Runtime |

### Switching ASR Backends

**Default (FluidAudio):**

```bash
cargo tauri dev
# or
cargo tauri build
```

**parakeet-rs:**

```bash
cargo tauri dev --no-default-features --features custom-protocol,asr-parakeet
# or
cargo tauri build --no-default-features --features custom-protocol,asr-parakeet
```

Note: `custom-protocol` is a Tauri feature required for the app to function and must always be included when overriding default features.

### How the Backend is Selected

The `asr/engine.rs` module defines the `AsrEngine` trait and a `create_engine()` factory function. At compile time, the active feature flag determines which backend implementation is compiled:

- `asr-fluidaudio` compiles `fluidaudio_backend.rs`
- `asr-parakeet` compiles `parakeet_backend.rs`

Both implement the same `AsrEngine` trait, so the rest of the codebase is backend-agnostic.

## Architecture Overview

### Rust Backend

The backend is organized into focused modules:

- **`commands/`** -- Tauri IPC handlers invoked by the frontend. Each file groups related commands (recording, transcription history, settings, permissions, setup).
- **`audio/`** -- Audio capture via cpal and resampling to 16 kHz via rubato. The capture module also computes RMS levels for the frontend waveform.
- **`asr/`** -- Speech recognition. The `AsrEngine` trait abstracts over backends. The factory function instantiates the correct backend based on compile-time feature flags.
- **`paste/`** -- Paste-at-cursor using macOS CGEvent APIs to simulate `Cmd+V`.
- **`hotkeys/`** -- Global shortcut registration and the recording flow (start capture, wait for release/toggle, run ASR, paste result).
- **`tray/`** -- System tray icon and context menu (Copy Last, View History, Settings, Quit).
- **`state.rs`** -- Centralized `AppState` struct with `Mutex` and `AtomicBool` fields for thread-safe shared state.
- **`models.rs`** -- Data structures (`Transcription`, `Settings`, `ModelStatus`) shared across modules.

### Svelte Frontend

The frontend uses Svelte 5 with rune-based reactivity (`.svelte.ts` stores):

- **Components** are in `src/lib/components/`. The overlay pill, waveform, and timer are the most performance-sensitive (canvas rendering at 60fps).
- **Stores** in `src/lib/stores/` use Svelte 5's `$state` and `$derived` runes for reactive state management.
- **IPC wrappers** in `src/lib/utils/tauri.ts` provide typed functions for calling Rust commands, keeping Tauri API usage centralized.

### Data Flow

```
User presses hotkey
  → hotkeys/manager.rs registers the event
    → audio/capture.rs starts cpal stream, sends RMS levels to frontend
      → Frontend renders waveform in overlay-pill.svelte
    → User releases key (or presses toggle again)
      → audio/capture.rs stops, audio buffer ready
        → audio/resample.rs converts to 16 kHz mono
          → asr/engine.rs runs inference (FluidAudio or parakeet-rs)
            → Result text copied to clipboard
              → paste/macos.rs simulates Cmd+V
                → Text appears at cursor
```

## Known Issues and Workarounds

### Accessibility Permission Invalidation (Development)

**Problem:** Accessibility permission in macOS is tied to the application's code signature. Every development build (`cargo tauri dev` or `cargo tauri build`) creates a new ad-hoc signature, which invalidates the previously granted Accessibility permission. Paste-at-cursor silently fails.

**Symptoms:** After a rebuild, transcription completes but no text appears at your cursor. No error is shown.

**Workaround:**

1. Reset the TCC database entry:
   ```bash
   tccutil reset Accessibility com.sotto.app
   ```
2. Open **System Settings** > **Privacy & Security** > **Accessibility**
3. If Sotto is listed, remove it (select and click `-`)
4. Click `+` and navigate to the newly built `Sotto.app`
5. Relaunch Sotto via `open` (not the raw binary)

**Tip:** You can verify Accessibility status from within the app -- the `permissions.rs` module calls `AXIsProcessTrusted()` and reports the result.

### Must Launch via `open` Command

**Problem:** Running the Sotto binary directly (e.g., `./src-tauri/target/release/sotto`) bypasses macOS launch services. This causes `LSUIElement` (hide from Dock) to not take effect, and TCC permission checks to behave incorrectly.

**Fix:** Always launch via:
```bash
open src-tauri/target/release/bundle/macos/Sotto.app
```

### FluidAudio Model Download on First Launch

**Problem:** The first call to `init_asr()` triggers a ~500 MB model download, which takes 1--2 minutes. The app may appear unresponsive during this time.

**Location:** Models are cached at `~/Library/Application Support/FluidAudio/Models/`.

**Workaround:** The onboarding flow handles this gracefully with a progress indicator. If you skip onboarding during development, the download still happens on the first recording attempt.

## Debugging

### Log Output

In development mode (`cargo tauri dev`), Rust logs are printed to the terminal. Look for output from the `sotto` and `fluidaudio` modules.

To increase log verbosity:

```bash
RUST_LOG=debug cargo tauri dev
```

For even more detail:

```bash
RUST_LOG=sotto=trace,fluidaudio=debug cargo tauri dev
```

### Frontend DevTools

In development mode, right-click any Sotto window and select **Inspect Element** to open the WebView developer tools. This gives access to the browser console, network tab, and element inspector.

### Checking Permissions

To verify microphone and accessibility permissions from the command line:

```bash
# Check if Sotto has microphone access (look for com.sotto.app)
sqlite3 ~/Library/Application\ Support/com.apple.TCC/TCC.db \
  "SELECT service, client, allowed FROM access WHERE client = 'com.sotto.app';"

# Check accessibility (programmatic)
# The app itself calls AXIsProcessTrusted() -- check terminal output
```

### Common Issues

| Symptom | Likely Cause | Fix |
|---|---|---|
| Text not pasted after recording | Accessibility permission lost | Re-add Sotto in Accessibility settings |
| App appears in Dock | Launched via raw binary | Launch via `open Sotto.app` |
| "Model not found" error | First launch, model not downloaded | Wait for download or check network |
| Overlay doesn't appear | Window creation failed | Check terminal for Tauri errors |
| No audio captured | Microphone permission denied | Grant in System Settings > Microphone |

### Useful Paths

| What | Path |
|---|---|
| FluidAudio models | `~/Library/Application Support/FluidAudio/Models/` |
| App data | `~/Library/Application Support/com.sotto.app/` |
| macOS TCC database | `~/Library/Application Support/com.apple.TCC/TCC.db` |
| Build output | `src-tauri/target/release/bundle/macos/Sotto.app` |
| DMG output | `src-tauri/target/release/bundle/dmg/` |

## Contributing

### Development Conventions

- **Rust:** `snake_case` for functions/variables, `PascalCase` for types, `SCREAMING_SNAKE_CASE` for constants
- **TypeScript/Svelte:** `camelCase` for functions/variables, `PascalCase` for types and components, `kebab-case` for utility files
- **Commits:** Use [Conventional Commits](https://www.conventionalcommits.org/) -- `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`
- **Branches:** `feature/<slug>` for features, `fix/<slug>` for bug fixes

### Workflow

1. Create a feature branch from `main`
2. Make changes, keeping commits atomic and well-described
3. Run the full check suite (build, clippy, test, frontend build)
4. Open a pull request with a clear description
5. Address review feedback in new commits (do not force-push during review)
6. Squash-merge once approved

### Key Rules

- **All audio processing happens in Rust.** Never add audio handling to the frontend.
- **The app must remain menu-bar-only.** No Dock icon, no main window.
- **Local-only processing is a hard constraint.** Never add features that send audio, transcriptions, or usage data to external services.
- **Never use `npx`.** Use `npm run <script>` or invoke tools directly.

For the complete design document, see [docs/designs/architecture.md](docs/designs/architecture.md).
