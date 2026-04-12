# Release Process

## Overview

SottoASR uses a tag-driven release workflow. Pushing a `v*` tag to `main` triggers a GitHub Actions build that produces a signed `.dmg` and creates a **draft** GitHub Release. After verifying the build artifact, manually publish the draft to make it the latest release.

## Steps

### 1. Version Bump

Update the version string in all five files:

| File | Field |
|------|-------|
| `package.json` | `"version"` |
| `package-lock.json` | `"version"` (two occurrences: root and `packages[""]`) |
| `src-tauri/tauri.conf.json` | `"version"` |
| `src-tauri/Cargo.toml` | `version` under `[package]` |
| `src-tauri/Cargo.lock` | `version` next to `name = "sottoasr"` |

All five must match exactly.

**Important:** The About screen reads its version at runtime via `getVersion()` from `@tauri-apps/api/app`, which pulls from `tauri.conf.json`. Do NOT hardcode version strings in frontend components — always use this API.

After bumping, verify no hardcoded versions remain:

```bash
grep -rn '0\.OLD\.VERSION' src/ --include='*.svelte' --include='*.ts'
```

### 2. Update CHANGELOG.md

Add a new section at the top of CHANGELOG.md following the existing format:

```markdown
## [X.Y.Z] — YYYY-MM-DD

One-line summary of the release.

### Added
### Changed
### Fixed
### Infrastructure
```

Only include sections that have entries. Write changelog entries from the user's perspective, not the developer's.

### 3. Update Website Version

Update the version badge in `website/index.html`:

```html
<span class="version-badge">vX.Y.Z</span>
```

### 3.5. Run Pre-Release Smoke Tests

Run the automated smoke test suite and fix any failures before committing:

```bash
./scripts/pre-release-check.sh
```

For automated checks only (e.g., in a CI-like local run):

```bash
./scripts/pre-release-check.sh --auto-only
```

All automated checks must pass before proceeding. Interactive check failures should be investigated but may be deferred if they are environment-specific (e.g., no external microphone available).

### 4. Commit and Push

Stage all changes and commit with a conventional commit message:

```
chore: release vX.Y.Z
```

Push to `main`. This push also triggers Cloudflare Pages to deploy the updated website.

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

## CI Workflow Details

The release workflow (`.github/workflows/build-release.yml`) uses `tauri-apps/tauri-action@v0` with `__VERSION__` placeholders. Tauri action reads the version from `tauri.conf.json` and substitutes it into:

- `tagName: v__VERSION__`
- `releaseName: 'SottoASR v__VERSION__'`
- `releaseBody` (DMG filename, changelog link)

The release is created as a **draft** (`releaseDraft: true`) so you can review before publishing.

## Versioning Scheme

Follow [Semantic Versioning](https://semver.org/):

- **Patch** (0.2.x): Bug fixes, polish, icon/branding changes
- **Minor** (0.x.0): New features, significant UX changes
- **Major** (x.0.0): Breaking changes, major rewrites
