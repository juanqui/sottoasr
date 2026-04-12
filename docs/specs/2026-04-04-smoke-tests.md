# Pre-Release Smoke Test Script

- **Version:** 1.0
- **Date:** 2026-04-04
- **Status:** Approved

## Table of Contents

1. [Summary](#1-summary)
2. [Problem Statement](#2-problem-statement)
3. [Design Overview](#3-design-overview)
4. [Detailed Design](#4-detailed-design)
5. [Edge Cases](#5-edge-cases)
6. [File Changes](#6-file-changes)
7. [Testing Strategy](#7-testing-strategy)
8. [Security Considerations](#8-security-considerations)
9. [Cost Analysis](#9-cost-analysis)
10. [Implementation Tasks](#10-implementation-tasks)

---

## 1. Summary

Add a shell script (`scripts/pre-release-check.sh`) that developers run on their Mac before tagging a release. The script performs 10 fully automated checks (version consistency, build health, lint, tests) and 5 semi-automated interactive checks (permissions, model availability, recording, paste, settings persistence). This is Phase 3 of the 5-phase testing initiative, bridging the gap between unit/integration tests (Phases 1-2) and CI assertions (Phase 5) by catching system-integration issues that only manifest on a real macOS machine.

## 2. Problem Statement

SottoASR relies on macOS system APIs that cannot be exercised in CI:

- **Accessibility permission** (CGEvent-based Cmd+V paste) is gated by TCC and tied to code signature.
- **Microphone capture** (cpal + CoreAudio) requires user consent and a physical audio device.
- **FluidAudio ASR** (CoreML/ANE) requires Apple Silicon and downloaded models.
- **NSPanel overlay** behavior varies across macOS versions and multi-monitor setups.

These capabilities are tested only by manual QA today, which is ad-hoc and error-prone. Past releases have shipped with:

- Version strings out of sync between `package.json` and `tauri.conf.json` (caught by users, not by us).
- Missing window labels in `capabilities/default.json` (caused runtime permission errors).
- CHANGELOG entries forgotten for the tagged version.
- Website version badge left on the previous release.

A structured pre-release checklist, enforced by a script, prevents these classes of errors.

## 3. Design Overview

```
Developer runs: ./scripts/pre-release-check.sh

Phase 1: Automated checks (no interaction needed)
  ┌─────────────────────────────────────────────────┐
  │  1. Version consistency (5 files, 6 values)      │
  │  2. Capability completeness (windows list)      │
  │  3. CHANGELOG entry exists                      │
  │  4. Website version badge matches               │
  │  5. cargo build                                 │
  │  6. cargo clippy --all-targets -- -D warnings    │
  │  7. npm run check                               │
  │  8. cargo test                                  │
  │  9. No hardcoded versions in frontend           │
  │ 10. Sidecar script valid                        │
  └─────────────────────────────────────────────────┘
         │ all pass → continue (or --auto-only → exit)
         ▼
Phase 2: Semi-automated checks (guided prompts)
  ┌─────────────────────────────────────────────────┐
  │ 11. Permission status (Accessibility + Mic)     │
  │ 12. ASR model available                         │
  │ 13. Recording smoke test                        │
  │ 14. Paste verification                          │
  │ 15. Settings round-trip                         │
  └─────────────────────────────────────────────────┘
         │
         ▼
Summary: 15/15 passed, 0 failed, 0 warnings
```

The script is a standalone bash script with no dependencies beyond standard macOS tools (`grep`, `sed`, `python3`). It does not require compilation, so it can be run even when the Rust build is broken (to diagnose _why_ the build is broken).

## 4. Detailed Design

### 4.1 Script Location and Invocation

**Path:** `scripts/pre-release-check.sh`

**Usage:**

```bash
# Full run (automated + interactive)
./scripts/pre-release-check.sh

# Automated checks only (skip interactive prompts)
./scripts/pre-release-check.sh --auto-only

# Verify a specific version (instead of reading from tauri.conf.json)
./scripts/pre-release-check.sh --version 0.7.0

# Combined
./scripts/pre-release-check.sh --auto-only --version 0.7.0
```

**Exit codes:**

| Code | Meaning |
|------|---------|
| 0 | All checks passed |
| 1 | One or more automated checks failed |
| 2 | Script usage error (bad arguments) |

Interactive check failures are reported but do not affect the exit code, since they involve subjective human judgment and may fail for environment-specific reasons (e.g., no microphone plugged in).

### 4.2 Output Format

The script uses ANSI color codes for terminal output:

```
=== Pre-Release Smoke Tests for SottoASR ===
Target version: 0.6.3

── Automated Checks ────────────────────────

 ✓  Version consistency: all 5 files (6 values) match (0.6.3)
 ✓  Capability completeness: all 7 windows listed
 ✓  CHANGELOG has entry for 0.6.3
 ✓  Website version badge matches (v0.6.3)
 ✓  cargo build succeeded (42s)
 ✓  cargo clippy clean (15s)
 ✓  npm run check passed
 ✓  cargo test passed (12 tests, 18s)
 ✓  No hardcoded versions in frontend
 ✓  Sidecar script is valid Python

── Interactive Checks ──────────────────────

 ✓  Accessibility permission: granted
 ⚠  Microphone permission: not determined (will prompt on first use)
 ✓  FluidAudio models found (524 MB)
 ✓  Recording smoke test: user confirmed
 ✗  Paste verification: user reported failure

── Summary ─────────────────────────────────

 Automated:   10 passed, 0 failed
 Interactive:  3 passed, 1 failed, 1 warning
 Total:       13 passed, 1 failed, 1 warning
```

Colors:
- Green (`\033[0;32m`): `✓` pass
- Red (`\033[0;31m`): `✗` fail
- Yellow (`\033[0;33m`): `⚠` warning
- Bold (`\033[1m`): section headers
- Reset (`\033[0m`): after each colored token

### 4.3 Automated Check Details

#### Check 1: Version Consistency

**What:** Extract the version string from all 5 files (6 values, since `package-lock.json` has two) and verify they are identical.

**How:**

```bash
# package.json — top-level "version" field
V_PKG=$(python3 -c "import json; print(json.load(open('package.json'))['version'])")

# package-lock.json — root "version" field
V_LOCK_ROOT=$(python3 -c "import json; print(json.load(open('package-lock.json'))['version'])")

# package-lock.json — packages[""] version
V_LOCK_INNER=$(python3 -c "import json; print(json.load(open('package-lock.json'))['packages']['']['version'])")

# tauri.conf.json — "version" field
V_TAURI=$(python3 -c "import json; print(json.load(open('src-tauri/tauri.conf.json'))['version'])")

# Cargo.toml — version under [package] (uses tomllib, built-in since Python 3.11)
V_CARGO=$(python3 -c "import tomllib; print(tomllib.load(open('src-tauri/Cargo.toml','rb'))['package']['version'])")

# Cargo.lock — version next to name = "sottoasr"
V_CARGO_LOCK=$(grep -A1 'name = "sottoasr"' src-tauri/Cargo.lock | grep 'version' | sed 's/version = "\(.*\)"/\1/' | tr -d ' ')
```

**Pass criterion:** All 6 extracted values are identical. If `--version` was provided, they must also match the provided value.

**Failure output:** Lists each file and its extracted version, highlighting mismatches.

#### Check 2: Capability Completeness

**What:** Every window label used in the Vite multi-page config and Rust source code is listed in `src-tauri/capabilities/default.json`.

**How:**

```bash
# Extract window labels from capabilities/default.json
CAP_WINDOWS=$(python3 -c "
import json
caps = json.load(open('src-tauri/capabilities/default.json'))
for w in caps['windows']:
    print(w)
")

# Extract window labels from vite.config.ts (the keys of rollupOptions.input)
VITE_WINDOWS=$(python3 -c "
import re
content = open('vite.config.ts').read()
# Extract the input: { ... } block inside rollupOptions
m = re.search(r'input:\s*\{([^}]+)\}', content)
if m:
    for key in re.findall(r'(\w+)\s*:', m.group(1)):
        print(key)
" | sort)

# Compare: every Vite window must appear in capabilities
MISSING=""
for w in $VITE_WINDOWS; do
    if ! echo "$CAP_WINDOWS" | grep -qx "$w"; then
        MISSING="$MISSING $w"
    fi
done
```

**Pass criterion:** `MISSING` is empty.

**Failure output:** Lists the missing window labels and shows the current capabilities windows list for reference.

#### Check 3: CHANGELOG Entry

**What:** `CHANGELOG.md` contains a section header matching `## [X.Y.Z]` for the target version.

**How:**

```bash
if grep -q "## \[$VERSION\]" CHANGELOG.md; then
    # pass
fi
```

**Pass criterion:** The grep matches at least one line.

**Failure output:** Shows the first 5 section headers from CHANGELOG.md so the developer can see what versions are present.

#### Check 4: Website Version Badge

**What:** `website/index.html` contains `<span class="version-badge">vX.Y.Z</span>` matching the target version.

**How:**

```bash
BADGE_VERSION=$(grep -oE 'version-badge">v[0-9]+\.[0-9]+\.[0-9]+' website/index.html | \
    sed 's/version-badge">v//')

if [ "$BADGE_VERSION" = "$VERSION" ]; then
    # pass
fi
```

**Pass criterion:** Extracted badge version matches target version.

**Failure output:** Shows the extracted badge version vs. the expected version.

#### Check 5: Build Succeeds

**What:** `cargo build` completes without errors.

**How:**

```bash
local exit_code=0
cargo build --manifest-path src-tauri/Cargo.toml 2>&1 | tee /tmp/sotto-smoke-build.txt || exit_code=$?
```

**Note:** The `|| exit_code=$?` pattern is required because the script uses `set -euo pipefail`. A bare `command; EXIT=$?` would terminate the script on failure before `$?` is captured. The `|| ...` clause prevents `set -e` from triggering.

**Pass criterion:** `exit_code` is 0.

**Failure output:** Prints the last 20 lines of build output for diagnosis. Full output is at `/tmp/sotto-smoke-build.txt`.

**Timing:** The elapsed time is displayed on success (e.g., "cargo build succeeded (42s)").

#### Check 6: Clippy Clean

**What:** `cargo clippy --all-targets -- -D warnings` produces no warnings or errors. The `--all-targets` flag ensures test code is also linted.

**How:**

```bash
local exit_code=0
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings 2>&1 | tee /tmp/sotto-smoke-clippy.txt || exit_code=$?
```

**Pass criterion:** `exit_code` is 0.

**Failure output:** Prints the last 20 lines of clippy output. Full output at `/tmp/sotto-smoke-clippy.txt`.

**Timing:** The elapsed time is displayed on success (e.g., "cargo clippy clean (15s)").

#### Check 7: Frontend Type Check

**What:** `npm run check` (svelte-check + tsc) passes.

**How:**

```bash
local exit_code=0
npm run check 2>&1 | tee /tmp/sotto-smoke-check.txt || exit_code=$?
```

**Pass criterion:** `exit_code` is 0.

**Failure output:** Prints the last 20 lines of output. Full output at `/tmp/sotto-smoke-check.txt`.

#### Check 8: Rust Tests Pass

**What:** `cargo test` passes (includes unit tests from Phase 1 of the testing initiative).

**How:**

```bash
local exit_code=0
cargo test --manifest-path src-tauri/Cargo.toml 2>&1 | tee /tmp/sotto-smoke-test.txt || exit_code=$?
```

**Pass criterion:** `exit_code` is 0.

**Failure output:** Prints the last 20 lines of test output. Full output at `/tmp/sotto-smoke-test.txt`.

**Extra info on success:** Extracts and displays the test count from cargo test output (e.g., "12 tests").

**Timing:** The elapsed time is displayed on success (e.g., "cargo test passed (12 tests, 18s)").

#### Check 9: No Hardcoded Versions in Frontend

**What:** No Svelte or TypeScript files in `src/` contain literal version strings like `v0.6.3`. Versions should come from `getVersion()` at runtime.

**How:**

```bash
HARDCODED=$(grep -rn 'v0\.[0-9]\+\.[0-9]\+' src/ \
    --include='*.svelte' --include='*.ts' 2>/dev/null \
    | grep -v '//.*v0\.' | grep -v 'http' || true)

if [ -z "$HARDCODED" ]; then
    # pass
fi
```

**Pass criterion:** No matches found.

**Failure output:** Prints each matching line with file path and line number.

**Note:** The pattern `v0\.\d+\.\d+` is intentionally broad — it catches any `v0.X.Y` string. This will need adjustment if the project reaches `v1.0.0`, but that is far enough away to not matter now.

#### Check 10: Sidecar Script Valid

**What:** The LLM cleanup sidecar script exists and is syntactically valid Python.

**How:**

```bash
SIDECAR="src-tauri/sidecar/llm_cleanup.py"
if [ ! -f "$SIDECAR" ]; then
    # fail: file missing
fi

python3 -c "
import py_compile, sys
try:
    py_compile.compile('$SIDECAR', doraise=True)
except py_compile.PyCompileError as e:
    print(str(e))
    sys.exit(1)
"
```

**Pass criterion:** File exists and `py_compile.compile()` succeeds.

**Failure output:** Shows the Python syntax error message.

### 4.4 Semi-Automated Check Details

Each interactive check follows this pattern:

1. The script performs any automated pre-check it can (e.g., querying TCC database, checking file existence).
2. It prints the result and, if human verification is needed, prompts with `[y/n/s]` (yes / no / skip).
3. Skipped checks are counted as warnings, not failures.

#### Check 11: Permission Status

**What:** Check whether SottoASR has Accessibility and Microphone permissions granted.

**How:**

Accessibility permission can be queried via the TCC database (read-only):

```bash
# Check Accessibility permission
ACC_STATUS=$(sqlite3 \
    "$HOME/Library/Application Support/com.apple.TCC/TCC.db" \
    "SELECT auth_value FROM access WHERE service='kTCCServiceAccessibility' AND client='com.sottoasr.app';" \
    2>/dev/null)

case "$ACC_STATUS" in
    2) echo "Accessibility permission: granted" ;;
    0) echo "Accessibility permission: denied" ;;
    *)
        echo "Accessibility permission: not found in TCC database"
        echo "  Hint: Run the app once, or grant manually in"
        echo "  System Settings > Privacy & Security > Accessibility"
        ;;
esac
```

Microphone permission uses a similar query against `kTCCServiceMicrophone`.

**Note:** On macOS Sequoia and later, the user-level TCC database may not be directly queryable due to SIP protections. If the `sqlite3` query fails, the script falls back to printing a warning and asking the user to verify manually.

**Prompt:** "Are both permissions granted? [y/n/s] "

#### Check 12: ASR Model Available

**What:** FluidAudio model files exist at the expected cache location.

**How:**

```bash
MODEL_DIR="$HOME/Library/Application Support/FluidAudio/Models"
if [ -d "$MODEL_DIR" ]; then
    MODEL_SIZE=$(du -sh "$MODEL_DIR" 2>/dev/null | cut -f1)
    MODEL_COUNT=$(find "$MODEL_DIR" -type f | wc -l | tr -d ' ')
    echo "FluidAudio models found: $MODEL_COUNT files, $MODEL_SIZE"
else
    echo "FluidAudio model directory not found at:"
    echo "  $MODEL_DIR"
    echo "  Models are downloaded on first use (~500 MB)."
fi
```

**Prompt:** If the directory exists and has files, this auto-passes with an informational note. If not found, it warns and asks: "Continue without models? [y/s] "

#### Check 13: Recording Smoke Test

**What:** The core record-transcribe flow works end-to-end.

**Prompt:**

```
Recording smoke test:
  1. Launch SottoASR if not already running
  2. Press your configured hotkey (default: Cmd+Shift+Space)
  3. Say "hello world"
  4. Release the hotkey
  5. Check that a transcription appeared in the overlay

Did the transcription appear? [y/n/s]
```

#### Check 14: Paste Verification

**What:** Transcribed text is pasted at the cursor position in another application.

**Prompt:**

```
Paste verification:
  1. Open TextEdit (or any text editor)
  2. Place your cursor in the document
  3. Press your hotkey, say a short phrase, release
  4. Check that the transcribed text appeared at the cursor

Did transcribed text appear at the cursor? [y/n/s]
```

#### Check 15: Settings Round-Trip

**What:** Settings changes persist across close/reopen.

**Prompt:**

```
Settings round-trip:
  1. Open Settings from the tray menu (or press Cmd+,)
  2. Change a setting (e.g., toggle a checkbox or change the hotkey)
  3. Close the Settings window
  4. Reopen Settings
  5. Check that your change was preserved

Did the setting persist? [y/n/s]
```

### 4.5 Summary Report

After all checks complete, the script prints a summary:

```bash
echo ""
echo "── Summary ─────────────────────────────────"
echo ""
echo " Automated:   $AUTO_PASS passed, $AUTO_FAIL failed"
if [ "$AUTO_ONLY" = "false" ]; then
    echo " Interactive:  $INT_PASS passed, $INT_FAIL failed, $INT_WARN warning(s)"
fi
echo " Total:       $TOTAL_PASS passed, $TOTAL_FAIL failed, $TOTAL_WARN warning(s)"
```

If any automated check failed, the script exits with code 1 and prints:

```
One or more automated checks failed. Fix the issues above before tagging a release.
```

### 4.6 Script Structure (Pseudocode)

```bash
#!/bin/bash
set -euo pipefail

# ── Constants ──────────────────────────────────
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[0;33m'
BOLD='\033[1m'
RESET='\033[0m'

# ── Counters ───────────────────────────────────
AUTO_PASS=0
AUTO_FAIL=0
INT_PASS=0
INT_FAIL=0
INT_WARN=0

# ── Helpers ────────────────────────────────────
pass()    { AUTO_PASS=$((AUTO_PASS + 1)); printf " ${GREEN}✓${RESET}  %s\n" "$1"; }
fail()    { AUTO_FAIL=$((AUTO_FAIL + 1)); printf " ${RED}✗${RESET}  %s\n" "$1"; }
warn()    { INT_WARN=$((INT_WARN + 1));   printf " ${YELLOW}⚠${RESET}  %s\n" "$1"; }
int_pass(){ INT_PASS=$((INT_PASS + 1));   printf " ${GREEN}✓${RESET}  %s\n" "$1"; }
int_fail(){ INT_FAIL=$((INT_FAIL + 1));   printf " ${RED}✗${RESET}  %s\n" "$1"; }

# Prompt user: ask_user "question" returns 0=yes, 1=no, 2=skip
ask_user() {
    printf "%s [y/n/s] " "$1"
    read -r answer
    case "$answer" in
        [Yy]*) return 0 ;;
        [Nn]*) return 1 ;;
        *)     return 2 ;;
    esac
}

# ── Parse arguments ────────────────────────────
AUTO_ONLY=false
TARGET_VERSION=""

while [ $# -gt 0 ]; do
    case "$1" in
        --auto-only)
            AUTO_ONLY=true
            shift
            ;;
        --version)
            [ $# -ge 2 ] || { echo "Error: --version requires a value"; exit 2; }
            TARGET_VERSION="$2"
            shift 2
            ;;
        *)
            echo "Usage: $0 [--auto-only] [--version X.Y.Z]"
            exit 2
            ;;
    esac
done

# ── Resolve project root ──────────────────────
# Script is at scripts/pre-release-check.sh, so root is one level up
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_ROOT"

# ── Determine target version ──────────────────
if [ -z "$TARGET_VERSION" ]; then
    TARGET_VERSION=$(python3 -c "
import json
print(json.load(open('src-tauri/tauri.conf.json'))['version'])
")
fi
VERSION="$TARGET_VERSION"

printf "\n${BOLD}=== Pre-Release Smoke Tests for SottoASR ===${RESET}\n"
printf "Target version: %s\n\n" "$VERSION"

# ── Pre-check: clean working directory ────────
DIRTY=$(git status --porcelain 2>/dev/null || true)
if [ -n "$DIRTY" ]; then
    printf " ${YELLOW}⚠${RESET}  Working directory has uncommitted changes:\n"
    echo "$DIRTY" | head -10
    printf "    (Consider committing or stashing before a release check.)\n\n"
fi

printf "${BOLD}── Automated Checks ────────────────────────${RESET}\n\n"

# ── Check 1: Version consistency ───────────────
check_version_consistency()  # ... extract from 5 files (6 values), compare

# ── Check 2: Capability completeness ──────────
check_capability_completeness()  # ... compare vite.config.ts vs default.json

# ── Check 3: CHANGELOG entry ──────────────────
check_changelog_entry()  # ... grep for ## [$VERSION]

# ── Check 4: Website version badge ────────────
check_website_badge()  # ... extract from website/index.html

# ── Check 5: Build ────────────────────────────
check_build()  # ... cargo build 2>&1 | tee /tmp/sotto-smoke-build.txt

# ── Check 6: Clippy ───────────────────────────
check_clippy()  # ... cargo clippy --all-targets -- -D warnings

# ── Check 7: Frontend type check ─────────────
check_frontend()  # ... npm run check

# ── Check 8: Rust tests ──────────────────────
check_tests()  # ... cargo test

# ── Check 9: Hardcoded versions ───────────────
check_hardcoded_versions()  # ... grep src/

# ── Check 10: Sidecar script ─────────────────
check_sidecar()  # ... python3 py_compile

# ── Interactive checks ────────────────────────
if [ "$AUTO_ONLY" = "false" ]; then
    printf "\n${BOLD}── Interactive Checks ──────────────────────${RESET}\n\n"

    check_permissions()       # Check 11
    check_asr_models()        # Check 12
    check_recording()         # Check 13
    check_paste()             # Check 14
    check_settings_roundtrip() # Check 15
fi

# ── Summary ───────────────────────────────────
print_summary()

# ── Exit code ─────────────────────────────────
if [ "$AUTO_FAIL" -gt 0 ]; then
    exit 1
fi
exit 0
```

### 4.7 Temporary Files

All build/lint/test output is written to `/tmp/sotto-smoke-*.txt` for post-mortem analysis:

| File | Source |
|------|--------|
| `/tmp/sotto-smoke-build.txt` | `cargo build` output |
| `/tmp/sotto-smoke-clippy.txt` | `cargo clippy` output |
| `/tmp/sotto-smoke-check.txt` | `npm run check` output |
| `/tmp/sotto-smoke-test.txt` | `cargo test` output |

These files are overwritten on each run. They are in `/tmp` and thus cleaned up by the OS on reboot.

### 4.8 Integration with Release Process

The release process in `.claude/rules/release.md` will be updated to reference the smoke test script. A new step 3.5 is inserted between "Update Website Version" (step 3) and "Commit and Push" (step 4):

```markdown
### 3.5. Run Pre-Release Smoke Tests

Run the automated smoke test suite and fix any failures before committing:

\```bash
./scripts/pre-release-check.sh
\```

For a full pre-release check including interactive tests:

\```bash
./scripts/pre-release-check.sh
\```

For automated checks only (e.g., in a CI-like local run):

\```bash
./scripts/pre-release-check.sh --auto-only
\```

All automated checks must pass before proceeding. Interactive check failures should be investigated but may be deferred if they are environment-specific (e.g., no external microphone available).
```

## 5. Edge Cases

### 5.1 Missing Tools

| Tool | Required By | Fallback |
|------|------------|----------|
| `python3` (3.11+) | JSON parsing, TOML parsing (`tomllib`), py_compile | Fatal error: print message and exit. Python 3 ships with macOS and is required by the sidecar anyway. `tomllib` is built-in since 3.11; macOS Ventura (13.0+) ships 3.12+. |
| `cargo` | Build, clippy, test | Fatal error: print message and exit. Cannot test a Rust project without Rust. |
| `npm` | Frontend type check | Fatal error: print message and exit. Frontend build requires Node.js. |
| `sqlite3` | TCC database query | Warn and skip permission auto-check; fall back to manual prompt. |
| `grep`, `sed` | Various text extraction | These are POSIX standard; if missing, something is very wrong. |

The script checks for `python3`, `cargo`, and `npm` at startup and exits immediately with a clear message if any are missing.

### 5.2 Cargo.toml and Cargo.lock Version Extraction

`Cargo.toml` version extraction uses `python3` with `tomllib` (built-in since Python 3.11) to parse the `[package].version` field. This is robust against section ordering and avoids the fragility of grep-based extraction (e.g., matching a `version` key in `[dependencies]` instead of `[package]`). Since the project already requires Python 3 for the sidecar script, `tomllib` availability is guaranteed on any supported development machine.

`Cargo.lock` contains many `[[package]]` entries. The grep for `name = "sottoasr"` must match the correct one. Since package names are unique in a lockfile, `grep -A1 'name = "sottoasr"'` is unambiguous. Verified: the current `Cargo.lock` has exactly one `name = "sottoasr"` entry at line 4888.

### 5.3 Running from Wrong Directory

The script resolves the project root relative to its own location (`$SCRIPT_DIR/..`), so it works regardless of the caller's working directory. However, if the script is invoked via a symlink, the resolution may fail. This is acceptable; symlinked invocation is not a supported use case.

### 5.4 Version String with Pre-Release Suffix

If the version ever includes a pre-release suffix (e.g., `0.7.0-beta.1`), the checks still work because they compare exact strings. The hardcoded-version grep pattern (`v0\.\d+\.\d+`) would not match a suffixed version, which is correct behavior — pre-release suffixes in frontend code would be a different kind of bug.

### 5.5 Concurrent Runs

If two smoke test runs overlap, they write to the same `/tmp/sotto-smoke-*.txt` files. This is benign — the files are for post-mortem review, and the later run's output overwrites the earlier one. Concurrent pre-release checks are not a realistic scenario.

### 5.6 TCC Database Inaccessible

On macOS Sequoia (15.0+) and later, Full Disk Access may be required to read the TCC database. If the `sqlite3` query fails (permission denied or database locked), the script:

1. Prints a warning explaining why the auto-check failed.
2. Falls back to the manual prompt asking the user to verify permissions via System Settings.
3. Does not count the failed auto-check as a failure.

### 5.7 No website/index.html

If the website directory does not exist (e.g., on a contributor's fork that doesn't include the marketing site), the website badge check emits a warning instead of a failure. This is a soft check.

### 5.8 set -e and Intentional Non-Zero Exits

The script uses `set -euo pipefail` for safety, but several commands are expected to return non-zero (e.g., `grep` with no matches, `cargo build` with compilation errors). These are handled by:

- Using `|| exit_code=$?` to capture the exit code of commands that may fail (build, clippy, test, frontend check). This prevents `set -e` from terminating the script before the exit code is recorded. The pattern is: `local exit_code=0; command || exit_code=$?`.
- Using `|| true` to suppress expected non-zero exits where the exit code is not needed (e.g., `grep` that may match nothing).
- Using `if command; then ... else ... fi` which does not trigger `set -e`.

## 6. File Changes

| File | Action | Description |
|------|--------|-------------|
| `scripts/pre-release-check.sh` | **Create** | The smoke test script (~300 lines). Must be executable (`chmod +x`). |
| `.claude/rules/release.md` | **Modify** | Add step 3.5 referencing the smoke test script between "Update Website Version" and "Commit and Push". Also fix pre-existing error: "all four files" changed to "all five files" to match the table that lists 5 files. |

No other files are created or modified. The script is self-contained with no configuration files, no generated output beyond `/tmp/` temporaries, and no dependencies to install.

## 7. Testing Strategy

### 7.1 Testing the Script Itself

The smoke test script is itself tested by running it. This is circular by design — it is a developer tool, not production code. Verification approach:

1. **Happy path:** Run the script on the current codebase (which should be in a consistent state). All automated checks should pass.
2. **Induced failures:** Temporarily break each check and verify the script catches it:
   - Change the version in `package.json` to `99.99.99` and run. Check 1 should fail.
   - Remove a window from `capabilities/default.json` and run. Check 2 should fail.
   - Delete the current version's CHANGELOG section. Check 3 should fail.
   - Change the website badge version. Check 4 should fail.
   - Introduce a Rust compile error. Check 5 should fail.
   - Introduce a clippy warning. Check 6 should fail.
   - Introduce a TypeScript error. Check 7 should fail.
   - Make a Rust test fail. Check 8 should fail.
   - Add a hardcoded version string to a `.svelte` file. Check 9 should fail.
   - Rename the sidecar script. Check 10 should fail.
3. **Flag testing:** Run with `--auto-only` and verify interactive prompts are skipped. Run with `--version X.Y.Z` and verify the override works.
4. **Edge case testing:** Run from a subdirectory (not the project root) to verify the script resolves paths correctly.

### 7.2 Testing in the Release Flow

The ultimate test is using the script during an actual release:

1. Follow the release process through step 3 (Update Website Version).
2. Run `./scripts/pre-release-check.sh`.
3. Fix any failures.
4. Proceed with step 4 (Commit and Push).

If the script catches a real issue during a release, that is evidence it is working as intended.

## 8. Security Considerations

### 8.1 TCC Database Access

The script reads the TCC database in read-only mode (`sqlite3 ... "SELECT ..."`). It never writes to the database. On systems where Full Disk Access is required to read TCC, the query simply fails and the script falls back to a manual prompt. No security boundary is crossed.

### 8.2 Temporary Files

Build output is written to `/tmp/sotto-smoke-*.txt`. These files may contain file paths, compiler warnings, and test output. They do not contain secrets (no API keys, tokens, or credentials are involved in the build process). The files are world-readable (default `/tmp` permissions), which is acceptable for build logs.

### 8.3 Script Execution

The script does not download anything, does not execute remote code, and does not modify the project's source code. It only reads files and runs standard build tools (`cargo`, `npm`) that the developer would run manually anyway.

### 8.4 No Elevated Privileges

The script does not use `sudo` and does not require root. All operations run under the developer's user account.

## 9. Cost Analysis

### 9.1 Execution Time

| Check | Estimated Time | Notes |
|-------|---------------|-------|
| Version consistency | < 1s | File reads only |
| Capability completeness | < 1s | File reads only |
| CHANGELOG entry | < 1s | Single grep |
| Website version badge | < 1s | Single grep |
| `cargo build` | 30-120s | Depends on cache state; incremental build is ~30s |
| `cargo clippy` | 10-60s | Depends on cache state; usually fast after a build |
| `npm run check` | 5-15s | svelte-check + tsc |
| `cargo test` | 10-30s | Depends on number of tests |
| Hardcoded versions | < 1s | Single grep |
| Sidecar script | < 1s | py_compile |
| Interactive checks | 2-5 min | Depends on developer speed |

**Total estimated time:**
- `--auto-only`: 1-4 minutes (dominated by cargo build/clippy/test)
- Full run: 3-9 minutes

### 9.2 Resource Usage

- **Disk:** No new files checked in beyond the script itself (~300 lines, ~10 KB). Temporary files in `/tmp` total < 1 MB.
- **Dependencies:** None. Uses only tools already required by the project (python3, cargo, npm, standard Unix utilities).
- **CI impact:** The script is designed for local use, not CI. The `--auto-only` flag exists to support a future CI integration (Phase 5), but that is out of scope for this spec.

### 9.3 Maintenance Burden

The script must be updated when:
- A new window label is added to the app (the capability check will catch this automatically — it reads from `vite.config.ts` dynamically).
- The version file locations change (unlikely, as they are dictated by npm, Cargo, and Tauri conventions).
- The sidecar script is renamed or moved.
- The project reaches `v1.0.0` (the hardcoded version grep pattern needs updating from `v0\.` to a more general pattern).

The maintenance burden is low because the script reads from the same source-of-truth files that the build system uses, rather than maintaining its own list of expected values.

## 10. Implementation Tasks

- [ ] **Task 1: Create `scripts/` directory and script skeleton**
  Create `scripts/pre-release-check.sh` with the shebang, `set -euo pipefail`, color constants, counter variables, helper functions (`pass`, `fail`, `warn`, `int_pass`, `int_fail`, `ask_user`), argument parsing (including `--version` guard against missing value), project root resolution, and version detection. After printing the header, run a `git status --porcelain` pre-check and warn if the working directory has uncommitted changes. The script should exit cleanly with the summary (showing 0/0 counts). Mark executable with `chmod +x`.

- [ ] **Task 2: Implement prerequisite checks**
  Add a startup block that verifies `python3`, `cargo`, and `npm` are on `$PATH`. If any are missing, print a clear error message naming the missing tool and exit with code 2.

- [ ] **Task 3: Implement Check 1 — Version consistency**
  Extract versions from all 5 files (6 values including both `package-lock.json` locations). Compare all values. If `--version` was provided, also compare against that. Call `pass` or `fail` with a descriptive message. On failure, list each file and its extracted version.

- [ ] **Task 4: Implement Check 2 — Capability completeness**
  Extract window labels from `vite.config.ts` input keys. Extract the windows array from `src-tauri/capabilities/default.json`. Compute the set difference. Call `pass` or `fail`. On failure, list missing window labels.

- [ ] **Task 5: Implement Check 3 — CHANGELOG entry**
  Grep `CHANGELOG.md` for a section header matching `## [$VERSION]`. Call `pass` or `fail`. On failure, show the first 5 version headers found.

- [ ] **Task 6: Implement Check 4 — Website version badge**
  Extract the version from the `version-badge` span in `website/index.html`. Compare against target version. If `website/index.html` does not exist, emit a warning instead of a failure. Call `pass`, `fail`, or `warn`.

- [ ] **Task 7: Implement Checks 5-8 — Build, clippy, frontend check, tests**
  Implement the four build/lint/test checks. Each one uses the `set -e`-safe pattern (`local exit_code=0; command 2>&1 | tee /tmp/sotto-smoke-*.txt || exit_code=$?`) to capture the exit code without triggering early termination. Use `--manifest-path src-tauri/Cargo.toml` for cargo commands. Call `pass` or `fail` based on `exit_code`. On failure, print the last 20 lines of output. On success for `cargo test`, extract and display the test count. Use `--all-targets` for clippy. Display elapsed time for build, clippy, and test checks.

- [ ] **Task 8: Implement Check 9 — Hardcoded versions**
  Run the grep for version-like strings in `src/` Svelte and TypeScript files. Call `pass` if no matches, `fail` if matches found (listing them).

- [ ] **Task 9: Implement Check 10 — Sidecar script**
  Check that `src-tauri/sidecar/llm_cleanup.py` exists. If it does, run `python3 -c "import py_compile; py_compile.compile('...', doraise=True)"`. Call `pass` or `fail`.

- [ ] **Task 10: Implement Checks 11-15 — Interactive checks**
  Implement the five interactive checks, gated behind `if [ "$AUTO_ONLY" = "false" ]`. Each check prints instructions, calls `ask_user`, and routes the response to `int_pass`, `int_fail`, or `warn` (for skip). For Check 11, attempt the TCC database query first and report the result before prompting. For Check 12, check the model directory and report size/count.

- [ ] **Task 11: Implement summary and exit logic**
  Print the summary section with pass/fail/warning counts. Exit with code 1 if `AUTO_FAIL > 0`, otherwise exit 0.

- [ ] **Task 12: Update release.md**
  Add step 3.5 ("Run Pre-Release Smoke Tests") to `.claude/rules/release.md` between the existing step 3 and step 4. Include the invocation command and a note about when interactive tests can be deferred.

- [ ] **Task 13: End-to-end verification**
  Run the script on the current codebase (`./scripts/pre-release-check.sh --auto-only`) and verify all automated checks pass. Fix any issues in the script. Capture the output with `tee` for review.
