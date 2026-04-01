# Project: SottoASR

## Overview

SottoASR is a local, privacy-first speech-to-text application for macOS. The user presses a global hotkey, speaks, and transcribed text is pasted at their cursor position. All processing happens on-device — no audio or text is ever sent to a cloud service.

## Tech Stack

| Layer | Technology | Purpose |
|-------|-----------|---------|
| App framework | Tauri v2 | Native app shell, system tray, IPC |
| Backend | Rust | Audio capture, ASR processing, clipboard, hotkeys |
| Frontend | Svelte 5 + TypeScript | Recording overlay UI, settings, onboarding |
| ASR engine (default) | FluidAudio via fluidaudio-rs | CoreML/ANE speech-to-text (macOS) |
| ASR engine (optional) | parakeet-rs (ONNX Runtime) | Cross-platform fallback via `asr-parakeet` feature |
| Audio capture | cpal 0.15 | Cross-platform audio input |
| Build tool | Vite 8 | Multi-page frontend bundling |

## Architecture

- **Menu bar / tray app only.** No Dock icon. No main window by default. The app lives in the macOS menu bar.
- **Floating overlay window** appears during recording to show visual feedback (waveform, status). Implemented as an NSPanel-style always-on-top window.
- **Audio pipeline:** Microphone → cpal → audio buffer → parakeet-rs → transcribed text → clipboard → simulated Cmd+V paste.
- **All audio and ASR processing happens in Rust.** The frontend is only for UI — it never touches audio data or model inference.

## Package Managers

- **Frontend (npm):** `package.json` in project root
- **Backend (cargo):** `Cargo.toml` in `src-tauri/`

## Commands

```bash
# Full app development (frontend + backend)
cargo tauri dev

# Production build
cargo tauri build

# Rust only
cargo build                    # Build Rust backend
cargo clippy -- -D warnings    # Lint Rust code
cargo test                     # Run Rust tests

# Frontend only
npm run dev                    # Svelte dev server
npm run build                  # Frontend production build
npm run check                  # TypeScript/Svelte type checking
```

## Key Directories

```
sotto/
├── src/                  # Svelte 5 frontend
│   ├── lib/              # Shared components and utilities
│   └── routes/           # SvelteKit routes (if applicable)
├── src-tauri/            # Tauri + Rust backend
│   ├── src/              # Rust source code
│   ├── sidecar/          # Python LLM sidecar (mlx-lm)
│   ├── Cargo.toml        # Rust dependencies
│   └── tauri.conf.json   # Tauri configuration
├── benchmarks/llm/       # LLM cleanup benchmarks (dataset, runner, prompts)
├── docs/                 # Documentation
│   ├── specs/            # Feature specs (date-prefixed, immutable)
│   ├── research/         # Research notes (date-prefixed)
│   ├── journals/         # Experiment logs (date-prefixed)
│   ├── audit/            # Code/architecture audits (date-prefixed)
│   └── designs/          # Living design docs
└── .claude/              # Claude Code rules and configuration
```

## Project-Specific Rules

- **NEVER use `npx` for project commands.** Use `npm run <script>` or invoke the CLI tool directly. `npx` can silently install wrong versions or different packages.
- **NEVER modify ASR model paths or download locations without explicit user consent.** Model files are large and their location matters for the user's disk usage and privacy.
- **All audio processing happens in Rust, not in the frontend.** If you need to add audio-related functionality, it goes in `src-tauri/src/`, not in `src/`.
- **The app MUST remain menu-bar-only.** No Dock icon, no main window by default. Any UI must be a floating overlay or a settings panel opened from the tray menu.
- **Accessibility permissions are required** for the paste-at-cursor functionality (simulated Cmd+V). Code that interacts with accessibility APIs must handle the case where permissions have not been granted and guide the user to enable them.
- **Local-only processing is a hard constraint.** Never add features that send audio, transcriptions, or usage data to external services.
- **Experiment logs go in `docs/journals/`**, not alongside code. Benchmark code lives in `benchmarks/`; the narrative of what was tried, measured, and learned lives in `docs/journals/YYYY-MM-DD-slug.md`.

## HuggingFace Artifacts

| Artifact | Repo | Visibility |
|----------|------|------------|
| **Training dataset** | [`juanquivilla/sotto-transcript-cleanup`](https://huggingface.co/datasets/juanquivilla/sotto-transcript-cleanup) | Private |
| **Fine-tuned model** | [`juanquivilla/sotto-cleanup-lfm25-350m`](https://huggingface.co/juanquivilla/sotto-cleanup-lfm25-350m) | Private |

- **HF token** is stored in `.env` (gitignored) as `HF_TOKEN`
- **Naming convention:** `juanquivilla/sotto-{purpose}-{base_model}-{size}` for models; `juanquivilla/sotto-{purpose}` for datasets
- **Upload process:** After each training run, upload the merged model and updated dataset to HF. Always update the model card with latest benchmark numbers.
- **Base model:** `LiquidAI/LFM2.5-350M-Base` — full fine-tuned + stage-2 concentrated training, merged for inference
- **MLX quantized models** for on-device deployment:
  - [`juanquivilla/sotto-cleanup-lfm25-350m-mlx-5bit`](https://huggingface.co/juanquivilla/sotto-cleanup-lfm25-350m-mlx-5bit) — **recommended** (233MB, ROUGE-L 0.926, 99% filler-free)
  - [`juanquivilla/sotto-cleanup-lfm25-350m-mlx-4bit`](https://huggingface.co/juanquivilla/sotto-cleanup-lfm25-350m-mlx-4bit) — smaller (190MB, ROUGE-L 0.897)
- **Quantization recipe:** `mlx_lm.convert --hf-path <model> --mlx-path <out> -q --q-bits 5 --q-group-size 64 --trust-remote-code`
