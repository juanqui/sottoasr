# Project: SottoASR

## Overview

SottoASR is a local-first, privacy-first speech-to-text application for macOS. The user presses a global hotkey and speaks. SottoASR then pastes the transcribed text at the cursor. By default, all speech recognition and transcript cleanup runs on the Mac, and no audio or text leaves it. Two optional cloud engines work only after the user selects them in Settings. AWS Transcribe sends the audio of each completed dictation to Amazon Transcribe. AWS Bedrock sends the parts of a dictation text that cleanup can change to Amazon Bedrock.

## Tech Stack

| Layer | Technology | Purpose |
|-------|-----------|---------|
| App framework | Tauri v2 (2.11) | Native app shell, system tray, IPC |
| Backend | Rust | Audio capture, the dictation pipeline, clipboard and paste, hotkeys, history |
| Frontend | Svelte 5 + TypeScript | Recording overlay, history, settings, onboarding, about and update windows |
| ASR engine (default) | FluidAudio via the vendored bridge `src-tauri/vendor/fluidaudio-rs` (Swift) | Parakeet Ultra on CoreML and the Apple Neural Engine (macOS) |
| ASR engine (optional) | parakeet-rs (ONNX Runtime) | Cross-platform fallback via the `asr-parakeet` feature |
| Cloud speech engine (optional) | AWS SDK for Rust (`aws-config`, `aws-sdk-transcribestreaming`) | Optional Amazon Transcribe engine, off by default. The SDK reads the AWS profiles of the user. |
| Transcript cleanup (optional) | Python sidecar `src-tauri/sidecar/llm_cleanup.py` with MLX and mlx-lm | The default cleanup engine. Runs the stock `openbmb/MiniCPM5-2B-MLX` model, pinned by revision |
| Cloud cleanup engine (optional) | AWS SDK for Rust (`aws-sdk-bedrockruntime`) | Optional Amazon Bedrock engine for AI cleanup, off by default. It uses the same AWS profiles and shared code (`src-tauri/src/aws/`) as AWS Transcribe. |
| Audio capture | cpal 0.16 | Microphone input |
| Build tool | Vite 8 | Multi-page frontend bundling |

## Architecture

- **Menu bar / tray app only.** No Dock icon. No main window by default. The app lives in the macOS menu bar.
- **Floating overlay window** appears during recording to show visual feedback (waveform, status). It is a non-activating NSPanel, so it does not take focus from the app that receives the text.
- **Dictation pipeline:** microphone → cpal → private WAV file on disk → FluidAudio (Parakeet Ultra), or Amazon Transcribe in cloud mode → optional acoustic vocabulary (local engine only) → exact replacements → optional cleanup (the local sidecar, or Amazon Bedrock in Amazon Bedrock mode) → paste or copy → history. `src-tauri/src/pipeline.rs` is the only implementation of start, stop and cancel.
- **All audio and ASR processing happens in the backend.** The frontend is only for UI. It never touches audio data or model inference. Rust owns the pipeline. Rust calls the Swift bridge for local ASR, sends the request to Amazon Transcribe in cloud mode, starts the Python sidecar for local cleanup, and sends the cleanup requests to Amazon Bedrock in Amazon Bedrock mode.
- **One speech engine object at a time.** The `speech_engine` setting selects the engine. In cloud mode no local speech model and no vocabulary model is in memory.
- **One cleanup backend at a time.** The `cleanup_engine` setting selects the backend, and `llm::engine::ensure_running` is the only place that makes one. In Amazon Bedrock mode no sidecar starts and no cleanup model is in memory. Both backends only propose edits. The same validator decides which deletions apply.

## Package Managers

- **Frontend (npm):** `package.json` in project root
- **Backend (cargo):** `Cargo.toml` in `src-tauri/`
- **Cleanup runtime (pip):** `src-tauri/sidecar/requirements.lock`, a hash-locked file generated from `requirements.in`. The app installs it into its own virtual environment. The regeneration command is at the top of `requirements.in`.

## Commands

```bash
# Full app development (frontend + backend)
cargo tauri dev

# Production build
cargo tauri build

# Rust only (run from src-tauri/)
cargo build                                    # Build Rust backend
cargo clippy --all-targets -- -D warnings      # Lint Rust code
cargo test                                     # Run Rust tests
cargo check --no-default-features --features custom-protocol,asr-parakeet   # Check the ONNX backend

# Frontend only
npm run dev                    # Svelte dev server
npm run build                  # Frontend production build
npm run check                  # TypeScript/Svelte type checking
npm test                       # Vitest tests

# Python tests and configuration checks (run from the project root)
python3 -m unittest discover -s src-tauri/sidecar -p 'test_*.py'
python3 -m unittest discover -s scripts -p 'test_*.py'
bash scripts/ci-checks.sh
```

## Key Directories

```
sottoasr/
├── *.html                # Vite page entries, one per window
├── src/                  # Svelte 5 frontend
│   ├── *.ts              # Window entry scripts
│   └── lib/              # Components, rune stores (.svelte.ts) and utilities
├── src-tauri/            # Tauri + Rust backend
│   ├── src/              # Rust source code
│   ├── sidecar/          # Python cleanup sidecar (mlx-lm) and its lock file
│   ├── vendor/fluidaudio-rs/   # Swift ASR bridge, SDK patches, model manifest
│   ├── capabilities/     # Tauri permissions per window
│   ├── Cargo.toml        # Rust dependencies
│   └── tauri.conf.json   # Tauri configuration
├── scripts/              # CI checks, release checks, Swift and model probes
├── benchmarks/           # ASR, cleanup (llm) and UI benchmarks
├── training/             # Earlier fine-tuning work (historical, not used by the app)
├── website/              # sottoasr.app static site
├── docs/                 # Documentation
│   ├── specs/            # Feature specs (date-prefixed, immutable)
│   ├── research/         # Research notes (date-prefixed)
│   ├── journals/         # Experiment logs (date-prefixed)
│   ├── audit/            # Code/architecture audits (date-prefixed)
│   ├── designs/          # Living design docs
│   └── templates/        # HuggingFace model card templates
└── .claude/              # Claude Code rules and configuration
```

## Project-Specific Rules

- **NEVER use `npx` for project commands.** Use `npm run <script>` or invoke the CLI tool directly. `npx` can silently install wrong versions or different packages.
- **NEVER modify ASR model paths or download locations without explicit user consent.** Model files are large and their location matters for the user's disk usage and privacy.
- **All audio processing happens in Rust, not in the frontend.** If you need to add audio-related functionality, it goes in `src-tauri/src/`, not in `src/`.
- **The app MUST remain menu-bar-only.** No Dock icon, no main window by default. Any UI must be a floating overlay or a settings panel opened from the tray menu.
- **Accessibility permissions are required** for the paste-at-cursor functionality (simulated Cmd+V). Code that interacts with accessibility APIs must handle the case where permissions have not been granted and guide the user to enable them.
- **Local by default.** Audio or text leaves the Mac only through a feature that the user turned on, and that the interface names. No telemetry. Never add a feature that sends audio, transcriptions or usage data to an external service without these two conditions.
- **Experiment logs go in `docs/journals/`**, not alongside code. Benchmark code lives in `benchmarks/`; the narrative of what was tried, measured, and learned lives in `docs/journals/YYYY-MM-DD-slug.md`.

## HuggingFace Artifacts (historical)

The shipped app does not use the artifacts in this section. Its cleanup model is the stock `openbmb/MiniCPM5-2B-MLX`, pinned by revision in `src-tauri/src/llm/engine.rs` (`SOTTO_MODEL`, `MODEL_REVISION`) and in `src-tauri/sidecar/llm_cleanup.py`. The fine-tuned LFM2.5 models and their dataset come from earlier work. `training/` keeps the scripts and data of that work as history. The facts, the protocol and the checklist below apply only if you upload to these repositories again.

| Artifact | Repo | Visibility |
|----------|------|------------|
| **Training dataset** | [`juanquivilla/sotto-transcript-cleanup`](https://huggingface.co/datasets/juanquivilla/sotto-transcript-cleanup) | Public |
| **Fine-tuned model (bf16)** | [`juanquivilla/sotto-cleanup-lfm25-350m`](https://huggingface.co/juanquivilla/sotto-cleanup-lfm25-350m) | Public |
| **MLX 5-bit** | [`juanquivilla/sotto-cleanup-lfm25-350m-mlx-5bit`](https://huggingface.co/juanquivilla/sotto-cleanup-lfm25-350m-mlx-5bit) | Public |
| **MLX 4-bit** | [`juanquivilla/sotto-cleanup-lfm25-350m-mlx-4bit`](https://huggingface.co/juanquivilla/sotto-cleanup-lfm25-350m-mlx-4bit) | Public |

- **HF token** is stored in `.env` (gitignored) as `HF_TOKEN`. The remote training machine at `juanqui@192.168.1.128` has a **read-only** cached token; use the write token from `.env` for uploads.
- **Naming convention:** `juanquivilla/sotto-{purpose}-{base_model}-{size}` for models; `juanquivilla/sotto-{purpose}` for datasets
- **Base model:** `LiquidAI/LFM2.5-350M-Base` — full fine-tuned (no LoRA), all 354M params trainable
- **Best result of that work:** v22+GRPO — ROUGE-L 0.954 (val set), 66% Exact Match, 91% Filler-Free. Pipeline: cleaned data (text fixes + 6K new) → SFT LR 3e-5, β2=0.95 → GRPO LoRA r=32, LR 3e-6
- **MLX quantized models** of that work:
  - 5-bit affine, group_size=64 (~237MB, minimal quality loss)
  - 4-bit affine, group_size=64 — smaller (~195MB, slightly lower quality)
- **Quantization recipe:** `mlx_lm.convert --hf-path <model> --mlx-path <out> -q --q-bits 5 --q-group-size 64 --trust-remote-code`

### HuggingFace Upload Protocol (CRITICAL)

This protocol is historical. The shipped app does not use these models. It still applies to every new upload to the repositories in the table above.

**Every model upload MUST include a detailed model card (README.md).** Use the templates in `docs/templates/` as the starting point:
- `docs/templates/hf-model-card-bf16.md` — Full precision model card template
- `docs/templates/hf-model-card-mlx-5bit.md` — MLX 5-bit model card template
- `docs/templates/hf-model-card-mlx-4bit.md` — MLX 4-bit model card template

Model cards MUST include:
1. Navigation links at top: sottoasr.app, all variant repos, training dataset
2. Overview describing the model and linking to [SottoASR](https://sottoasr.app)
3. Key specs table with sizes, ROUGE-L, Exact Match, Filler-Free, Latency
4. **"What It Does" examples table** showing input/output pairs (at least 4-6 examples)
5. Full per-category benchmark results table
6. Comparison with prompted 2B baseline (bf16 card only)
7. Usage code snippet (transformers for bf16, mlx_lm for MLX variants)
8. Training details (bf16 card) or quantization recipe (MLX cards)
9. "All Variants" table linking to all three repos
10. Links section with sottoasr.app, GitHub, dataset

**The website URL is `sottoasr.app` (NOT sotto.app).**

**Upload checklist (run in order):**
1. Copy template from `docs/templates/`, fill in benchmark numbers
2. Upload bf16 model + model card to `sotto-cleanup-lfm25-350m`
3. Generate MLX 5-bit quant locally (macOS only): `mlx_lm.convert ...`
4. Generate MLX 4-bit quant locally
5. Upload both MLX variants with variant-specific model cards
6. Verify all three repos have correct README.md with cross-links and examples
7. Update this section with latest benchmark numbers

**NEVER delete or overwrite a model card without replacing it.** If uploading a new model version, the commit message MUST include the version identifier and key metric (e.g., "v15: ROUGE-L 0.960").
