# Development Workflow

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

## Pre-PR Checklist

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

## Pull Request Workflow

1. Ensure all pre-PR checks pass locally.
2. Write a clear PR title using conventional commit style.
3. In the PR body, include:
   - Summary of changes (what and why)
   - Link to the spec if one exists
   - Testing notes (what was tested, how to verify)
4. Request review. Address feedback in new commits (do not force-push during review).
5. Squash-merge into main once approved.
