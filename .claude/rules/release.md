# Release Process

## Overview

SottoASR uses a release workflow on GitHub Actions (`.github/workflows/build-release.yml`). A push to `main` and a push of a `v*` tag both run it. Each run builds a signed and notarized `.dmg` and creates or updates a **draft** GitHub Release for `v<version>`. After you examine the build artifact, publish the draft manually to make it the latest release.

`scripts/check-release-target.py` runs first in the workflow. It stops the build if the tag `v<version>` or its draft release already points to a different commit. Thus you must bump the version before you release new code.

## Steps

### 1. Version Bump

Update the version string in five files. `package-lock.json` holds it twice, so there are six values:

| File | Field |
|------|-------|
| `package.json` | `"version"` |
| `package-lock.json` | `"version"` (two occurrences: root and `packages[""]`) |
| `src-tauri/tauri.conf.json` | `"version"` |
| `src-tauri/Cargo.toml` | `version` under `[package]` |
| `src-tauri/Cargo.lock` | `version` next to `name = "sottoasr"` |

All six values must match exactly. The website badge (step 3) must also match. `scripts/ci-checks.sh` fails when one of these values is different.

**Important:** The About screen reads its version at runtime via `getVersion()` from `@tauri-apps/api/app`, which pulls from `tauri.conf.json`. Do NOT hardcode version strings in frontend components — always use this API.

`scripts/ci-checks.sh` fails if the old or new version string appears in a `.ts` or `.svelte` file in `src/`. To find these strings before you run it, use this command:

```bash
grep -rn '0\.OLD\.VERSION' src/ --include='*.svelte' --include='*.ts'
```

### 2. Update CHANGELOG.md

During development, add each entry under `## [Unreleased]` at the top of `CHANGELOG.md`. At release time, rename that heading to the release version and date:

```markdown
## [X.Y.Z] — YYYY-MM-DD

One-line summary of the release.

### Added
### Changed
### Fixed
### Infrastructure
```

Include only the sections that have entries. Write each entry from the point of view of the user, not the developer.

The heading must be exactly `## [X.Y.Z]`. `scripts/ci-checks.sh` fails without it. The release workflow copies the text between this heading and the next `## [` heading into the release notes.

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

To run only the automated checks, use `--auto-only`:

```bash
./scripts/pre-release-check.sh --auto-only
```

To check against a version other than the one in `tauri.conf.json`, add `--version X.Y.Z`.

The script needs these tools on `PATH`: `python3` (3.11 or later, for `tomllib`), `cargo`, `npm`, `node`, `jq`, `cargo audit` and `cargo tauri`. It uses the installed `node_modules`. Run `npm ci` first for a clean install. If a tool is missing, the script stops and shows the install command.

The automated checks start with a version check on the six values. Then the script runs the PR CI commands from `.github/workflows/ci.yml`, in the same order:

1. `scripts/ci-checks.sh`.
2. `npm run build`, `npm run check` and `npm test`.
3. `cargo test --locked`.
4. `cargo check` with the `asr-parakeet` feature, then the resampler tests with that feature.
5. `cargo clippy --locked --all-targets -- -D warnings`.
6. The `zip` check (`cargo tree -i zip --target aarch64-apple-darwin` must print nothing), then `cargo audit`. If `zip` is no longer in `Cargo.lock`, the check passes with a notice to delete the `zip` ignores and checks.
7. `npm audit --omit=dev`.
8. The Python tests in `src-tauri/sidecar/` and `scripts/`.
9. An unsigned `cargo tauri build` of the `.app` and the `.dmg`. PR CI builds the same bundles with `tauri-action`.

Each check writes its output to `/tmp/sotto-smoke-<name>.txt`. The script exits with a non-zero status if an automated check fails. Interactive results do not change the exit status.

All automated checks must pass before you continue. If an interactive check fails, investigate it. You can defer an interactive failure that is specific to your environment, for example when no external microphone is available.

CAUTION: The interactive checks start `cargo tauri dev`. The development build uses the same data folder and global shortcuts as an installed SottoASR, because both use the identifier `com.sottoasr.app`. Quit the installed app before you start the interactive checks.

### 4. Commit and Push

Stage all changes and commit with a conventional commit message:

```
chore: release vX.Y.Z
```

Push to `main`. This push starts the release workflow, which creates the draft release. It also starts the Cloudflare Pages deployment of the website.

### 5. Tag and Push

```bash
git tag vX.Y.Z
git push origin vX.Y.Z
```

The tag push runs `.github/workflows/build-release.yml` again for the tag.

### 6. Monitor CI

```bash
gh run list --workflow=build-release.yml --limit 3
gh run watch <run-id>
```

The workflow:
- Checks out the pushed commit
- Runs `scripts/check-release-target.py`
- Installs Node.js, Rust, and dependencies
- Runs the PR CI checks, except the `asr-parakeet` check
- Imports the Apple signing certificate
- Runs `tauri-action`, which builds, signs and notarizes the app, creates or updates the **draft** GitHub Release, and uploads the bundles to it
- Uploads the `.dmg` and the updater archive as workflow artifacts too

### 7. Publish the Release

When the workflow succeeds, publish the draft:

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

The release workflow uses `tauri-apps/tauri-action` (v0) with `__VERSION__` placeholders. The action reads the version from `tauri.conf.json` and puts it into:

- `tagName: v__VERSION__`
- `releaseName: 'SottoASR v__VERSION__'`
- `releaseBody` (DMG filename, changelog section)

The release is created as a **draft** (`releaseDraft: true`) so you can review it before you publish it.

Both workflows pin each action to a full commit SHA, with the version in a comment. Secrets are declared only in the `env` of the steps that use them. `scripts/test_workflows.py` fails if a `uses:` line is not pinned to a 40-character SHA, or if a secret is declared in workflow-level or job-level `env`.

## Versioning Scheme

Follow [Semantic Versioning](https://semver.org/):

- **Patch** (0.2.x): Bug fixes, polish, icon/branding changes
- **Minor** (0.x.0): New features, significant UX changes
- **Major** (x.0.0): Breaking changes, major rewrites
