# SottoASR

**Local, privacy-first speech-to-text for macOS.**

Press a hotkey, speak, and text appears at your cursor. All processing happens on-device using Apple's Neural Engine -- no audio ever leaves your machine.

<!-- ![SottoASR screenshot](docs/images/screenshot.png) -->

## Features

- **Two dictation modes** -- push-to-talk (hold `Cmd+Shift+Space`) or toggle (`Cmd+Shift+D`) for longer sessions
- **Paste anywhere** -- transcribed text is inserted at your cursor in any application
- **Menu bar app** -- lives in your system tray, no Dock icon, invisible when idle
- **Recording overlay** -- floating pill with real-time canvas waveform, pulsing indicator, and timer
- **Transcription history** -- browse, copy, and manage past transcriptions
- **Settings panel** -- configure hotkeys, audio input, and behavior
- **Onboarding flow** -- guided first-launch setup with automatic model download
- **Fast** -- ~44x real-time transcription speed on Apple M4
- **Lightweight** -- 14 MB app bundle, minimal memory when idle
- **Fully local** -- powered by FluidAudio (CoreML/Apple Neural Engine) with the Parakeet TDT v3 model

## Requirements

- **macOS 14 Sonoma** or later (FluidAudio requires macOS 14+)
- **Apple Silicon** (M1 or later) -- required for Neural Engine acceleration
- ~500 MB disk space for the ASR model (downloaded automatically on first launch)
- Microphone permission
- Accessibility permission (for paste-at-cursor)

## Quick Start

### Prerequisites

- [Rust](https://rustup.rs/) 1.77+ (tested with 1.94)
- [Node.js](https://nodejs.org/) 20+ (tested with 25.4)
- Xcode Command Line Tools: `xcode-select --install`
- Tauri CLI: `cargo install tauri-cli --version "^2"`

### Install and Run

```bash
git clone https://github.com/juanqui/sottoasr.git
cd sotto
npm install
cargo tauri dev
```

On first launch, SottoASR will download the FluidAudio CoreML model (~500 MB). This takes 1--2 minutes and is cached for subsequent runs.

### Build for Production

```bash
cargo tauri build
```

This produces `SottoASR.app` and a `.dmg` installer in `src-tauri/target/release/bundle/`.

## Default Hotkeys

| Shortcut | Mode | Behavior |
|---|---|---|
| `Cmd+Shift+Space` | Push-to-talk | Hold to record, release to transcribe and paste |
| `Cmd+Shift+D` | Toggle | Press to start recording, press again to stop and paste |
| `Escape` | -- | Cancel current recording |

## How It Works

SottoASR is a Tauri v2 application with a Rust backend and a Svelte 5 frontend.

```
Hotkey pressed
  → cpal captures microphone audio
    → Audio resampled to 16 kHz
      → FluidAudio (CoreML/ANE) runs Parakeet TDT inference
        → Transcribed text copied to clipboard
          → CGEvent simulates Cmd+V paste at cursor
```

The frontend provides the recording overlay (floating pill with canvas-based waveform visualization), transcription history, settings panel, and onboarding flow. All audio capture and ASR inference happens entirely in the Rust backend -- the frontend never touches audio data.

## Permissions

SottoASR requires two macOS permissions:

### Microphone

Prompted automatically the first time you start a recording. Grant access when the system dialog appears.

### Accessibility

Required for paste-at-cursor (simulated `Cmd+V`). Must be added manually:

1. Open **System Settings** > **Privacy & Security** > **Accessibility**
2. Click the **+** button
3. Navigate to and select `SottoASR.app`

> **Note for developers:** Accessibility permission is tied to the app's code signature. Each development build creates a new ad-hoc signature, which invalidates the previous permission grant. You will need to remove and re-add SottoASR in Accessibility settings after each rebuild. See [DEVELOPMENT.md](DEVELOPMENT.md) for workarounds.

## Tech Stack

| Layer | Technology |
|---|---|
| Desktop framework | [Tauri v2](https://v2.tauri.app/) (Rust backend) |
| Frontend | [Svelte 5](https://svelte.dev/) + TypeScript |
| ASR engine (default) | [FluidAudio](https://github.com/fluidaudio) via fluidaudio-rs (CoreML / Apple Neural Engine) |
| ASR engine (optional) | [parakeet-rs](https://github.com/altunenes/parakeet-rs) (ONNX Runtime, cross-platform) |
| Audio capture | [cpal](https://github.com/RustAudioGroup/cpal) 0.15 |
| Build tool | [Vite](https://vitejs.dev/) 8 |

## Cross-Platform Support

SottoASR defaults to FluidAudio, which uses CoreML and Apple's Neural Engine for maximum performance on macOS. For future cross-platform support, an alternative backend is available via feature flags:

| Feature Flag | Backend | Platform | Notes |
|---|---|---|---|
| `asr-fluidaudio` (default) | FluidAudio CoreML/ANE | macOS only | Best performance on Apple Silicon |
| `asr-parakeet` | parakeet-rs ONNX Runtime | Cross-platform | CPU-based, no hardware dependency |

To build with the parakeet-rs backend:

```bash
cargo tauri build --no-default-features --features custom-protocol,asr-parakeet
```

## Architecture

For the full design document, see [docs/designs/architecture.md](docs/designs/architecture.md).

For development setup, project structure, debugging tips, and known issues, see [DEVELOPMENT.md](DEVELOPMENT.md).

## License

MIT
