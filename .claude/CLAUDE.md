# SottoASR

Local-first, privacy-first speech-to-text for macOS. Press a hotkey, speak, and text appears at your cursor. Local by default. Two optional cloud engines, AWS Transcribe for speech and AWS Bedrock for AI cleanup, work only after the user turns them on.

## Tech Stack
- Tauri v2 (Rust backend)
- Svelte 5 + TypeScript (frontend)
- FluidAudio via fluidaudio-rs (CoreML/ANE — default macOS backend)
- parakeet-rs (ONNX Runtime — cross-platform fallback via `asr-parakeet` feature flag)
- AWS SDK for Rust (optional Amazon Transcribe and Amazon Bedrock engines)
- Python sidecar with MLX and mlx-lm (MiniCPM5 2B, the default AI cleanup engine)
- cpal 0.16 (audio capture)
- Vite 8 (build tool)

## Quick Commands
- `cargo tauri dev` — Run the app in development mode
- `cargo tauri build` — Build production bundle (.app + .dmg)
- `cargo clippy -- -D warnings` — Lint Rust code (run from src-tauri/)
- `cargo test` — Run Rust tests (run from src-tauri/)
- `npm run dev` — Frontend only dev server
- `npm run build` — Frontend production build

## Key Architecture Decisions
- Menu bar / tray app only (no Dock icon — `LSUIElement` + `ActivationPolicy::Accessory`)
- All audio capture and ASR inference in Rust (frontend is UI only)
- Dual ASR backend via Cargo feature flags (`asr-fluidaudio` default, `asr-parakeet` optional)
- Optional cloud speech engine: the `speech_engine` setting replaces the local engine with AWS Transcribe (no local speech model in memory)
- Optional cloud cleanup engine: the `cleanup_engine` setting replaces the local cleanup model with AWS Bedrock. One cleanup backend at a time. In Amazon Bedrock mode, no sidecar and no cleanup model in memory
- Floating overlay window with Canvas-based waveform (ring buffer + dynamic range normalization)
- Clipboard + CGEvent Cmd+V for paste-at-cursor (requires Accessibility permission)
- Local by default. Audio or text leaves the Mac only through a feature that the user turned on, and that the interface names. No telemetry.
- Multi-page Vite setup (separate HTML entries for overlay, history, settings, onboarding)
- Svelte 5 rune stores use `.svelte.ts` extension (required for `$state()` compilation)

## Important Notes
- Accessibility permission is tied to code signature — must remove and re-add after each rebuild
- `tccutil reset Accessibility com.sottoasr.app` resets the TCC entry
- Always launch via `open SottoASR.app`, never the raw binary (LSUIElement/TCC require the .app bundle)
- FluidAudio models (Parakeet Ultra, about 630 MB) are cached at `~/Library/Application Support/FluidAudio/Models/`
- Logs at `~/Library/Logs/com.sottoasr.app/SottoASR.log`

## Rules
See `.claude/rules/` for development conventions and workflows.
