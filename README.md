# SottoASR

**Local-first, privacy-first speech-to-text for macOS.**

Press a hotkey, speak, and text appears at your cursor. By default, speech recognition runs locally through CoreML, optional cleanup uses MLX on the GPU, and your recordings stay on this Mac. Two optional cloud engines, [AWS Transcribe](#aws-transcribe-optional) for speech and [AWS Bedrock](#aws-bedrock-optional) for AI cleanup, work only after you turn them on.

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
- **Recording recovery** -- an interrupted dictation keeps its audio, and History offers to reprocess a recording of 15 seconds or more
- **Personal vocabulary** -- recognize saved words using audio evidence, plus exact spelling replacements. Works with AI off.
- **Optional AI cleanup** -- remove clear fillers and accidental repeats. By default it runs on this Mac with MiniCPM5 2B. The original stays in History. Off by default.
- **Onboarding flow** -- guided first-launch setup with automatic model download and progress
- **Dark and light appearance** -- follows the macOS setting
- **Local by default** -- powered by FluidAudio (CoreML/Apple Neural Engine) with the Parakeet Ultra model
- **Optional cloud engines** -- [AWS Transcribe](#aws-transcribe-optional) for speech and [AWS Bedrock](#aws-bedrock-optional) for AI cleanup, with your own AWS account, for Macs with little free memory. Both are off by default.

## Requirements

- **macOS 14 Sonoma** or later (FluidAudio requires macOS 14+)
- **Apple Silicon** (M1 or later) -- required for Neural Engine acceleration
- About 630 MB of disk space for the speech model, which onboarding downloads automatically
- Optional acoustic vocabulary: about 103 MB, prepared when you save your first words
- Optional AI cleanup on this Mac: about 1.4 GB for the cleanup model, plus its Python runtime. You do not need to install Python. If no native arm64 Python 3.11 to 3.14 is installed, SottoASR downloads its own copy.
- Optional AWS Transcribe or AWS Bedrock: an AWS account and an internet connection
- Microphone permission
- Accessibility permission (for paste-at-cursor)

## Quick Start

### Prerequisites

- [Rust](https://rustup.rs/) 1.94.1+ (CI uses 1.96.0)
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

On first launch, onboarding downloads the speech model, Parakeet Ultra (about 630 MB), and verifies each file. The model stays in `~/Library/Application Support/FluidAudio/Models/`. The setup time depends on your network and on CoreML compilation.

An installation that used Parakeet TDT v3 keeps dictating with it while Parakeet Ultra downloads in the background, then switches automatically. The old `parakeet-tdt-0.6b-v3` and `parakeet-tdt-0.6b-v3-coreml` folders stay in that location. Once Parakeet Ultra is in use, Settings offers to delete them; SottoASR never deletes them without your confirmation.

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
| `Escape` | -- | Cancel the current recording. Nothing is pasted. With the local engine, History keeps the transcript, marked as cancelled. With AWS Transcribe, SottoASR does not send the recording, and History keeps a cancelled entry without text. |
| `Cmd+Shift+,` | -- | Open Settings |

You can change each shortcut in **Settings → General**. Push to talk, toggle and cancel can also have an alternate shortcut. SottoASR registers the cancel shortcut only while it records, so `Escape` works normally in other apps.

## How It Works

SottoASR is a Tauri v2 application with a Rust backend and a Svelte 5 frontend.

```
Hotkey pressed
  → cpal captures the selected or default microphone
    → (AI cleanup on this Mac) warm the resident cleanup model with fixed text
    → Disk worker writes a private WAV file and checkpoints it about once per second
Stop
  → Parakeet Ultra recognizes the full recording through CoreML (or Amazon Transcribe, if you selected it)
    → Optional acoustic vocabulary checks saved words against the audio
      → Exact replacements apply once
        → Optional cleanup processes bounded text windows in batches of up to 16 (on this Mac, or with Amazon Bedrock if you selected it)
          → Text is pasted at the cursor, or copied
            → History saves the transcript, then the WAV file is deleted
```

The frontend provides the recording overlay, transcription history, settings, onboarding, and the about and update windows. All audio capture and ASR inference happens in the backend. The frontend never touches audio data.

SottoASR pastes before it saves History, so a slow history write does not delay the text. If the app that was in front at the start is still in front, SottoASR pastes without activating it again. If you switched apps during the recording, the text goes to the app that is in front at the end. If the app that is to receive the paste is SottoASR itself, SottoASR copies the text instead.

In **Settings → Vocabulary → Words you use**, add a correctly spelled name such as **Qwen** and save. Sotto prepares a separate local acoustic model, then compares candidates against the recording. This does not retrain Parakeet or guarantee ambiguous homophones. The tested guard preserves uncertain cases, and most Quinn/Qwen homophones still need an exact replacement. Version numbers and punctuation currently need a full alias under **Exact replacements**.

Exact replacements match whole words without cascading, and protect URLs, email addresses, and backtick code. History retains the original ASR text for Raw/Diff views. [Vocabulary research](docs/research/2026-09-08-vocabulary-asr.md) documents how this differs from VoiceInk, which currently supplies vocabulary to AI enhancement.

The speech model is [Parakeet Ultra](https://huggingface.co/moondream/parakeet-ultra), moondream's post-training of NVIDIA Parakeet TDT v3 with the same architecture (600M parameters) and the same 25 languages. On an Apple M2 Pro on 2026-11-19, it had a word error rate of 3.80% on the full LibriSpeech test-other set, against 4.12% for Parakeet TDT v3. In that run, Ultra had a higher peak memory (644 MiB against 487 MiB) and took 13% longer in total. On SottoASR's 88 synthetic clips, the two models were equal within noise. The [Parakeet Ultra research note](docs/research/2026-11-19-parakeet-ultra.md) gives the method and the limits.

On 2026-09-08, on an M4/32 GiB Mac, the same Parakeet TDT v3 weights produced identical text in four CoreML compute configurations. CPU+ANE had a 62.70 ms warm median on short synthetic clips, versus about 138 ms for CPU-only or a GPU encoder. These exclude capture, cleanup and paste. [Model measurements](benchmarks/asr/README.md) and [UI measurements](docs/research/2026-09-08-settings-performance.md) include reproduction steps and limits. No wattage was measured.

### AI Cleanup

By default, AI cleanup runs on this Mac. In **Settings → Dictation**, turn on **AI transcript cleanup**. SottoASR then prepares the local runtime and model, and shows the download progress. Then click **Save** to turn cleanup on. The model is the stock `openbmb/MiniCPM5-2B-MLX`, pinned to one revision and verified by SHA-256. The runtime packages come from a bundled lock file with a SHA-256 hash for each package. To use Amazon Bedrock in place of the local model, see [AWS Bedrock (optional)](#aws-bedrock-optional).

The cleanup model, MiniCPM5 2B or the model in Amazon Bedrock, proposes filler and accidental-repeat removals. Rust validates the edits and rebuilds the accepted text from the original. Cleanup stops after 60 seconds, or after the first runtime failure, and the rest stays as spoken. Accepted cleanup is used for paste and Copy. The original remains in expanded History. Cancelled or interrupted recordings skip cleanup.

Cleanup is off by default. A failed, rejected, or incomplete window leaves its region unchanged while validated edits elsewhere still apply. Quotes, code, identifiers, dictionary replacements, and vocabulary terms are protected. Reliable non-English detections skip cleanup. Ambiguous fillers and word-choice errors can remain. These safeguards do not prove semantic correctness. With the local engine, startup and recording-start warmups use fixed text, not your speech. With AWS Bedrock, SottoASR sends no warmup. [Bundled smoke tests](benchmarks/llm/release-smoke/README.md) describe the cleanup checks.

If the local engine reports a runtime problem, click **Repair cleanup runtime** at the top of Settings. A repair builds a new runtime beside the old one and replaces the old one only after the new one passes its check.

## AWS Transcribe (optional)

SottoASR transcribes on this Mac by default. If your Mac has little free memory, or you prefer a cloud service, you can use Amazon Transcribe instead. In this mode, SottoASR loads no speech model and no vocabulary model, and it sends the audio of each completed dictation to AWS with your own AWS account. On an M2 Pro, the system used about 600 MB less memory with AWS Transcribe than with Parakeet Ultra loaded and in use. AWS Transcribe is off until you turn it on.

### Setup

1. Create or pick an AWS profile in `~/.aws/config` or `~/.aws/credentials`. Profiles with static keys, IAM Identity Center or `credential_process` work. A `credential_process` command needs its full path, because an app that Finder starts does not have the `PATH` of your shell.
2. Give the profile the IAM permission `transcribe:StartStreamTranscription`.
3. Open **Settings → Dictation**. Under **Speech recognition**, select **AWS Transcribe**.
4. Select the **AWS profile**, the **Region** and the **Language**. If you have a custom vocabulary in your AWS account, type its name in **Custom vocabulary**. It must be for the same Region and language.
5. Click **Test connection**. SottoASR sends one second of silence to Amazon Transcribe and shows the result.
6. Click **Save**.

To go back, select **On this Mac** and click **Save**. SottoASR then loads Parakeet Ultra again. If the model is not on disk, SottoASR downloads it first.

### What SottoASR sends

- After a recording ends, SottoASR sends the audio of that dictation as 16 kHz 16-bit PCM over TLS. The destination is the Amazon Transcribe streaming endpoint of the selected Region (`transcribestreaming.<region>.amazonaws.com`).
- When you reprocess a kept recording in History, SottoASR sends that recording.
- **Test connection** sends one second of silence.
- SottoASR does not send a cancelled recording. It deletes the recording, and History keeps a cancelled entry without text.

SottoASR stores no AWS credentials. The AWS SDK for Rust reads your AWS files when it signs a request. [SECURITY.md](SECURITY.md) lists each network request.

### Cost and data use

- AWS bills your account for each second of audio. See [Amazon Transcribe pricing](https://aws.amazon.com/transcribe/pricing/).
- By default, AWS can keep the audio to improve its services. An AWS Organizations opt-out policy stops that. See [the opt-out page of AWS](https://docs.aws.amazon.com/transcribe/latest/dg/opt-out.html).

### Limits

- One language. SottoASR does not use the automatic language identification of Amazon Transcribe.
- The wait after a dictation is about a quarter of the length of the dictation, plus about 0.6 s. On a 150 s recording, Amazon Transcribe worked at 4.3 times real time.
- Vocabulary assistance (**Words you use**) does not run, because it needs the local model. Exact replacements still run on this Mac. AI cleanup runs with the cleanup engine that you select. For names and terms, use a custom vocabulary in your AWS account.
- AWS Transcribe needs an internet connection. If a request fails, SottoASR shows the error and keeps the recording. If the recording is 15 seconds or longer, you can reprocess it from History. For a shorter one, say the words again.
- A new installation starts with the local engine, and its setup window downloads Parakeet Ultra (about 630 MB). You can select AWS Transcribe after the setup.

## AWS Bedrock (optional)

SottoASR cleans up text on this Mac by default. If your Mac has little free memory, or you prefer a cloud model, you can use Amazon Bedrock instead. In this mode, SottoASR starts no cleanup sidecar and loads no cleanup model. It sends the parts of a dictation text that cleanup can change to a model in Amazon Bedrock, with your own AWS account. In a test from `us-west-2`, one request took a median of about 0.7 s with Claude Haiku 5.5, Claude Sonnet 5.5 or GPT-6.2 Sol. With Claude Opus 5.5, it took about 1.5 s. AWS Bedrock is off until you turn it on.

### Setup

1. Create or pick an AWS profile in `~/.aws/config` or `~/.aws/credentials`. Profiles with static keys, IAM Identity Center or `credential_process` work, as for AWS Transcribe.
2. Give the profile the IAM permission `bedrock:InvokeModel` on the inference profile and on the foundation model of the model that you want. For the foundation model, the permission must cover each destination Region of the inference profile. Make sure that your AWS account has access to the model in Amazon Bedrock.
3. Open **Settings → Dictation**. Under **Transcript cleanup**, select **AWS Bedrock**.
4. Select the **AWS profile**, the **Region** and the **Model**. Settings then shows the inference profile and where AWS processes the text.
5. Click **Test connection**. SottoASR sends one fixed sentence to Amazon Bedrock and shows the result.
6. If **AI transcript cleanup** is off, turn it on. With AWS Bedrock, SottoASR downloads no cleanup model.
7. Click **Save**.

To go back, select **On this Mac** and click **Save**. If AI cleanup is on, SottoASR first prepares the local model, and downloads it if it is not on disk. While AWS Bedrock is in use, you can remove the local cleanup model in **Settings → Advanced → Local models**.

### What SottoASR sends

- After a dictation ends, SottoASR sends the parts of its text that cleanup can change, over TLS. Each part is a window of at most 100 words. The destination is the Amazon Bedrock runtime endpoint of the selected Region (`bedrock-runtime.<region>.amazonaws.com`).
- When you reprocess a kept recording in History, SottoASR sends the parts of its text in the same way.
- **Test connection** sends the fixed sentence "Please um keep this readiness check short."
- SottoASR sends nothing for a text with nothing to clean, a text in another language, or a cancelled or interrupted recording. It sends no warmup request.
- SottoASR never sends audio, History, your vocabulary words or your replacement rules.

The inference profile decides where AWS processes the text. SottoASR uses the profile of the geography of your Region when AWS has one for the model. Then the request stays inside that geography. Otherwise SottoASR uses the global profile, and AWS can process the text in any commercial AWS Region. Settings shows which profile it uses before you save.

| Geography | Regions | Models with a profile for the geography |
|---|---|---|
| United States (`us.`) | `us-east-1`, `us-east-2`, `us-west-1`, `us-west-2` | All four |
| Canada (`ca.`) | `ca-central-1` | Claude Haiku 5.5, Claude Sonnet 5.5 |
| European Union (`eu.`) | `eu-central-1`, `eu-north-1`, `eu-west-1`, `eu-west-2`, `eu-west-3` | Claude Haiku 5.5, Claude Sonnet 5.5, Claude Opus 5.5 |
| Japan (`jp.`) | `ap-northeast-1`, `ap-northeast-3` | Claude Haiku 5.5, Claude Opus 5.5 |
| Australia (`au.`) | `ap-southeast-2` | Claude Haiku 5.5, Claude Sonnet 5.5, Claude Opus 5.5 |

Each other combination of Region and model uses the global profile (`global.`). In `ap-northeast-2`, `ap-south-1`, `ap-southeast-1` and `sa-east-1`, all four models use the global profile. The AWS page [cross-Region inference](https://docs.aws.amazon.com/bedrock/latest/userguide/cross-region-inference.html) explains the profiles.

SottoASR stores no AWS credentials. The AWS SDK for Rust reads your AWS files when it signs a request. [SECURITY.md](SECURITY.md) lists each network request.

### Cost and data use

- AWS bills your account for the input and output tokens. One request has about 950 input tokens with a Claude model, and about 580 with GPT-6.2 Sol. See [Amazon Bedrock pricing](https://aws.amazon.com/bedrock/pricing/).
- Amazon Bedrock does not give your text to the model provider. See [how Amazon Bedrock protects your data](https://docs.aws.amazon.com/bedrock/latest/userguide/data-protection.html).
- For GPT-6.2 Sol, AWS can keep text that its abuse classifier flags for up to 30 days, for automated abuse detection. The three Claude models are not in the list of models with this retention. See [abuse detection](https://docs.aws.amazon.com/bedrock/latest/userguide/abuse-detection.html) and [data retention](https://docs.aws.amazon.com/bedrock/latest/userguide/data-retention.html).

### Limits

- AI cleanup is for English, as with the local engine. If SottoASR detects another language, it skips cleanup and sends nothing to AWS.
- SottoASR examines each change that the model proposes, and it can only remove words. If the model rewrites or adds words, SottoASR rejects the proposal, and the text stays as you said it.
- The model list is fixed: Claude Haiku 5.5, Claude Sonnet 5.5, Claude Opus 5.5 and GPT-6.2 Sol. You cannot select another model or your own inference profile.
- AWS Bedrock needs an internet connection. If the connection, the credentials or the permission fail, SottoASR pastes the dictation without cleanup. After you sign in again, the next dictation uses the new credentials. If one request fails or takes more than 10 s, only its part of the text stays as you said it.

## Settings

Settings has four tabs. A status area above the tabs shows the speech model and AI cleanup.

| Tab | Contents |
|---|---|
| General | Keyboard shortcuts, **Launch at login**, **Check for updates** |
| Dictation | **Speech recognition**, **Microphone**, **Sound cues**, **AI transcript cleanup** with its **Cleanup engine**, **Paste automatically**, **Restore clipboard**, **Return to the original app**, **Show recording overlay** |
| Vocabulary | **Words you use** (up to 100) and **Exact replacements** (up to 200) |
| Advanced | **Keep transcripts** and **Keep recordings**, permission status, local models, and the AI cleanup strategy |

SottoASR validates settings when you click **Save**. If a value is not valid, nothing is saved and Settings shows the problem. If the settings file cannot be read, Settings offers **Back up and reset**. This keeps the unreadable file as a backup and starts again with the default settings.

The retention options are **Keep until I delete** (the default), 1, 7, 30, 90 or 365 days. SottoASR applies them at launch, every 6 hours, and when you save a change. **Keep recordings** also deletes recordings that wait for recovery.

If a speech or vocabulary model file differs from the verified release, dictation continues to work and Settings shows a warning. Click **Re-download speech model** to replace the files. Dictation continues with the loaded model while the files download.

## Recording Recovery

If a dictation fails or is interrupted, or SottoASR exits during one, History lists the recording for recovery. At launch, SottoASR opens History when recordings wait for recovery. Each row shows the date, the duration and the audio path.

History lists only recordings of 15 seconds or more: to say a short dictation again is faster than to recover it. A shorter recording stays in the recording folder, and the retention setting applies to it. The error message of a failed dictation has a **Show audio** button for its file.

1. Click **Show in Finder** to locate a recording.
2. Click **Reprocess** to transcribe it with the selected speech engine and save the result in History. With AWS Transcribe, this sends the recording to AWS. With AI cleanup on and AWS Bedrock selected, this also sends the parts of the text that cleanup can change to Amazon Bedrock.
3. Copy the recovered transcript when you are ready. Recovery never pastes automatically.
4. To remove a recording that you do not need, click **Delete audio**.

New recordings use private WAV files in `~/Library/Application Support/com.sottoasr.app/recordings/`. The disk worker checkpoints the WAV header and synchronizes audio about once per second. A sudden crash can lose audio after the last checkpoint. Disk stalls can extend that interval. A full capture queue stops recording and reports an error instead of silently dropping speech.

SottoASR deletes the WAV of a successful dictation only after History saves it. Failed or interrupted operations keep the audio. If the microphone is disconnected or stops sending audio, SottoASR marks the transcript as interrupted, keeps the audio, and pastes nothing. Reprocessed audio remains in the recording folder, and a receipt prevents repeated recovery prompts. Closing History postpones recovery without deleting recordings.

If you quit while SottoASR records or transcribes, a warning asks you to wait. **Quit Anyway** finishes the recording file first. If the recording is 15 seconds or longer, the next launch offers it for recovery.

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

By default, speech recognition and cleanup run on this Mac, and SottoASR sends no audio or transcript text over the network. If you turn on [AWS Transcribe](#aws-transcribe-optional), SottoASR sends the audio of each completed dictation to Amazon Transcribe. If you turn on [AWS Bedrock](#aws-bedrock-optional), SottoASR sends the parts of a dictation text that cleanup can change to Amazon Bedrock. Neither engine sends History. SottoASR also connects to GitHub for app updates and for the optional standalone Python, to Hugging Face for models, and to PyPI for the cleanup runtime. [SECURITY.md](SECURITY.md) lists each request and when it happens.

## Tech Stack

| Layer | Technology |
|---|---|
| Desktop framework | [Tauri v2](https://v2.tauri.app/) (Rust backend) |
| Frontend | [Svelte 5](https://svelte.dev/) + TypeScript |
| ASR engine (default) | [FluidAudio](https://github.com/FluidInference/FluidAudio) via the [vendored ASR bridge](src-tauri/vendor/fluidaudio-rs/README.md) (CoreML / Apple Neural Engine) |
| ASR engine (optional) | [parakeet-rs](https://github.com/altunenes/parakeet-rs) (ONNX Runtime, cross-platform) |
| Cloud speech engine (optional) | [AWS SDK for Rust](https://github.com/awslabs/aws-sdk-rust) (optional Amazon Transcribe engine) |
| AI cleanup engine (default) | Python sidecar with [MLX](https://github.com/ml-explore/mlx) and mlx-lm |
| Cloud cleanup engine (optional) | [AWS SDK for Rust](https://github.com/awslabs/aws-sdk-rust) (optional Amazon Bedrock engine) |
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
