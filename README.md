# SottoASR

**Local, privacy-first speech-to-text for macOS.**

Press a hotkey, speak, and text appears at your cursor. Speech recognition runs locally through CoreML, and optional cleanup uses MLX on the GPU. Your recordings stay on this Mac.

<!-- ![SottoASR screenshot](docs/images/screenshot.png) -->

## Features

- **Two dictation modes** -- push-to-talk (hold `Cmd+Shift+Space`) or toggle (`Cmd+Shift+D`) for longer sessions
- **Paste anywhere** -- transcribed text is inserted at your cursor in any application
- **Menu bar app** -- lives in your system tray, no Dock icon, invisible when idle
- **Recording overlay** -- floating pill with real-time canvas waveform, pulsing indicator, and timer
- **Transcription history** -- browse, copy, and manage past transcriptions
- **Personal vocabulary** -- recognize saved words using audio evidence, plus exact spelling replacements; works with AI off
- **Optional AI cleanup** -- remove clear fillers and accidental repeats locally with MiniCPM; keep the original in History; disabled by default
- **Organized Settings** -- General, Dictation, Vocabulary, and Advanced, with automatic local model setup
- **Onboarding flow** -- guided first-launch setup with automatic model download
- **Responsive UI** -- paged history with full search; no idle waveform animation
- **Fully local** -- powered by FluidAudio (CoreML/Apple Neural Engine) with the Parakeet TDT v3 model

## Requirements

- **macOS 14 Sonoma** or later (FluidAudio requires macOS 14+)
- **Apple Silicon** (M1 or later) -- required for Neural Engine acceleration
- ~500 MB disk space for the ASR model (downloaded automatically on first launch)
- Optional AI cleanup: Python 3.11+, about 227 MB for the model; enabling handles runtime/model setup and stays off until you save
- Optional acoustic vocabulary: about 103 MB, prepared when you save your first words
- Microphone permission
- Accessibility permission (for paste-at-cursor)

## Quick Start

### Prerequisites

- [Rust](https://rustup.rs/) 1.80+ (tested with 1.94)
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

On first launch, SottoASR will download the FluidAudio CoreML model (~500 MB). It is cached for subsequent runs; initial setup time depends on the network and CoreML compilation.

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
    → (AI cleanup enabled) warm the resident cleanup model with fixed text
    → Disk worker writes private WAV checkpoints during capture and finalizes at Stop
      → Parakeet TDT v3 recognizes speech through CoreML (full recording)
        → Optional acoustic vocabulary checks saved words against audio
          → Exact replacements apply once
            → Optional cleanup processes bounded text windows in batches of 16
              → Validated transcript saved in history and pasted or copied
```

The frontend provides the recording overlay (floating pill with canvas-based waveform visualization), transcription history, settings panel, and onboarding flow. All audio capture and ASR inference happens entirely in the Rust backend -- the frontend never touches audio data.

In **Settings → Vocabulary → Words you use**, add a correctly spelled name such as **Qwen** and save. Sotto prepares a separate local acoustic model, then compares candidates against the recording. This does not retrain Parakeet or guarantee ambiguous homophones. The tested guard preserves uncertain cases; most Quinn/Qwen homophones still need an exact replacement. Version numbers and punctuation currently need a full alias under **Exact replacements**.

Exact replacements match whole words without cascading, and protect URLs, email addresses, and backtick code. History retains the original ASR text for Raw/Diff views. [Vocabulary research](docs/research/2026-09-08-vocabulary-asr.md) documents how this differs from VoiceInk, which currently supplies vocabulary to AI enhancement.

On this M4/32 GiB Mac, the same Parakeet weights produced identical text in four CoreML compute configurations. CPU+ANE had a 62.70 ms warm median on short synthetic clips, versus about 138 ms for CPU-only or a GPU encoder. These exclude capture, cleanup and paste. Newer ASR alternatives did not improve the tested accuracy/resource tradeoff. [Model measurements](benchmarks/asr/README.md) and [UI measurements](docs/research/2026-09-08-settings-performance.md) include reproduction steps and limits; no wattage was measured.

In **Settings → Dictation**, enabling AI cleanup prepares the local runtime and model. Save activates the preference. MiniCPM5 2B proposes filler and accidental-repeat removals. After Stop, the full recording passes through ASR and vocabulary processing. Cleanup then processes bounded text windows in batches of up to 16. Rust validates the edits and reconstructs accepted text from the original. Startup and recording-start warmups use fixed text, not your speech. Accepted cleanup is used for paste and Copy. The original remains in expanded History. Cancelled or interrupted recordings skip cleanup.

Cleanup is off by default. A failed, rejected, or incomplete window leaves its region unchanged while validated edits elsewhere still apply. Quotes, code, identifiers, dictionary replacements, and vocabulary terms are protected. Reliable non-English detections skip cleanup. Ambiguous fillers and word-choice errors can remain. These safeguards do not prove semantic correctness. Long recordings still require processing after Stop; this release does not promise tail-only transcription. The earlier live-window cache was replaced by the [batched stop-path design](docs/specs/2026-09-13-batched-stop-path-correction.md). [Bundled smoke tests](benchmarks/llm/release-smoke/README.md) describe the cleanup checks. Old History suggestions remain readable.

Version 0.11.0 speeds up acoustic vocabulary processing with typed CoreML tensor access and explicit silence padding. The speech model, chunk boundaries, and acceptance thresholds stay unchanged. The retained [latency measurements](benchmarks/asr/latency-2026-09-15.json) distinguish tested improvements from unqualified streaming alternatives.

## Recording Recovery

If SottoASR exits unexpectedly, the next launch opens History with a recovery notice. Pending recordings appear with their audio paths and durations.

1. Click **Show in Finder** to locate a recording.
2. Click **Reprocess** to transcribe it locally and save the result in History.
3. Copy the recovered transcript when you are ready. Recovery never pastes automatically.

New recordings use private WAV files in `~/Library/Application Support/com.sottoasr.app/recordings/`. The disk worker checkpoints the WAV header and synchronizes audio about once per second. A sudden crash can lose audio after the last checkpoint. Disk stalls can extend that interval. A full capture queue stops recording and reports an error instead of silently dropping speech.

Successful ordinary dictation removes its WAV only after History saves. Failed or interrupted operations keep the audio. Recovery copies older `sotto_<UUID>.wav` files from the current user temporary directory without changing the originals. Reprocessed audio remains in the recording directory; an acknowledgement prevents repeated recovery prompts. Closing History postpones recovery without deleting recordings.

A recovery notice indicates an unclean exit, not a confirmed diagnosis. Empty or unreadable WAV files show an error and remain available in Finder. Recordings removed by the operating system before recovery cannot be restored.


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
| ASR engine (default) | [FluidAudio](https://github.com/FluidInference/FluidAudio) via the [vendored ASR bridge](src-tauri/vendor/fluidaudio-rs/README.md) (CoreML / Apple Neural Engine) |
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
