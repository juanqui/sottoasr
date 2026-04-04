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
| **Training dataset** | [`juanquivilla/sotto-transcript-cleanup`](https://huggingface.co/datasets/juanquivilla/sotto-transcript-cleanup) | Public |
| **Fine-tuned model (bf16)** | [`juanquivilla/sotto-cleanup-lfm25-350m`](https://huggingface.co/juanquivilla/sotto-cleanup-lfm25-350m) | Public |
| **MLX 5-bit (recommended)** | [`juanquivilla/sotto-cleanup-lfm25-350m-mlx-5bit`](https://huggingface.co/juanquivilla/sotto-cleanup-lfm25-350m-mlx-5bit) | Public |
| **MLX 4-bit** | [`juanquivilla/sotto-cleanup-lfm25-350m-mlx-4bit`](https://huggingface.co/juanquivilla/sotto-cleanup-lfm25-350m-mlx-4bit) | Public |

- **HF token** is stored in `.env` (gitignored) as `HF_TOKEN`. The remote training machine at `juanqui@192.168.1.128` has a **read-only** cached token; use the write token from `.env` for uploads.
- **Naming convention:** `juanquivilla/sotto-{purpose}-{base_model}-{size}` for models; `juanquivilla/sotto-{purpose}` for datasets
- **Base model:** `LiquidAI/LFM2.5-350M-Base` — full fine-tuned (no LoRA), all 354M params trainable
- **Current best:** v22+GRPO — ROUGE-L 0.954 (val set), 66% Exact Match, 91% Filler-Free. Pipeline: cleaned data (text fixes + 6K new) → SFT LR 3e-5, β2=0.95 → GRPO LoRA r=32, LR 3e-6
- **MLX quantized models** for on-device deployment:
  - 5-bit affine, group_size=64 — **recommended** (~237MB, minimal quality loss)
  - 4-bit affine, group_size=64 — smaller (~195MB, slightly lower quality)
- **Quantization recipe:** `mlx_lm.convert --hf-path <model> --mlx-path <out> -q --q-bits 5 --q-group-size 64 --trust-remote-code`

### HuggingFace Upload Protocol (CRITICAL)

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
