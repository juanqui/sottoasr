You are an experienced, pragmatic software engineering AI agent. Do not over-engineer a solution when a simple one is possible. Keep edits minimal. If you want an exception to ANY rule, you MUST stop and get permission first.

# Project Overview

## About SottoASR

SottoASR is a local, privacy-first speech-to-text application for macOS. The user presses a global hotkey, speaks, and transcribed text is pasted at their cursor position. All processing happens on-device — no audio or text is ever sent to a cloud service.

## Goals

- Provide fast, accurate speech-to-text with zero latency to cloud services
- Maintain complete user privacy — all audio and transcription stays on-device
- Offer a seamless macOS experience with menu-bar-only UI and global hotkey activation
- Support multiple ASR engines with FluidAudio (CoreML/ANE) as default and parakeet-rs as optional fallback

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

---

# Reference

## Key Directories

```
sotto/
├── src/                  # Svelte 5 frontend
│   ├── lib/              # Shared components and utilities
│   └── routes/           # SvelteKit routes (if applicable)
├── src-tauri/            # Tauri + Rust backend
│   ├── src/              # Rust source code
│   ├── Cargo.toml        # Rust dependencies
│   └── tauri.conf.json   # Tauri configuration
├── docs/                 # Documentation (specs, research, designs)
│   ├── specs/            # Feature specifications (immutable once implemented)
│   ├── research/         # Research notes and findings
│   └── designs/          # Design documents (living docs)
├── .claude/              # Claude Code rules and configuration
└── .github/              # GitHub Actions workflows
```

## Important Files

- `package.json` — Frontend dependencies and scripts
- `src-tauri/Cargo.toml` — Backend dependencies and features
- `src-tauri/tauri.conf.json` — Tauri configuration (includes version)
- `docs/specs/` — Feature specifications following date-prefixed naming
- `.github/workflows/build-release.yml` — CI/CD for releases

## Project-Specific Constraints

- **NEVER use `npx` for project commands.** Use `npm run <script>` or invoke the CLI tool directly.
- **NEVER modify ASR model paths or download locations without explicit user consent.**
- **All audio processing happens in Rust, not in the frontend.**
- **The app MUST remain menu-bar-only.** No Dock icon, no main window by default.
- **Accessibility permissions are required** for paste-at-cursor functionality.
- **Local-only processing is a hard constraint.** Never add features that send audio, transcriptions, or usage data to external services.

---

# Essential Commands

## Development

```bash
# Full app development (frontend + backend)
cargo tauri dev

# Production build
cargo tauri build
```

## Rust Backend

```bash
# Build
cargo build 2>&1 | tee /tmp/build-output.txt

# Lint (catches style and correctness issues)
cargo clippy -- -D warnings 2>&1 | tee /tmp/clippy-output.txt

# Test
cargo test 2>&1 | tee /tmp/test-output.txt
```

## Frontend

```bash
# Dev server
npm run dev

# Production build
npm run build

# Type checking
npm run check 2>&1 | tee /tmp/check-output.txt
```

## Release Process

See [Release Process](#release-process) for the complete tag-driven workflow.

---

# Patterns

## Naming Conventions

### Rust (src-tauri/)

- Variables and functions: `snake_case`
- Types and traits: `PascalCase`
- Constants: `SCREAMING_SNAKE_CASE`
- Modules: `snake_case`

### Svelte / TypeScript (src/)

- Variables and functions: `camelCase`
- Components: `PascalCase` (e.g., `RecordingOverlay.svelte`)
- Files: `kebab-case` for utilities and modules (e.g., `audio-utils.ts`), `PascalCase` for component files
- Types and interfaces: `PascalCase`
- Constants: `SCREAMING_SNAKE_CASE`

## Import Order

Organize imports in this order, separated by blank lines:

1. Framework imports (`svelte`, `tauri`, `std`)
2. Third-party libraries
3. Internal modules (relative imports)
4. Type-only imports

```typescript
// Example (TypeScript)
import { onMount } from 'svelte';
import { invoke } from '@tauri-apps/api/core';

import { formatDuration } from '../utils/time';
import { AudioState } from '../stores/audio';

import type { TranscriptionResult } from '../types';
```

## Documentation Structure

### Directory Organization

```
docs/
├── specs/        # Feature specifications (immutable once implemented)
│   └── YYYY-MM-DD-slug.md
├── research/     # Research notes and findings
│   └── YYYY-MM-DD-slug.md
└── designs/      # Design documents (living docs, updated over time)
    └── slug.md
```

- **Specs** are date-prefixed and represent a point-in-time decision. Once a spec is implemented, create a new spec for further changes rather than rewriting the original.
- **Research** documents capture investigation results, benchmarks, and external findings. Date-prefixed because they reflect knowledge at a point in time.
- **Designs** are living documents (no date prefix) that evolve with the system.

### Document Header Format

Every document must start with:

```markdown
# Title

- **Version:** 1.0
- **Date:** YYYY-MM-DD
- **Status:** Draft | In Review | Approved | Implemented | Superseded
```

### Spec Structure

A complete spec follows this outline:

1. **Summary** — One paragraph explaining what this spec covers and why.
2. **Problem Statement** — What problem does this solve? Who is affected?
3. **Design Overview** — High-level approach with a diagram if helpful.
4. **Detailed Design** — Implementation details, data structures, APIs.
5. **Edge Cases** — What can go wrong? How is each case handled?
6. **File Changes** — Table of files to be created, modified, or deleted.
7. **Testing Strategy** — Unit tests, integration tests, manual verification steps.
8. **Migration Plan** — If applicable: how to migrate existing data or users.
9. **Security Considerations** — Threat model, permissions, data handling.
10. **Cost Analysis** — Performance impact, resource usage, dependencies added.
11. **Implementation Tasks** — Ordered checklist of work items.
12. **Implementation Status** — Updated during and after implementation.

**Critical:** One file per spec. Never split a spec into separate review, summary, or notes files.

## Spec-Driven Development Workflow

Every non-trivial feature follows this lifecycle:

### Phase 1: Research

1. Read requirements carefully. Clarify ambiguities before proceeding.
2. Explore existing codebase to understand architecture, patterns, constraints.
3. Map the blast radius — which files, modules, systems will be affected?
4. Research external dependencies, libraries, APIs using MCP tools.
5. Document findings as you go.

**Output:** Enough understanding to write a grounded spec.

### Phase 2: Specification

Create the spec document at `docs/specs/YYYY-MM-DD-slug.md` following the structure above. Set status to "Draft."

### Phase 3: Review

Perform a minimum of **3 sequential review passes**:

- **Pass 1: Assumption Validation** — Are all technical claims accurate and verified?
- **Pass 2: Completeness** — Are all edge cases, error handling, security implications covered?
- **Pass 3: Clarity and Actionability** — Could another developer implement this without asking questions?

Update status to "In Review" or "Approved" after passes.

### Phase 4: Experimentation (Optional)

When the spec involves unfamiliar technology or risky trade-offs, create small experiments in `/tmp/experiments/` to test specific questions.

### Phase 5: Implementation

1. Break the spec into atomic, committable tasks.
2. Implement in the order defined by the spec's task list.
3. Commit incrementally — each commit should represent a coherent, buildable unit.
4. Follow the file changes table from the spec. If you need to deviate, update the spec first.
5. Write tests alongside implementation, not after.

### Phase 6: Verification

Before declaring work complete, verify all checks pass (see [Pre-PR Checklist](#pre-pr-checklist)).

### Phase 7: Spec Maintenance

1. Update spec status to "Implemented."
2. Document any deviations in a "Deviations" section.
3. Add implementation notes for future developers.
4. If the feature will evolve further, create a design doc in `docs/designs/`.

---

# Anti-Patterns

## Security

- **NEVER** commit secrets — API keys, tokens, credentials, `.env` files, or any sensitive configuration.
- **NEVER** modify model configuration, API keys, or download locations without explicit user permission.

## Data Safety

- **NEVER** delete or reset database data without explicit user consent.
- **NEVER** run destructive operations (file deletion, git reset --hard, database wipes) without confirming intent.

## Git Discipline

- **NEVER** commit or push to git unless explicitly asked. Stage and show changes, then wait for user confirmation.
- **NEVER** push to `main` directly — always use feature branches and pull requests.
- **NEVER** force-push during code review — address feedback in new commits.

## Efficiency

- **NEVER** run builds, tests, or linters multiple times when once suffices. Capture output with `tee`.
- **NEVER** re-read files you have already read in the current session unless the file has been modified.
- **NEVER** shotgun-debug by making multiple speculative changes at once. Follow the scientific method.

## Development Mistakes

- **NEVER** assume you know how something works — always verify by reading the actual code, config, or documentation.
- **NEVER** skip the spec review process for complex features. If unsure whether a feature is complex, it probably is.
- **NEVER** hardcode version strings in frontend components — always use the `getVersion()` API from `@tauri-apps/api/app`.

---

# Code Style

## Commit Conventions

Use conventional commits with these prefixes:

- `feat:` — New feature or capability
- `fix:` — Bug fix
- `docs:` — Documentation changes only
- `refactor:` — Code restructuring without behavior change
- `test:` — Adding or updating tests
- `chore:` — Build config, dependencies, tooling

Keep the subject line under 72 characters. Use the body for context on *why*, not *what*.

```
feat: add recording overlay with waveform visualization

The overlay provides visual feedback during recording so users
know the app is actively capturing audio. Uses a floating
NSPanel to stay above other windows.
```

## Branch Strategy

- `main` is protected — never push directly
- Feature branches: `feature/<short-slug>` (e.g., `feature/overlay-window`)
- Bug fix branches: `fix/<short-slug>` (e.g., `fix/paste-permissions`)
- Keep branches short-lived. Rebase on main before opening a PR.

## Pull Request Guidelines

### Pre-PR Checklist

Run these in order before opening a pull request:

```bash
# 1. Build (catches compilation errors)
cargo build 2>&1 | tee /tmp/cargo-build.txt

# 2. Lint (catches style and correctness issues)
cargo clippy -- -D warnings 2>&1 | tee /tmp/cargo-clippy.txt

# 3. Test (catches regressions)
cargo test 2>&1 | tee /tmp/cargo-test.txt

# 4. Frontend build (if frontend changes were made)
npm run build 2>&1 | tee /tmp/npm-build.txt
```

Always capture output with `tee` so you can review results without re-running.

### PR Requirements

1. Ensure all pre-PR checks pass locally.
2. Write a clear PR title using conventional commit style.
3. In the PR body, include:
   - Summary of changes (what and why)
   - Link to the spec if one exists
   - Testing notes (what was tested, how to verify)
4. Request review. Address feedback in new commits (do not force-push during review).
5. Squash-merge into main once approved.

---

# Release Process

## Overview

SottoASR uses a tag-driven release workflow. Pushing a `v*` tag to `main` triggers a GitHub Actions build that produces a signed `.dmg` and creates a **draft** GitHub Release. After verifying the build artifact, manually publish the draft.

## Steps

### 1. Version Bump

Update the version string in **all five** files:

| File | Field |
|------|-------|
| `package.json` | `"version"` |
| `package-lock.json` | `"version"` (two occurrences: root and `packages[""]`) |
| `src-tauri/tauri.conf.json` | `"version"` |
| `src-tauri/Cargo.toml` | `version` under `[package]` |
| `src-tauri/Cargo.lock` | `version` next to `name = "sottoasr"` |

All five must match exactly.

**Important:** The About screen reads its version at runtime via `getVersion()` from `@tauri-apps/api/app`, which pulls from `tauri.conf.json`. Do NOT hardcode version strings in frontend components.

After bumping, verify no hardcoded versions remain:

```bash
grep -rn '0\.OLD\.VERSION' src/ --include='*.svelte' --include='*.ts'
```

### 2. Update CHANGELOG.md

Add a new section at the top following this format:

```markdown
## [X.Y.Z] — YYYY-MM-DD

One-line summary of the release.

### Added
### Changed
### Fixed
### Infrastructure
```

Only include sections that have entries. Write changelog entries from the user's perspective.

### 3. Update Website Version

Update the version badge in `website/index.html`:

```html
<span class="version-badge">vX.Y.Z</span>
```

### 4. Commit and Push

Stage all changes and commit:

```bash
git add .
git commit -m "chore: release vX.Y.Z"
git push origin main
```

This triggers Cloudflare Pages to deploy the updated website.

### 5. Tag and Push

```bash
git tag vX.Y.Z
git push origin vX.Y.Z
```

This triggers the `.github/workflows/build-release.yml` workflow.

### 6. Monitor CI

```bash
gh run list --workflow=build-release.yml --limit 3
gh run watch <run-id>
```

The workflow:
- Checks out the tag
- Installs Node.js, Rust, and dependencies
- Runs `tauri-action` which builds the app and creates a **draft** GitHub Release
- Uploads the `.dmg` artifact to the release

### 7. Publish the Release

Once the workflow succeeds:

```bash
gh release list --limit 3          # find the draft
gh release edit vX.Y.Z --draft=false
```

Or publish from the GitHub web UI: Releases → Edit → Publish release.

### 8. Verify

- Confirm the release page shows the correct version, changelog, and `.dmg` download
- Confirm the website at the custom domain shows the updated version badge
- Optionally: download the `.dmg` and verify it installs and launches correctly

## Versioning Scheme

Follow [Semantic Versioning](https://semver.org/):

- **Patch** (0.2.x): Bug fixes, polish, icon/branding changes
- **Minor** (0.x.0): New features, significant UX changes
- **Major** (x.0.0): Breaking changes, major rewrites

---

# Critical Rules

These rules are non-negotiable. Violating any of them can cause data loss, security incidents, or broken deployments.

## Verification

- Work is not done until it is verified: build passes, linter is clean, tests pass.
- **NEVER** assume you know how something works — always verify by reading the actual code, config, or documentation.
- All significant work products (specs, designs, implementations) must be reviewed iteratively. Specs require a minimum of 3 review passes before implementation begins.

## Debugging

- No rash decisions during debugging. Follow the scientific method: formulate a hypothesis, validate it with evidence, then apply the fix.
- **NEVER** shotgun-debug by making multiple speculative changes at once. Change one thing, verify, then proceed.

## Planning

- Write a spec first when the feature is complex. If you are unsure whether a feature is complex, it probably is.
- Track all implementation work with tasks. Every unit of work should be accounted for.
