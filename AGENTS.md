# SottoASR

Local-first, privacy-first speech-to-text for macOS. Press a hotkey, speak, and text appears at your cursor. Local by default. Two optional cloud engines, AWS Transcribe for speech and AWS Bedrock for AI cleanup, work only after the user turns them on.

## Tech Stack

| Layer | Technology | Purpose |
|-------|-----------|---------|
| App framework | Tauri v2 | Native app shell, system tray, IPC |
| Backend | Rust | Audio capture, ASR processing, clipboard, hotkeys |
| Frontend | Svelte 5 + TypeScript | Recording overlay UI, settings, onboarding |
| ASR engine (default) | FluidAudio via fluidaudio-rs | CoreML/ANE speech-to-text (macOS) |
| ASR engine (optional) | parakeet-rs (ONNX Runtime) | Cross-platform fallback via `asr-parakeet` feature |
| Cloud speech engine (optional) | AWS SDK for Rust | Optional Amazon Transcribe engine, off by default |
| Transcript cleanup (optional) | Python sidecar with MLX and mlx-lm | MiniCPM5 2B on the Mac, the default cleanup engine |
| Cloud cleanup engine (optional) | AWS SDK for Rust (`aws-sdk-bedrockruntime`) | Optional Amazon Bedrock engine for AI cleanup, off by default |
| Audio capture | cpal 0.16 | Cross-platform audio input |
| Build tool | Vite 8 | Multi-page frontend bundling |

## Quick Commands

```bash
# Full app development (frontend + backend)
cargo tauri dev

# Production build
cargo tauri build

# Rust only (run from src-tauri/)
cargo build
cargo clippy -- -D warnings
cargo test

# Frontend only
npm run dev
npm run build
npm run check
```

## Key Architecture Decisions

- **Menu bar / tray app only.** No Dock icon (`LSUIElement` + `ActivationPolicy::Accessory`). No main window by default.
- **All audio capture and ASR inference in Rust.** Frontend is UI only — it never touches audio data or model inference.
- **Dual ASR backend** via Cargo feature flags (`asr-fluidaudio` default, `asr-parakeet` optional).
- **Optional cloud speech engine.** The `speech_engine` setting replaces the local engine with AWS Transcribe. Then no local speech model is in memory.
- **Optional cloud cleanup engine.** The `cleanup_engine` setting replaces the local cleanup model with AWS Bedrock. One cleanup backend exists at a time. In Amazon Bedrock mode no sidecar starts and no cleanup model is in memory.
- **Floating overlay window** with Canvas-based waveform (ring buffer + dynamic range normalization).
- **Clipboard + CGEvent Cmd+V** for paste-at-cursor (requires Accessibility permission).
- **Local by default.** Audio or text leaves the Mac only through a feature that the user turned on, and that the interface names. No telemetry.
- **Multi-page Vite setup** (separate HTML entries for overlay, history, settings, onboarding).
- **Svelte 5 rune stores** use `.svelte.ts` extension (required for `$state()` compilation).

## Important Notes

- Accessibility permission is tied to code signature — must remove and re-add after each rebuild.
- `tccutil reset Accessibility com.sottoasr.app` resets the TCC entry.
- Always launch via `open SottoASR.app`, never the raw binary (LSUIElement/TCC require the .app bundle).
- FluidAudio models (Parakeet Ultra, about 630 MB) are cached at `~/Library/Application Support/FluidAudio/Models/`.
- Logs at `~/Library/Logs/com.sottoasr.app/SottoASR.log`.

## Key Directories

```
sottoasr/
├── src/                  # Svelte 5 frontend
│   └── lib/              # Shared components and utilities
├── src-tauri/            # Tauri + Rust backend
│   ├── src/              # Rust source code
│   ├── sidecar/          # Python LLM sidecar (mlx-lm)
│   ├── Cargo.toml        # Rust dependencies
│   └── tauri.conf.json   # Tauri configuration
├── benchmarks/llm/       # LLM cleanup benchmarks
├── docs/                 # Documentation
│   ├── specs/            # Feature specs (date-prefixed, immutable)
│   ├── research/         # Research notes (date-prefixed)
│   ├── journals/         # Experiment logs (date-prefixed)
│   ├── audit/            # Code/architecture audits (date-prefixed)
│   └── designs/          # Living design docs
└── scripts/              # Build and release scripts
```

---

## Critical Rules

Non-negotiable. Violating any can cause data loss, security incidents, or broken deployments.

### Security

- NEVER commit secrets — API keys, tokens, credentials, `.env` files, or any sensitive configuration.
- NEVER modify model configuration, API keys, or download locations without explicit user permission.

### Data Safety

- NEVER delete or reset database data without explicit user consent.
- NEVER run destructive operations (file deletion, git reset --hard, database wipes) without confirming intent.

### Git Discipline

- NEVER commit or push to git unless explicitly asked. Stage and show changes, then wait for confirmation.
- ALWAYS run the build before pushing to catch compilation errors.
- Use `tee` to capture build/test output so it can be reviewed without re-running expensive commands.

### Verification

- Work is not done until verified: build passes, linter is clean, tests pass.
- NEVER assume you know how something works — always verify by reading the actual code, config, or documentation.
- All significant work products must be reviewed iteratively. Specs require a minimum of 3 review passes before implementation begins.

### Debugging

- No rash decisions. Follow the scientific method: hypothesis → evidence → fix.
- NEVER shotgun-debug by making multiple speculative changes at once.

### Efficiency

- NEVER run builds, tests, or linters multiple times when once suffices. Capture output with `tee`.
- NEVER re-read files already read in the current session unless modified.

### Planning

- Write a spec first when the feature is complex. If unsure whether it's complex, it probably is.
- Track all implementation work with tasks. Every unit of work should be accounted for.

---

## Development Workflow

### Naming Conventions

**Rust (`src-tauri/`):** `snake_case` for variables/functions, `PascalCase` for types/traits, `SCREAMING_SNake_CASE` for constants, `snake_case` for modules.

**Svelte / TypeScript (`src/`):** `camelCase` for variables/functions, `PascalCase` for components and types, `kebab-case` for utility files, `SCREAMING_SNAKE_CASE` for constants.

### Import Order

1. Framework imports (`svelte`, `tauri`, `std`)
2. Third-party libraries
3. Internal modules (relative imports)
4. Type-only imports

### Commit Conventions

Use conventional commits: `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`. Subject under 72 characters. Body explains *why*, not *what*.

### Branch Strategy

- `main` is protected — never push directly.
- Feature branches: `feature/<short-slug>`
- Bug fix branches: `fix/<short-slug>`
- Keep branches short-lived. Rebase on main before opening a PR.

### Pre-PR Checklist

```bash
cargo build 2>&1 | tee /tmp/cargo-build.txt
cargo clippy -- -D warnings 2>&1 | tee /tmp/cargo-clippy.txt
cargo test 2>&1 | tee /tmp/cargo-test.txt
npm run build 2>&1 | tee /tmp/npm-build.txt
```

### Pull Request Workflow

1. Ensure all pre-PR checks pass locally.
2. Clear PR title using conventional commit style.
3. PR body: summary of changes, link to spec, testing notes.
4. Address feedback in new commits (no force-push during review).
5. Squash-merge into main once approved.

---

## Spec-Driven Development Workflow

Every non-trivial feature follows this lifecycle. Do not skip phases.

### Phase 1: Research

Understand the problem space — read requirements, explore the codebase, map blast radius, research external dependencies. Use web search for current information.

### Phase 2: Specification

Create `docs/specs/YYYY-MM-DD-slug.md`. Ground every claim with evidence. Be specific about file changes. Write implementation tasks as an ordered, dependency-aware checklist. Status: "Draft."

### Phase 3: Review

Minimum 3 sequential review passes:

1. **Assumption Validation** — Are technical claims accurate? Hidden assumptions?
2. **Completeness** — Edge cases? Error handling? Security? Migration plan?
3. **Clarity and Actionability** — Could another developer implement this without questions?

### Phase 4: Experimentation (Optional)

For unfamiliar technology or risky trade-offs: small experiments in `/tmp/experiments/`, one question per experiment, feed results back into the spec.

### Phase 5: Implementation

Atomic, committable tasks in spec order. Commit incrementally. Write tests alongside implementation.

### Phase 6: Verification

```bash
cargo build 2>&1 | tee /tmp/verify-build.txt
cargo clippy -- -D warnings 2>&1 | tee /tmp/verify-clippy.txt
npm run check 2>&1 | tee /tmp/verify-check.txt
cargo test 2>&1 | tee /tmp/verify-test.txt
```

Manual verification of happy path and edge cases. Confirm alignment with spec.

### Phase 7: Spec Maintenance

Update status to "Implemented." Document deviations. Add implementation notes.

---

## Documentation Standards

### Directory Structure

```
docs/
├── specs/        # Feature specs (date-prefixed, immutable once implemented)
├── research/     # Research notes (date-prefixed)
├── journals/     # Experiment logs (date-prefixed)
├── audit/        # Code/architecture audits (date-prefixed)
└── designs/      # Living design docs (no date prefix)
```

### Document Format

Every document starts with:

```markdown
# Title

- **Version:** 1.0
- **Date:** YYYY-MM-DD
- **Status:** Draft | In Review | Approved | Implemented | Superseded
```

- Table of Contents if 3+ sections.
- Numbered sections for specs and designs.
- Tables for comparisons.
- Mermaid diagrams for flows and architecture.
- Short paragraphs (3-5 sentences max).

### Spec Structure

1. Summary, 2. Problem Statement, 3. Design Overview, 4. Detailed Design, 5. Edge Cases, 6. File Changes, 7. Testing Strategy, 8. Migration Plan, 9. Security Considerations, 10. Cost Analysis, 11. Implementation Tasks, 12. Implementation Status.

### Rules

- One file per spec. Never split across files.
- Single source of truth per feature or decision.
- No orphaned docs — superseded specs link to their replacement.

---

## Release Process

A push to `main` and a push of a `v*` tag both run `.github/workflows/build-release.yml`. Each run produces a signed `.dmg` and creates or updates a **draft** GitHub Release for `v<version>`.

### Steps

1. **Version Bump** — Update version in all five files (six values, because `package-lock.json` holds it twice): `package.json`, `package-lock.json`, `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`. All must match. The About screen reads version via `getVersion()` from `@tauri-apps/api/app` — never hardcode.

2. **Update CHANGELOG.md** — During development, entries go under `## [Unreleased]` at the top. At release, rename it to `## [X.Y.Z] — YYYY-MM-DD` with Added/Changed/Fixed/Infrastructure subsections.

3. **Update Website Version** — `website/index.html`: `<span class="version-badge">vX.Y.Z</span>`.

4. **Pre-Release Smoke Tests** — `./scripts/pre-release-check.sh` (or `--auto-only`).

5. **Commit and Push** — `chore: release vX.Y.Z` to `main`.

6. **Tag and Push** — `git tag vX.Y.Z` then `git push origin vX.Y.Z`.

7. **Monitor CI** — `gh run watch <run-id>` on `build-release.yml`.

8. **Publish** — `gh release edit vX.Y.Z --draft=false`.

9. **Verify** — Release page, website badge, `.dmg` installs correctly.

### Versioning

- **Patch** (0.2.x): Bug fixes, polish, icon/branding
- **Minor** (0.x.0): New features, significant UX changes
- **Major** (x.0.0): Breaking changes, major rewrites

---

## HuggingFace Artifacts

Historical: the shipped app does not use these artifacts. Its cleanup model is the stock `openbmb/MiniCPM5-2B-MLX`, pinned by revision in `src-tauri/src/llm/engine.rs` (`SOTTO_MODEL`, `MODEL_REVISION`) and `src-tauri/sidecar/llm_cleanup.py`. `training/` keeps the earlier fine-tuning work that produced the models below.

| Artifact | Repo | Visibility |
|----------|------|------------|
| Training dataset | [`juanquivilla/sotto-transcript-cleanup`](https://huggingface.co/datasets/juanquivilla/sotto-transcript-cleanup) | Public |
| Fine-tuned model (bf16) | [`juanquivilla/sotto-cleanup-lfm25-350m`](https://huggingface.co/juanquivilla/sotto-cleanup-lfm25-350m) | Public |
| MLX 5-bit | [`juanquivilla/sotto-cleanup-lfm25-350m-mlx-5bit`](https://huggingface.co/juanquivilla/sotto-cleanup-lfm25-350m-mlx-5bit) | Public |
| MLX 4-bit | [`juanquivilla/sotto-cleanup-lfm25-350m-mlx-4bit`](https://huggingface.co/juanquivilla/sotto-cleanup-lfm25-350m-mlx-4bit) | Public |

- **HF token** in `.env` as `HF_TOKEN`. Remote machine at `juanqui@192.168.1.128` has read-only cached token.
- **Naming:** `juanquivilla/sotto-{purpose}-{base_model}-{size}` for models; `juanquivilla/sotto-{purpose}` for datasets.
- **Base model:** `LiquidAI/LFM2.5-350M-Base` — full fine-tuned (no LoRA), all 354M params.
- **MLX quantization:** `mlx_lm.convert --hf-path <model> --mlx-path <out> -q --q-bits 5 --q-group-size 64 --trust-remote-code`

### Upload Protocol (CRITICAL)

Every model upload MUST include a detailed model card (README.md). Use templates in `docs/templates/`. Model cards MUST include: navigation links, overview, key specs table, "What It Does" examples (4-6 pairs), per-category benchmarks, usage code snippet, training/quantization details, "All Variants" table, links section.

**Upload checklist:**
1. Copy template from `docs/templates/`, fill in benchmark numbers
2. Upload bf16 model + model card
3. Generate MLX 5-bit and 4-bit quant locally
4. Upload both MLX variants with variant-specific model cards
5. Verify all three repos have correct README.md with cross-links
6. Update benchmark numbers in this document

**NEVER delete or overwrite a model card without replacing it.**

---

## Project-Specific Rules

- **NEVER use `npx` for project commands.** Use `npm run <script>` or invoke the CLI tool directly.
- **NEVER modify ASR model paths or download locations** without explicit user consent.
- **All audio processing happens in Rust**, not in the frontend.
- **The app MUST remain menu-bar-only.** No Dock icon, no main window by default.
- **Accessibility permissions are required** for paste-at-cursor. Handle missing permissions gracefully.
- **Local by default.** Audio or text leaves the Mac only through a feature that the user turned on, and that the interface names. No telemetry.
- **Experiment logs go in `docs/journals/`**, not alongside code.
- **The website URL is `sottoasr.app`** (NOT sotto.app).
