# CI Configuration Assertions

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

Add automated CI checks that catch configuration drift bugs before they ship. This is Phase 5 of a 5-phase testing initiative. The checks enforce invariants that span multiple files — version consistency across 6 sources, window labels in capabilities, command registration completeness, frontend IPC alignment, changelog coverage, and no hardcoded version strings. A single `scripts/ci-checks.sh` script runs all assertions and is invoked as a CI step in the release workflow before the build.

Recent releases (v0.6.2, v0.6.3) shipped bugs caused by exactly these classes of configuration drift: windows not registered in capabilities, update modal issues from stale state, and CI environment variables overriding critical build flags. These were avoidable with automated checks.

## 2. Problem Statement

SottoASR's configuration is spread across 5+ files that must stay in sync. There is no automated enforcement. The current failure modes include:

1. **Version skew.** The version is specified in `package.json`, `package-lock.json` (two places), `tauri.conf.json`, `Cargo.toml`, and `Cargo.lock`. A mismatch causes the Tauri build to produce artifacts with wrong version labels, or `getVersion()` in the frontend to return a different version than what the DMG filename says.

2. **Capability gaps.** Every window created via `WebviewWindowBuilder::new` must be listed in `src-tauri/capabilities/default.json`. A missing entry causes the window to open without IPC permissions — Tauri commands silently fail, the window appears blank or non-functional. This happened when the `update` window was added in v0.6.0 and initially omitted from the capabilities list (fixed in v0.6.1).

3. **Unregistered commands.** A `#[tauri::command]` function that is not listed in `generate_handler![]` in `lib.rs` silently does nothing when invoked from the frontend. The Rust compiler does not catch this because both the attribute and the macro are valid independently — the function compiles fine, it just never gets wired to IPC.

4. **IPC drift.** TypeScript wrapper functions in `src/lib/utils/tauri.ts` call command names as strings. If a Rust command is renamed or removed, the TypeScript call compiles fine but fails at runtime.

5. **Missing changelog entries.** A tagged release without a corresponding `## [X.Y.Z]` entry in `CHANGELOG.md` produces a GitHub Release with an empty "What's New" section, since the extract step (`awk` in the workflow) finds nothing.

6. **Hardcoded versions.** Frontend `.svelte` or `.ts` files that embed a version string like `"0.6.3"` or `"v0.6.3"` become stale after every release. The canonical version comes from `getVersion()` via `@tauri-apps/api/app`.

All of these are detectable with static analysis against the source tree. None require a build or runtime execution to check.

## 3. Design Overview

```
scripts/ci-checks.sh
├── Check 1: Version consistency (6 sources)
├── Check 2: Capability window completeness
├── Check 3: Tauri command registration completeness
├── Check 4: Frontend IPC command alignment
├── Check 5: CHANGELOG entry exists (tag builds only)
├── Check 6: No hardcoded versions in frontend
├── Check 7: cargo test (delegates to cargo)
└── Check 8: npm run check (delegates to npm)

.github/workflows/build-release.yml
└── New step: "Run CI assertions" (after npm ci, before cargo audit)
```

The script is a single Bash file that:
- Runs each check independently, reporting pass/fail per check.
- Collects all failures before exiting, so a developer sees every problem in one run.
- Exits non-zero if any check fails.
- Uses only standard Unix tools (`jq`, `grep`, `sed`, `awk`, `sort`, `comm`) plus `cargo` and `npm`.
- The `jq` dependency is already available on GitHub Actions `macos-latest` runners.

Checks 7 and 8 (`cargo test` and `npm run check`) are included in the script for completeness but can also be run as separate CI steps if parallelism is desired. The script detects whether they have already been run (via environment variable) to avoid duplication.

## 4. Detailed Design

### 4.1 Script Structure

The script follows a consistent pattern for each check:

```bash
#!/usr/bin/env bash
set -euo pipefail

FAILED=0
CHECKS_RUN=0
CHECKS_PASSED=0

pass() {
  CHECKS_PASSED=$((CHECKS_PASSED + 1))
  echo "  PASS: $1"
}

fail() {
  FAILED=$((FAILED + 1))
  echo "  FAIL: $1"
}

header() {
  CHECKS_RUN=$((CHECKS_RUN + 1))
  echo ""
  echo "[$CHECKS_RUN] $1"
  echo "---"
}
```

Each check is a function that calls `pass` or `fail`. At the end, the script prints a summary and exits with the appropriate code.

### 4.2 Check 1: Version Consistency

Extract the version from each of the 6 sources and compare them.

```bash
check_version_consistency() {
  header "Version consistency"

  local pkg_version
  pkg_version=$(jq -r '.version' package.json)

  local lock_version
  lock_version=$(jq -r '.version' package-lock.json)

  local lock_pkg_version
  lock_pkg_version=$(jq -r '.packages[""].version' package-lock.json)

  local tauri_version
  tauri_version=$(jq -r '.version' src-tauri/tauri.conf.json)

  local cargo_version
  cargo_version=$(sed -n '/^\[package\]/,/^\[/p' src-tauri/Cargo.toml \
    | grep -m1 '^version' | sed 's/version = "\(.*\)"/\1/')

  local cargo_lock_version
  cargo_lock_version=$(awk '/^name = "sottoasr"/{getline; print}' src-tauri/Cargo.lock \
    | sed 's/version = "\(.*\)"/\1/')

  echo "  package.json:              $pkg_version"
  echo "  package-lock.json (root):  $lock_version"
  echo "  package-lock.json (pkg):   $lock_pkg_version"
  echo "  tauri.conf.json:           $tauri_version"
  echo "  Cargo.toml:                $cargo_version"
  echo "  Cargo.lock:                $cargo_lock_version"

  local all_match=true
  for v in "$lock_version" "$lock_pkg_version" "$tauri_version" "$cargo_version" "$cargo_lock_version"; do
    if [[ "$v" != "$pkg_version" ]]; then
      all_match=false
    fi
  done

  if $all_match; then
    pass "All 6 version sources match: $pkg_version"
  else
    fail "Version mismatch detected (see above)"
  fi
}
```

**Why each source matters:**
- `package.json` — used by `npm` scripts, Tauri's `beforeBuildCommand`.
- `package-lock.json` (root `.version` and `.packages[""].version`) — both must match `package.json` or `npm ci` fails in CI.
- `tauri.conf.json` — Tauri reads this at build time for the app version; `tauri-action` substitutes `__VERSION__` from it.
- `Cargo.toml` — Rust crate version; embedded in the binary.
- `Cargo.lock` — must match `Cargo.toml` or the lockfile is stale.

### 4.3 Check 2: Capability Window Completeness

Every window label created by `WebviewWindowBuilder::new` must appear in the `windows` array of `src-tauri/capabilities/default.json`.

```bash
check_capability_windows() {
  header "Capability window completeness"

  # Extract window labels from capabilities JSON
  local cap_windows
  cap_windows=$(jq -r '.windows[]' src-tauri/capabilities/default.json | sort)

  # Extract window labels from Rust code.
  # These function calls are multiline in the source (the label string is on a
  # subsequent line), so we use grep -A to capture context lines after the
  # function name, then extract quoted strings from the context.
  #
  # Pattern 1: WebviewWindowBuilder::new(&handle, "LABEL", ...)
  #   or       WebviewWindowBuilder::new(&app, "LABEL", ...)
  # Pattern 2: open_or_focus_window(app, "LABEL", ...)
  #   The open_or_focus_window helper passes label to WebviewWindowBuilder::new internally.
  local code_labels
  code_labels=$(
    grep -A3 'WebviewWindowBuilder::new(' src-tauri/src/*.rs src-tauri/src/**/*.rs 2>/dev/null \
    | grep -oE '"[a-z_]+"' | sed 's/"//g' \
    | sort -u
  )

  # Also check open_or_focus_window calls that pass string literals
  local helper_labels
  helper_labels=$(
    grep -A2 'open_or_focus_window(' src-tauri/src/*.rs src-tauri/src/**/*.rs 2>/dev/null \
    | grep -oE '"[a-z_]+"' | sed 's/"//g' \
    | sort -u
  )

  # Merge and deduplicate
  local all_code_labels
  all_code_labels=$(echo -e "${code_labels}\n${helper_labels}" | sort -u | grep -v '^$')

  echo "  Capabilities: $(echo "$cap_windows" | tr '\n' ' ')"
  echo "  Code labels:  $(echo "$all_code_labels" | tr '\n' ' ')"

  # Find labels in code but NOT in capabilities
  local missing
  missing=$(comm -23 <(echo "$all_code_labels") <(echo "$cap_windows"))

  if [[ -z "$missing" ]]; then
    pass "All window labels in code are listed in capabilities"
  else
    fail "Window labels in code but missing from capabilities/default.json:"
    echo "$missing" | while read -r label; do
      echo "    - $label"
    done
  fi

  # Informational: labels in capabilities but not in code (not a failure,
  # could be created dynamically or be legacy)
  local extra
  extra=$(comm -13 <(echo "$all_code_labels") <(echo "$cap_windows"))
  if [[ -n "$extra" ]]; then
    echo "  INFO: Labels in capabilities but not found in code: $(echo "$extra" | tr '\n' ' ')"
    echo "        (Not a failure — may be dynamically created or reserved)"
  fi
}
```

**How window labels appear in the codebase (verified):**

| Location | Label | Pattern |
|----------|-------|---------|
| `lib.rs` | `"onboarding"` | `WebviewWindowBuilder::new(&handle, "onboarding", ...)` |
| `hotkeys/manager.rs` | `"overlay"` | `WebviewWindowBuilder::new(&app, "overlay", ...)` |
| `hotkeys/manager.rs` | `"overlay"` | `WebviewWindowBuilder::new(&app, "overlay", ...)` (fallback creation) |
| `tray/menu.rs` | variable `label` | `WebviewWindowBuilder::new(app, label, ...)` |
| `tray/menu.rs` | `"history"`, `"settings"`, `"update"`, `"about"` | `open_or_focus_window(app, "history", ...)` etc. |

The `tray/menu.rs` call via `open_or_focus_window` uses a variable `label`, not a string literal. However, the function `open_or_focus_window` is only called from `tray/menu.rs` with literal strings — the grep on `open_or_focus_window` calls catches those. The `"main"` label in capabilities is reserved by Tauri's default window; it does not appear in code because we use `"app": { "windows": [] }` in `tauri.conf.json` (no main window), but Tauri still references it internally. The script reports it as informational, not a failure.

### 4.4 Check 3: Tauri Command Registration Completeness

Every function annotated with `#[tauri::command]` must appear in the `generate_handler![]` invocation in `lib.rs`.

```bash
check_command_registration() {
  header "Tauri command registration completeness"

  # Extract all #[tauri::command] function names.
  # The pattern: #[tauri::command] followed by a line with `pub async fn NAME`
  # or `pub fn NAME`.
  local defined_commands
  defined_commands=$(
    grep -A1 '#\[tauri::command\]' src-tauri/src/commands/*.rs src-tauri/src/updater/mod.rs \
    | grep -oE 'pub\s+(async\s+)?fn\s+[a-zA-Z_][a-zA-Z0-9_]*' \
    | sed 's/.*fn //' \
    | sort -u
  )

  # Extract registered commands from generate_handler![].
  # The handler spans multiple lines. Commands are listed as module::path::name.
  # We extract the last segment (function name).
  # Filter out 'generate_handler' itself, which matches the regex as
  # tauri::generate_handler but is the macro invocation, not a command.
  local registered_commands
  registered_commands=$(
    sed -n '/generate_handler!\[/,/\]/p' src-tauri/src/lib.rs \
    | grep -v '^\s*//' \
    | grep -oE '[a-zA-Z_][a-zA-Z0-9_]*(::[a-zA-Z_][a-zA-Z0-9_]*)+' \
    | sed 's/.*:://' \
    | grep -v 'generate_handler' \
    | sort -u
  )

  echo "  Defined:    $(echo "$defined_commands" | wc -l | tr -d ' ') commands"
  echo "  Registered: $(echo "$registered_commands" | wc -l | tr -d ' ') commands"

  # Find defined but not registered
  local unregistered
  unregistered=$(comm -23 <(echo "$defined_commands") <(echo "$registered_commands"))

  if [[ -z "$unregistered" ]]; then
    pass "All #[tauri::command] functions are registered in generate_handler![]"
  else
    fail "Commands defined but NOT registered in generate_handler![]:"
    echo "$unregistered" | while read -r cmd; do
      # Show which file defines it
      local file
      file=$(grep -rl "fn ${cmd}" src-tauri/src/commands/ src-tauri/src/updater/ 2>/dev/null | head -1)
      echo "    - $cmd (in $file)"
    done
  fi

  # Find registered but not defined (could indicate a stale registration)
  local orphan
  orphan=$(comm -13 <(echo "$defined_commands") <(echo "$registered_commands"))
  if [[ -n "$orphan" ]]; then
    fail "Commands registered in generate_handler![] but not defined with #[tauri::command]:"
    echo "$orphan" | while read -r cmd; do
      echo "    - $cmd"
    done
  fi
}
```

**Current state (verified):** 33 `#[tauri::command]` functions across `commands/*.rs` and `updater/mod.rs`. All 33 are listed in `generate_handler![]` in `lib.rs`.

### 4.5 Check 4: Frontend IPC Command Alignment

Every `invoke('command_name')` call in the frontend must correspond to a command registered in `generate_handler![]`.

```bash
check_frontend_ipc_alignment() {
  header "Frontend IPC command alignment"

  # Extract all invoke('name') calls from frontend .ts and .svelte files
  local frontend_commands
  frontend_commands=$(
    grep -rhoE "invoke\(\s*['\"][^'\"]+" src/ --include='*.ts' --include='*.svelte' \
    | sed "s/.*invoke([[:space:]]*['\"]//;s/['\"].*//" \
    | sort -u
  )

  # Extract registered commands from generate_handler![]
  # Filter out 'generate_handler' itself (the macro name, not a command).
  local registered_commands
  registered_commands=$(
    sed -n '/generate_handler!\[/,/\]/p' src-tauri/src/lib.rs \
    | grep -v '^\s*//' \
    | grep -oE '[a-zA-Z_][a-zA-Z0-9_]*(::[a-zA-Z_][a-zA-Z0-9_]*)+' \
    | sed 's/.*:://' \
    | grep -v 'generate_handler' \
    | sort -u
  )

  echo "  Frontend invocations: $(echo "$frontend_commands" | wc -l | tr -d ' ') unique commands"
  echo "  Registered handlers:  $(echo "$registered_commands" | wc -l | tr -d ' ') commands"

  # Find frontend calls that don't match any registered command
  local unmatched
  unmatched=$(comm -23 <(echo "$frontend_commands") <(echo "$registered_commands"))

  if [[ -z "$unmatched" ]]; then
    pass "All frontend invoke() calls match registered Rust commands"
  else
    fail "Frontend invoke() calls with no matching registered Rust command:"
    echo "$unmatched" | while read -r cmd; do
      local files
      files=$(grep -rl "invoke.*['\"]${cmd}['\"]" src/ --include='*.ts' --include='*.svelte' | head -3)
      echo "    - '$cmd' (called from: $files)"
    done
  fi

  # Informational: registered commands not called from frontend
  # (not a failure — commands may be called from Rust-side only or reserved)
  local uncalled
  uncalled=$(comm -13 <(echo "$frontend_commands") <(echo "$registered_commands"))
  if [[ -n "$uncalled" ]]; then
    local count
    count=$(echo "$uncalled" | wc -l | tr -d ' ')
    echo "  INFO: $count registered commands not invoked from frontend"
    echo "        (Not a failure — may be called from Rust or reserved)"
  fi
}
```

**Current state (verified):** The frontend calls `restart` (from `onboarding-view.svelte`) which is a Tauri built-in, not a custom command. It also calls `apply_shortcuts` (from `settings-panel.svelte`), `fix_accessibility_permission` (from `settings-panel.svelte`), `open_url` (from `settings-panel.svelte`), and `open_microphone_settings` (from `settings-panel.svelte`). The `restart` command is provided by `tauri-plugin-process` and won't appear in `generate_handler![]`. The script should filter out known Tauri built-in commands:

```bash
  # Filter out known Tauri built-in commands (provided by plugins, not generate_handler).
  # Add one command per line to the heredoc as new plugin commands are used.
  local builtins
  builtins=$(cat <<'BUILTINS'
restart
BUILTINS
  )
  unmatched=$(echo "$unmatched" | grep -vxF "$builtins" || true)
```

### 4.6 Check 5: CHANGELOG Entry Exists

For tag-triggered builds, verify the changelog contains a section for the version being released.

```bash
check_changelog_entry() {
  header "CHANGELOG entry exists"

  local version
  version=$(jq -r '.version' src-tauri/tauri.conf.json)

  if grep -q "^## \[$version\]" CHANGELOG.md; then
    pass "CHANGELOG.md has entry for version $version"
  else
    fail "CHANGELOG.md is missing an entry for version $version"
    echo "    Expected a line matching: ## [$version]"
  fi
}
```

This check runs unconditionally (even on non-tag builds) because it catches the problem earlier — before the developer pushes a tag.

### 4.7 Check 6: No Hardcoded Versions in Frontend

Scan `.svelte` and `.ts` files in `src/` for strings that look like hardcoded version numbers. The canonical version comes from `getVersion()` at runtime.

```bash
check_no_hardcoded_versions() {
  header "No hardcoded versions in frontend"

  local version
  version=$(jq -r '.version' src-tauri/tauri.conf.json)

  # Search for the current version string in frontend source files.
  # Exclude: type definitions, comments, import paths, changelog references.
  local matches
  matches=$(
    grep -rnE "(v?${version//./\\.})" src/ --include='*.svelte' --include='*.ts' \
    | grep -v '^\s*//' \
    | grep -v 'node_modules' \
    | grep -v '\.d\.ts:' \
    || true
  )

  if [[ -z "$matches" ]]; then
    pass "No hardcoded version '$version' found in frontend source"
  else
    fail "Hardcoded version '$version' found in frontend source files:"
    echo "$matches" | while read -r line; do
      echo "    $line"
    done
    echo "    Use getVersion() from @tauri-apps/api/app instead"
  fi
}
```

**Design note:** This check is version-specific — it looks for the *current* version string. This is intentional: a hardcoded `"0.5.0"` in a v0.6.3 build is harmless (likely dead code or a comment), but a hardcoded `"0.6.3"` would become stale at the next release.

### 4.8 Check 7: Cargo Test Gate

```bash
check_cargo_test() {
  header "Cargo test"

  if [[ "${CI_SKIP_CARGO_TEST:-}" == "1" ]]; then
    echo "  SKIP: CI_SKIP_CARGO_TEST=1 (already run in a separate step)"
    return
  fi

  if (cd src-tauri && cargo test 2>&1 | tee /tmp/ci-cargo-test.txt); then
    pass "cargo test passed"
  else
    fail "cargo test failed (see output above or /tmp/ci-cargo-test.txt)"
  fi
}
```

### 4.9 Check 8: Frontend Type Check Gate

```bash
check_frontend_types() {
  header "Frontend type check (npm run check)"

  if [[ "${CI_SKIP_NPM_CHECK:-}" == "1" ]]; then
    echo "  SKIP: CI_SKIP_NPM_CHECK=1 (already run in a separate step)"
    return
  fi

  if npm run check 2>&1 | tee /tmp/ci-npm-check.txt; then
    pass "npm run check passed"
  else
    fail "npm run check failed (see output above or /tmp/ci-npm-check.txt)"
  fi
}
```

### 4.10 Complete Script

The complete `scripts/ci-checks.sh` calls all checks in sequence and prints a summary:

```bash
#!/usr/bin/env bash
#
# CI Configuration Assertions for SottoASR
#
# Runs all configuration consistency checks. Exit code is non-zero
# if any check fails. Designed to run before the build step in CI.
#
# Usage:
#   ./scripts/ci-checks.sh           # Run all checks
#   CI_SKIP_CARGO_TEST=1 \
#   CI_SKIP_NPM_CHECK=1 \
#     ./scripts/ci-checks.sh         # Skip build/test checks (if run separately)

set -uo pipefail
# Note: not using set -e because we want to run all checks even if some fail.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_ROOT"

# --- Output helpers ---

FAILED=0
CHECKS_RUN=0
CHECKS_PASSED=0
CHECKS_SKIPPED=0

pass() {
  CHECKS_PASSED=$((CHECKS_PASSED + 1))
  echo "  PASS: $1"
}

fail() {
  FAILED=$((FAILED + 1))
  echo "  FAIL: $1"
}

skip() {
  CHECKS_SKIPPED=$((CHECKS_SKIPPED + 1))
  echo "  SKIP: $1"
}

header() {
  CHECKS_RUN=$((CHECKS_RUN + 1))
  echo ""
  echo "[$CHECKS_RUN] $1"
  echo "---"
}

# --- Check functions (defined in sections 4.2–4.9) ---

check_version_consistency() { ... }    # Section 4.2
check_capability_windows() { ... }     # Section 4.3
check_command_registration() { ... }   # Section 4.4
check_frontend_ipc_alignment() { ... } # Section 4.5
check_changelog_entry() { ... }        # Section 4.6
check_no_hardcoded_versions() { ... }  # Section 4.7
check_cargo_test() { ... }            # Section 4.8
check_frontend_types() { ... }        # Section 4.9

# --- Run all checks ---

echo "========================================"
echo " SottoASR CI Configuration Assertions"
echo "========================================"

check_version_consistency
check_capability_windows
check_command_registration
check_frontend_ipc_alignment
check_changelog_entry
check_no_hardcoded_versions
check_cargo_test
check_frontend_types

# --- Summary ---

echo ""
echo "========================================"
echo " Summary: $CHECKS_PASSED/$CHECKS_RUN passed, $FAILED failed, $CHECKS_SKIPPED skipped"
echo "========================================"

if [[ $FAILED -gt 0 ]]; then
  echo ""
  echo "CI assertions FAILED. Fix the issues above before merging."
  exit 1
else
  echo ""
  echo "All CI assertions passed."
  exit 0
fi
```

### 4.11 CI Workflow Integration

Add a new step in `.github/workflows/build-release.yml` after `npm ci` and before `cargo audit`:

```yaml
      - name: Install frontend dependencies
        run: npm ci

      # NEW: CI configuration assertions
      - name: Run CI assertions
        env:
          CI_SKIP_CARGO_TEST: "1"   # cargo test runs later (or add it here)
          CI_SKIP_NPM_CHECK: "1"    # npm check runs later (or add it here)
        run: |
          chmod +x scripts/ci-checks.sh
          ./scripts/ci-checks.sh

      - name: Type-check frontend
        run: npm run check

      - name: Run Rust tests
        working-directory: src-tauri
        run: cargo test

      - name: Audit Rust dependencies
        working-directory: src-tauri
        run: |
          cargo install cargo-audit --quiet
          cargo audit
```

**Step placement rationale:**

- The config assertion checks (version, capabilities, commands, IPC, changelog, hardcoded versions) require no build artifacts — they run against source files using `grep`, `jq`, `awk`, and `comm`. Placing them right after `npm ci` (which provides `node_modules` for the `jq` alternative if needed) means configuration problems surface in seconds, before expensive compilation begins.
- `cargo test` and `npm run check` are separated into their own steps for clearer CI output and independent timing visibility. The `CI_SKIP_*` env vars tell the assertion script not to duplicate them.
- The Rust cache step (`swatinem/rust-cache`) comes before this, so `cargo test` benefits from cached compilation.

**Full updated workflow step list:**

1. Checkout
2. Setup Node.js
3. Install Rust stable
4. Rust cache
5. Install frontend dependencies (`npm ci`)
6. **Run CI assertions** (new)
7. **Type-check frontend** (`npm run check`) (new)
8. **Run Rust tests** (`cargo test`) (new)
9. Audit Rust dependencies (`cargo audit`)
10. Audit npm dependencies
11. Import Apple Developer Certificate
12. Verify signing certificate
13. Extract changelog for this version
14. Build and release (`tauri-action`)

## 5. Edge Cases

### 5.1 Dynamic Window Labels

`tray/menu.rs` creates windows using a variable `label` passed to `WebviewWindowBuilder::new`. The script cannot extract labels from variable references. However, the call site is the `open_or_focus_window` helper, and all calls to that helper use string literals:
- `open_or_focus_window(app, "history", ...)`
- `open_or_focus_window(app, "settings", ...)`
- `open_or_focus_window(app, "update", ...)`
- `open_or_focus_window(app, "about", ...)`

The script greps both `WebviewWindowBuilder::new` (for direct calls) and `open_or_focus_window` (for calls through the helper). If a future developer adds a new window via `open_or_focus_window` with a variable label, the script will not catch it. Mitigation: the script reports any capabilities entries not found in code as informational, prompting investigation.

### 5.2 Conditional Compilation

Window creation or command definitions behind `#[cfg(...)]` flags (e.g., `#[cfg(target_os = "macos")]`) are still grepped by the script. This is correct behavior — if a command is conditionally compiled, it should still be in the capabilities for the platforms where it exists. Since SottoASR only targets macOS, all cfg-gated code compiles for the release build.

### 5.3 Tauri Built-in Commands

The frontend calls `invoke('restart')` (from `tauri-plugin-process`), which is not a custom command and does not appear in `generate_handler![]`. The IPC alignment check must filter out known built-in commands. The current list of built-ins to exclude:

- `restart` (from `tauri-plugin-process`)

If more plugin-provided commands are used in the future, they must be added to the exclusion list in the script.

### 5.4 Version String in Comments

The hardcoded-version check (`check_no_hardcoded_versions`) could match version strings in code comments. The script filters lines starting with `//` but does not handle block comments (`/* ... */`) or HTML comments (`<!-- ... -->`). This is an acceptable false-positive risk — a developer can verify and update the check if needed.

### 5.5 The `main` Window Label

The capabilities file includes `"main"` but no code creates a window with that label (the app uses `"app": { "windows": [] }` in `tauri.conf.json`). The `main` label is reserved by Tauri's internal architecture. The script treats this as informational, not a failure.

### 5.6 Duplicate Overlay Creation

The overlay window is created in two places in `hotkeys/manager.rs` with the same label `"overlay"` (primary and fallback creation). The script deduplicates labels with `sort -u`, so this is handled correctly.

### 5.7 Cargo.lock Parsing

The `Cargo.lock` version extraction uses `awk` to find the line after `name = "sottoasr"`. If the lockfile format changes (e.g., Cargo switches to a different TOML layout), this could break. The current Cargo.lock format (version 3) places `version` immediately after `name`, so the `awk` pattern is reliable. If it breaks, it will manifest as an empty string, which will cause a version mismatch — a safe failure mode.

### 5.8 BSD grep Compatibility

The `grep -P` flag (Perl-compatible regex) is **not** available on macOS BSD grep or on GitHub Actions `macos-latest` runners. All code blocks in this spec use `grep -oE` (POSIX extended regex) with `sed` for capture group extraction instead. This is the only portable approach without requiring `ggrep` via Homebrew.

For multiline function calls (where the label string is on a subsequent line from the function name), the spec uses `grep -A<N>` to capture context lines and then extracts quoted strings from the combined output:

```bash
grep -A3 'WebviewWindowBuilder::new(' src-tauri/src/*.rs src-tauri/src/**/*.rs 2>/dev/null \
  | grep -oE '"[a-z_]+"' | sed 's/"//g'
```

### 5.9 Website Version Badge

The `website/index.html` contains a `<span class="version-badge">v0.6.3</span>` that must be updated with each release (per the release process docs). The CI script does **not** check this because the website is not part of the app build. However, a developer could optionally add a website version check. This is left as a future enhancement — the release process documentation already covers it.

### 5.10 Overlap with Phase 3 Smoke Test Script

The Phase 3 pre-release smoke test script (`scripts/smoke-test.sh`) also checks version consistency, capabilities, changelog, and hardcoded versions. This creates partial duplication between the two scripts. The division of responsibility is:

- **`ci-checks.sh` (this spec):** Automated gate that runs in CI on every push/PR. Must be fully non-interactive, deterministic, and fast. Focuses on checks that can block a build.
- **`smoke-test.sh` (Phase 3):** Developer-facing pre-release checklist that runs locally before tagging a release. Includes interactive checks (e.g., "does the app launch?", "does the overlay appear?") that cannot run in CI.

For the overlapping automated checks (version consistency, capabilities, changelog, hardcoded versions), the accepted approach is to **tolerate the duplication** rather than extract shared functions. Rationale: the two scripts serve different audiences (CI bot vs. developer), may diverge in strictness over time, and the shared logic is simple enough that a shared function library would add coupling without meaningful savings. Both scripts should include a comment referencing the other to help maintainers keep them in sync.

## 6. File Changes

| File | Action | Description |
|------|--------|-------------|
| `scripts/ci-checks.sh` | Create | Main CI assertion script (all 8 checks) |
| `.github/workflows/build-release.yml` | Modify | Add "Run CI assertions", "Type-check frontend", and "Run Rust tests" steps |

No other files are created or modified. The script reads existing source files but does not alter them.

## 7. Testing Strategy

### 7.1 Local Verification

Run the script locally before committing:

```bash
chmod +x scripts/ci-checks.sh
./scripts/ci-checks.sh
```

Expected output: all checks pass on a clean `main` branch.

### 7.2 Negative Testing

Introduce deliberate failures to verify each check catches them:

| Check | How to break it | Expected output |
|-------|----------------|-----------------|
| Version consistency | Change version in `package.json` only | "FAIL: Version mismatch detected" |
| Capability windows | Remove `"update"` from `default.json` | "FAIL: Window labels in code but missing from capabilities" |
| Command registration | Comment out one line in `generate_handler![]` | "FAIL: Commands defined but NOT registered" |
| Frontend IPC | Add `invoke('nonexistent_command')` to a `.ts` file | "FAIL: Frontend invoke() calls with no matching registered Rust command" |
| Changelog entry | Change version to one without a changelog entry | "FAIL: CHANGELOG.md is missing an entry" |
| Hardcoded versions | Add `const V = "0.6.3"` to a `.ts` file | "FAIL: Hardcoded version found" |

### 7.3 CI Verification

After merging, trigger the workflow by pushing a tag (or use `workflow_dispatch`) and verify:
- The "Run CI assertions" step appears in the workflow run.
- It completes in under 10 seconds (static checks only, no compilation).
- The output shows all checks with pass/fail status.
- The build proceeds only if all checks pass.

### 7.4 Regression Protection

Each check corresponds to a real bug that shipped in a past release:

| Check | Prevents recurrence of |
|-------|----------------------|
| Version consistency | Wrong version in DMG filename or About screen |
| Capability windows | v0.6.0 update window blank due to missing capability (fixed in v0.6.1) |
| Command registration | Silent command failures from unregistered handlers |
| Frontend IPC | Runtime errors from renamed/removed commands |
| Changelog entry | Empty "What's New" on GitHub Release page |
| Hardcoded versions | Stale version strings in UI |

## 8. Security Considerations

- The script reads source files and JSON configs. It does not execute application code, access secrets, or modify any files.
- The script runs in the CI environment with the same permissions as the build step. No elevated permissions are needed.
- The `jq` tool is used to parse JSON. It is a read-only JSON processor with no side effects.
- No secrets, tokens, or credentials are accessed or logged by the script.

## 9. Cost Analysis

### 9.1 CI Time Impact

- Static checks (1-6): under 5 seconds. These use `grep`, `jq`, `awk`, `sed` — no compilation.
- `cargo test` (check 7): 30-90 seconds depending on cache state. Already recommended as a CI step; this formalizes it.
- `npm run check` (check 8): 5-15 seconds. Already recommended; this formalizes it.
- **Net impact on release builds:** Under 10 seconds additional (static checks only; test/check are not duplicated if run as separate steps).

### 9.2 Maintenance Burden

- **New window added:** Developer must add the label to `capabilities/default.json`. The CI check catches the omission.
- **New command added:** Developer must add it to `generate_handler![]`. The CI check catches the omission.
- **New Tauri plugin command used in frontend:** Developer must add it to the `builtins` exclusion list in the script. The CI check surfaces this as a false positive, which is the safe direction.
- **Version bumped:** No script changes needed — the script dynamically reads the current version from `package.json`.

### 9.3 Dependencies

The script requires:
- `bash` (3.2+, compatible with macOS default)
- `jq` (for JSON parsing)
- `grep`, `sed`, `awk`, `sort`, `comm` (standard Unix tools)
- `cargo` and `npm` (for checks 7 and 8, if not skipped)

All of these are available on `macos-latest` GitHub Actions runners. No additional `brew install` step is needed (`jq` is pre-installed on GitHub Actions macOS runners).

## 10. Implementation Tasks

- [ ] **Task 1: Create `scripts/ci-checks.sh` with the output framework.** Set up the script skeleton with `header`, `pass`, `fail`, `skip` helpers and the summary section. Verify it runs and exits 0 with no checks.

- [ ] **Task 2: Implement Check 1 (version consistency).** Add `check_version_consistency` function. Test locally by changing one version file and confirming the failure is detected.

- [ ] **Task 3: Implement Check 2 (capability window completeness).** Add `check_capability_windows` function. Use `grep -oE` + `sed` patterns (not `grep -P`) for BSD grep compatibility. Test by temporarily removing a window label from `default.json`.

- [ ] **Task 4: Implement Check 3 (command registration).** Add `check_command_registration` function. Test by commenting out one entry in `generate_handler![]`.

- [ ] **Task 5: Implement Check 4 (frontend IPC alignment).** Add `check_frontend_ipc_alignment` function with the `restart` built-in exclusion. Test by adding a fake `invoke('does_not_exist')` to a `.ts` file.

- [ ] **Task 6: Implement Check 5 (changelog entry).** Add `check_changelog_entry` function. Test by changing the version in `tauri.conf.json` to a version not in the changelog.

- [ ] **Task 7: Implement Check 6 (no hardcoded versions).** Add `check_no_hardcoded_versions` function. Test by adding a hardcoded version string to a `.ts` file.

- [ ] **Task 8: Implement Checks 7-8 (cargo test and npm check gates).** Add `check_cargo_test` and `check_frontend_types` with `CI_SKIP_*` environment variable support.

- [ ] **Task 9: Add CI workflow steps.** Modify `.github/workflows/build-release.yml` to add the "Run CI assertions", "Type-check frontend", and "Run Rust tests" steps between `npm ci` and `cargo audit`.

- [ ] **Task 10: Verify end-to-end.** Run `./scripts/ci-checks.sh` locally on a clean `main` branch and confirm all checks pass. Run negative tests per section 7.2. Push a test branch and confirm the workflow step executes correctly via `workflow_dispatch`.
