# SottoASR

**Local, privacy-first speech-to-text for macOS.**

Press a hotkey, speak, and text appears at your cursor. Speech recognition runs locally through CoreML, and optional cleanup uses MLX on the GPU. Your recordings stay on this Mac.

<!-- ![SottoASR screenshot](docs/images/screenshot.png) -->

## Features

- **Two dictation modes** -- push-to-talk (hold `Cmd+Shift+Space`) or toggle (`Cmd+Shift+D`) for longer sessions
- **Paste anywhere** -- transcribed text is inserted at your cursor in any application, or copied if you turn off automatic paste
- **Clipboard restore** -- after a paste, SottoASR puts back what you had copied, including images and files
- **Menu bar app** -- lives in your system tray, no Dock icon, invisible when idle
- **Recording overlay** -- floating pill with a real-time waveform and a timer
- **Microphone selection** -- use the system default input or select a microphone in Settings
- **Optional sound cues** -- short system sounds for start, stop, cancel, busy and error (off by default)
- **Transcription history** -- browse, search, copy, export and delete past transcriptions, with optional retention periods
- **Recording recovery** -- an interrupted dictation keeps its audio, and History offers to reprocess it
- **Personal vocabulary** -- recognize saved words using audio evidence, plus exact spelling replacements. Works with AI off.
- **Optional AI cleanup** -- remove clear fillers and accidental repeats locally with MiniCPM5 2B. The original stays in History. Off by default.
- **Onboarding flow** -- guided first-launch setup with automatic model download and progress
- **Dark and light appearance** -- follows the macOS setting
- **Fully local** -- powered by FluidAudio (CoreML/Apple Neural Engine) with the Parakeet TDT v3 model

## Requirements

- **macOS 14 Sonoma** or later (FluidAudio requires macOS 14+)
- **Apple Silicon** (M1 or later) -- required for Neural Engine acceleration
- About 500 MB of disk space for the speech model, which onboarding downloads automatically
- Optional acoustic vocabulary: about 103 MB, prepared when you save your first words
- Optional AI cleanup: about 1.4 GB for the cleanup model, plus its Python runtime. You do not need to install Python. If no native arm64 Python 3.11 to 3.14 is installed, SottoASR downloads its own copy.
- Microphone permission
- Accessibility permission (for paste-at-cursor)

## Quick Start

### Prerequisites

- [Rust](https://rustup.rs/) 1.80+ (CI uses 1.94)
- [Node.js](https://nodejs.org/) 20.19+ or 22.12+ (CI uses 24)
- Xcode or the Xcode Command Line Tools (`xcode-select --install`), for the Swift compiler
- Tauri CLI: `cargo install tauri-cli --version "^2"`

### Install and Run

```bash
git clone https://github.com/juanqui/sottoasr.git
cd sottoasr
npm install
cargo tauri dev
```

On first launch, onboarding downloads the speech model (about 500 MB) and verifies each file. The model stays in `~/Library/Application Support/FluidAudio/Models/`. The setup time depends on your network and on CoreML compilation.

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
| `Escape` | -- | Cancel the current recording. Nothing is pasted. History keeps the transcript, marked as cancelled. |
| `Cmd+Shift+,` | -- | Open Settings |

You can change each shortcut in **Settings → General**. Push to talk, toggle and cancel can also have an alternate shortcut. SottoASR registers the cancel shortcut only while it records, so `Escape` works normally in other apps.

## How It Works

SottoASR is a Tauri v2 application with a Rust backend and a Svelte 5 frontend.

```
Hotkey pressed
  → cpal captures the selected or default microphone
    → (AI cleanup on) warm the resident cleanup model with fixed text
    → Disk worker writes a private WAV file and checkpoints it about once per second
Stop
  → Parakeet TDT v3 recognizes the full recording through CoreML
    → Optional acoustic vocabulary checks saved words against the audio
      → Exact replacements apply once
        → Optional cleanup processes bounded text windows in batches of up to 16
          → Text is pasted at the cursor, or copied
            → History saves the transcript, then the WAV file is deleted
```

The frontend provides the recording overlay, transcription history, settings, onboarding, and the about and update windows. All audio capture and ASR inference happens in the backend. The frontend never touches audio data.

SottoASR pastes before it saves History, so a slow history write does not delay the text. If the app that was in front at the start is still in front, SottoASR pastes without activating it again. If you switched apps during the recording, the text goes to the app that is in front at the end. If the app that is to receive the paste is SottoASR itself, SottoASR copies the text instead.

In **Settings → Vocabulary → Words you use**, add a correctly spelled name such as **Qwen** and save. Sotto prepares a separate local acoustic model, then compares candidates against the recording. This does not retrain Parakeet or guarantee ambiguous homophones. The tested guard preserves uncertain cases, and most Quinn/Qwen homophones still need an exact replacement. Version numbers and punctuation currently need a full alias under **Exact replacements**.

Exact replacements match whole words without cascading, and protect URLs, email addresses, and backtick code. History retains the original ASR text for Raw/Diff views. [Vocabulary research](docs/research/2026-09-08-vocabulary-asr.md) documents how this differs from VoiceInk, which currently supplies vocabulary to AI enhancement.

On this M4/32 GiB Mac, the same Parakeet weights produced identical text in four CoreML compute configurations. CPU+ANE had a 62.70 ms warm median on short synthetic clips, versus about 138 ms for CPU-only or a GPU encoder. These exclude capture, cleanup and paste. Newer ASR alternatives did not improve the tested accuracy/resource tradeoff. [Model measurements](benchmarks/asr/README.md) and [UI measurements](docs/research/2026-09-08-settings-performance.md) include reproduction steps and limits. No wattage was measured.

### AI Cleanup

In **Settings → Dictation**, turn on **AI transcript cleanup**. SottoASR then prepares the local runtime and model, and shows the download progress. Then click **Save** to turn cleanup on. The model is the stock `openbmb/MiniCPM5-2B-MLX`, pinned to one revision and verified by SHA-256. The runtime packages come from a bundled lock file with a SHA-256 hash for each package.

MiniCPM5 2B proposes filler and accidental-repeat removals. Rust validates the edits and rebuilds the accepted text from the original. Cleanup stops after 60 seconds, or after the first runtime failure, and the rest stays as spoken. Accepted cleanup is used for paste and Copy. The original remains in expanded History. Cancelled or interrupted recordings skip cleanup.

Cleanup is off by default. A failed, rejected, or incomplete window leaves its region unchanged while validated edits elsewhere still apply. Quotes, code, identifiers, dictionary replacements, and vocabulary terms are protected. Reliable non-English detections skip cleanup. Ambiguous fillers and word-choice errors can remain. These safeguards do not prove semantic correctness. Startup and recording-start warmups use fixed text, not your speech. [Bundled smoke tests](benchmarks/llm/release-smoke/README.md) describe the cleanup checks.

If cleanup reports a runtime problem, click **Repair cleanup runtime** at the top of Settings. A repair builds a new runtime beside the old one and replaces the old one only after the new one passes its check.

## Settings

Settings has four tabs. A status area above the tabs shows the speech model and AI cleanup.

| Tab | Contents |
|---|---|
| General | Keyboard shortcuts, **Launch at login**, **Check for updates** |
| Dictation | **Microphone**, **Sound cues**, **AI transcript cleanup**, **Paste automatically**, **Restore clipboard**, **Return to the original app**, **Show recording overlay** |
| Vocabulary | **Words you use** (up to 100) and **Exact replacements** (up to 200) |
| Advanced | **Keep transcripts** and **Keep recordings**, permission status, local models, and the AI cleanup strategy |

SottoASR validates settings when you click **Save**. If a value is not valid, nothing is saved and Settings shows the problem. If the settings file cannot be read, Settings offers **Back up and reset**. This keeps the unreadable file as a backup and starts again with the default settings.

The retention options are **Keep until I delete** (the default), 1, 7, 30, 90 or 365 days. SottoASR applies them at launch, every 6 hours, and when you save a change. **Keep recordings** also deletes recordings that wait for recovery.

If a speech or vocabulary model file differs from the verified release, dictation continues to work and Settings shows a warning. Click **Re-download speech model** to replace the files. Dictation continues with the loaded model while the files download.

## Recording Recovery

If a dictation fails or is interrupted, or SottoASR exits during one, History lists the recording for recovery. At launch, SottoASR opens History when recordings wait for recovery. Each row shows the date, the duration and the audio path.

1. Click **Show in Finder** to locate a recording.
2. Click **Reprocess** to transcribe it locally and save the result in History.
3. Copy the recovered transcript when you are ready. Recovery never pastes automatically.
4. To remove a recording that you do not need, click **Delete audio**.

New recordings use private WAV files in `~/Library/Application Support/com.sottoasr.app/recordings/`. The disk worker checkpoints the WAV header and synchronizes audio about once per second. A sudden crash can lose audio after the last checkpoint. Disk stalls can extend that interval. A full capture queue stops recording and reports an error instead of silently dropping speech.

SottoASR deletes the WAV of a successful dictation only after History saves it. Failed or interrupted operations keep the audio. If the microphone is disconnected or stops sending audio, SottoASR marks the transcript as interrupted, keeps the audio, and pastes nothing. Reprocessed audio remains in the recording folder, and a receipt prevents repeated recovery prompts. Closing History postpones recovery without deleting recordings.

If you quit while SottoASR records or transcribes, a warning asks you to wait. **Quit Anyway** finishes the recording file first, and the next launch offers it for recovery.

When you delete a history entry or clear History, SottoASR also deletes the audio kept for those entries. Recovery also copies older `sotto_<UUID>.wav` files from the current user temporary folder without changing the originals.

A recovery notice indicates an unclean exit, not a confirmed diagnosis. Empty or unreadable WAV files show an error and remain available in Finder. Recordings removed by the operating system before recovery cannot be restored.

## Permissions

SottoASR requires two macOS permissions:

### Microphone

Onboarding asks for it. If you skip onboarding, macOS asks the first time you start a recording. If access is denied, SottoASR shows an error that names the System Settings pane to use.

### Accessibility

Required for paste-at-cursor (simulated `Cmd+V`). Onboarding asks macOS to show its Accessibility prompt. **Settings → Advanced → Permissions** shows the status and opens the prompt again. You can also add the app manually:

1. Open **System Settings** > **Privacy & Security** > **Accessibility**
2. Click the **+** button
3. Navigate to and select `SottoASR.app`

Restart SottoASR after you grant access.

> **Note for developers:** Accessibility permission is tied to the app's code signature. Each development build creates a new ad-hoc signature, which invalidates the previous permission grant. Thus you must remove and re-add SottoASR in Accessibility settings after each rebuild. See [DEVELOPMENT.md](DEVELOPMENT.md) for workarounds.

## Privacy

Speech recognition and cleanup run on this Mac. SottoASR never sends audio or transcript text over the network. It connects to GitHub for app updates and for the optional standalone Python, to Hugging Face for models, and to PyPI for the cleanup runtime. [SECURITY.md](SECURITY.md) lists each request and when it happens.

## Tech Stack

| Layer | Technology |
|---|---|
| Desktop framework | [Tauri v2](https://v2.tauri.app/) (Rust backend) |
| Frontend | [Svelte 5](https://svelte.dev/) + TypeScript |
| ASR engine (default) | [FluidAudio](https://github.com/FluidInference/FluidAudio) via the [vendored ASR bridge](src-tauri/vendor/fluidaudio-rs/README.md) (CoreML / Apple Neural Engine) |
| ASR engine (optional) | [parakeet-rs](https://github.com/altunenes/parakeet-rs) (ONNX Runtime, cross-platform) |
| AI cleanup (optional) | Python sidecar with [MLX](https://github.com/ml-explore/mlx) and mlx-lm |
| Audio capture | [cpal](https://github.com/RustAudioGroup/cpal) 0.16 |
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

The `asr-parakeet` backend also downloads its model from a pinned Hugging Face commit and verifies the SHA-256 of each file.

## Architecture

For the full design document, see [docs/designs/architecture.md](docs/designs/architecture.md).

For development setup, project structure, debugging tips, and known issues, see [DEVELOPMENT.md](DEVELOPMENT.md).

## License

MIT
