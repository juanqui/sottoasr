#!/usr/bin/env bash
# Pre-Release Smoke Tests for SottoASR
# Runs automated checks (version consistency, build, lint, tests) and optional
# interactive checks (permissions, recording, paste) before tagging a release.
#
# Usage:
#   ./scripts/pre-release-check.sh                        # Full run
#   ./scripts/pre-release-check.sh --auto-only             # Skip interactive
#   ./scripts/pre-release-check.sh --version 0.7.0         # Override version
#   ./scripts/pre-release-check.sh --auto-only --version 0.7.0
set -uo pipefail

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
pass() {
  AUTO_PASS=$((AUTO_PASS + 1))
  printf " ${GREEN}✓${RESET}  %s\n" "$1"
}

fail() {
  AUTO_FAIL=$((AUTO_FAIL + 1))
  printf " ${RED}✗${RESET}  %s\n" "$1"
}

warn() {
  INT_WARN=$((INT_WARN + 1))
  printf " ${YELLOW}⚠${RESET}  %s\n" "$1"
}

int_pass() {
  INT_PASS=$((INT_PASS + 1))
  printf " ${GREEN}✓${RESET}  %s\n" "$1"
}

int_fail() {
  INT_FAIL=$((INT_FAIL + 1))
  printf " ${RED}✗${RESET}  %s\n" "$1"
}

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
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_ROOT"

# ── Prerequisite checks ───────────────────────
MISSING_TOOLS=""
for tool in python3 cargo npm; do
  if ! command -v "$tool" &>/dev/null; then
    MISSING_TOOLS="$MISSING_TOOLS $tool"
  fi
done

if [ -n "$MISSING_TOOLS" ]; then
  echo "Error: required tools not found on PATH:$MISSING_TOOLS"
  echo "Install them before running pre-release checks."
  exit 2
fi

# ── Determine target version ──────────────────
if [ -z "$TARGET_VERSION" ]; then
  TARGET_VERSION=$(python3 -c "
import json
print(json.load(open('src-tauri/tauri.conf.json'))['version'])
")
fi
VERSION="$TARGET_VERSION"

printf "\n${BOLD}=== Pre-Release Smoke Tests for SottoASR ===${RESET}\n"
printf "Target version: %s\n" "$VERSION"

# ── Pre-check: clean working directory ────────
DIRTY=$(git status --porcelain 2>/dev/null || true)
if [ -n "$DIRTY" ]; then
  printf "\n ${YELLOW}⚠${RESET}  Working directory has uncommitted changes:\n"
  echo "$DIRTY" | head -10
  printf "    (Consider committing or stashing before a release check.)\n"
fi

printf "\n${BOLD}── Automated Checks ────────────────────────${RESET}\n\n"

# ── Check 1: Version consistency ───────────────
V_PKG=$(python3 -c "import json; print(json.load(open('package.json'))['version'])")
V_LOCK_ROOT=$(python3 -c "import json; print(json.load(open('package-lock.json'))['version'])")
V_LOCK_INNER=$(python3 -c "import json; print(json.load(open('package-lock.json'))['packages']['']['version'])")
V_TAURI=$(python3 -c "import json; print(json.load(open('src-tauri/tauri.conf.json'))['version'])")
V_CARGO=$(python3 -c "import tomllib; print(tomllib.load(open('src-tauri/Cargo.toml','rb'))['package']['version'])")
V_CARGO_LOCK=$(grep -A1 'name = "sottoasr"' src-tauri/Cargo.lock | grep 'version' | sed 's/version = "\(.*\)"/\1/' | tr -d ' ')

ALL_MATCH=true
for pair in \
  "package.json:$V_PKG" \
  "package-lock.json (root):$V_LOCK_ROOT" \
  "package-lock.json (packages):$V_LOCK_INNER" \
  "tauri.conf.json:$V_TAURI" \
  "Cargo.toml:$V_CARGO" \
  "Cargo.lock:$V_CARGO_LOCK"; do
  FILE="${pair%%:*}"
  VER="${pair#*:}"
  if [ "$VER" != "$VERSION" ]; then
    ALL_MATCH=false
  fi
done

if [ "$ALL_MATCH" = true ]; then
  pass "Version consistency: all 5 files (6 values) match ($VERSION)"
else
  fail "Version mismatch detected:"
  for pair in \
    "package.json:$V_PKG" \
    "package-lock.json (root):$V_LOCK_ROOT" \
    "package-lock.json (packages):$V_LOCK_INNER" \
    "tauri.conf.json:$V_TAURI" \
    "Cargo.toml:$V_CARGO" \
    "Cargo.lock:$V_CARGO_LOCK"; do
    FILE="${pair%%:*}"
    VER="${pair#*:}"
    if [ "$VER" = "$VERSION" ]; then
      printf "      ${GREEN}✓${RESET} %s = %s\n" "$FILE" "$VER"
    else
      printf "      ${RED}✗${RESET} %s = %s (expected %s)\n" "$FILE" "$VER" "$VERSION"
    fi
  done
fi

# ── Check 2: Capability completeness ──────────
VITE_WINDOWS=$(python3 -c "
import re
content = open('vite.config.ts').read()
m = re.search(r'input:\s*\{([^}]+)\}', content)
if m:
    for key in re.findall(r'(\w+)\s*:', m.group(1)):
        print(key)
" | sort)

CAP_WINDOWS=$(python3 -c "
import json
caps = json.load(open('src-tauri/capabilities/default.json'))
for w in caps['windows']:
    print(w)
")

MISSING_CAPS=""
for w in $VITE_WINDOWS; do
  if ! echo "$CAP_WINDOWS" | grep -qx "$w"; then
    MISSING_CAPS="$MISSING_CAPS $w"
  fi
done

WINDOW_COUNT=$(echo "$VITE_WINDOWS" | wc -l | tr -d ' ')
if [ -z "$MISSING_CAPS" ]; then
  pass "Capability completeness: all $WINDOW_COUNT windows listed"
else
  fail "Windows missing from capabilities/default.json:$MISSING_CAPS"
  printf "      Current capabilities windows: %s\n" "$(echo "$CAP_WINDOWS" | tr '\n' ' ')"
fi

# ── Check 3: CHANGELOG entry ──────────────────
if grep -q "## \[$VERSION\]" CHANGELOG.md; then
  pass "CHANGELOG has entry for $VERSION"
else
  fail "CHANGELOG.md missing entry for [$VERSION]"
  printf "      Found versions: "
  grep -oE '## \[[0-9]+\.[0-9]+\.[0-9]+\]' CHANGELOG.md | head -5 | sed 's/## \[//;s/\]//' | tr '\n' ' '
  printf "\n"
fi

# ── Check 4: Website version badge ────────────
if [ ! -f "website/index.html" ]; then
  warn "Website version badge: website/index.html not found (skipped)"
else
  BADGE_VERSION=$(grep -oE 'version-badge">v[0-9]+\.[0-9]+\.[0-9]+' website/index.html | \
    sed 's/version-badge">v//' || true)

  if [ "$BADGE_VERSION" = "$VERSION" ]; then
    pass "Website version badge matches (v$VERSION)"
  else
    fail "Website version badge mismatch: found v$BADGE_VERSION, expected v$VERSION"
  fi
fi

# ── Check 5: Build ────────────────────────────
printf " ...  cargo build (running)\r"
BUILD_START=$(date +%s)
exit_code=0
(cd src-tauri && cargo build) > /tmp/sotto-smoke-build.txt 2>&1 || exit_code=$?
BUILD_END=$(date +%s)
BUILD_ELAPSED=$((BUILD_END - BUILD_START))

if [ "$exit_code" -eq 0 ]; then
  pass "cargo build succeeded (${BUILD_ELAPSED}s)"
else
  fail "cargo build failed (${BUILD_ELAPSED}s) — see /tmp/sotto-smoke-build.txt"
  tail -20 /tmp/sotto-smoke-build.txt | sed 's/^/      /'
fi

# ── Check 6: Clippy ───────────────────────────
printf " ...  cargo clippy (running)\r"
CLIPPY_START=$(date +%s)
exit_code=0
(cd src-tauri && cargo clippy --all-targets -- -D warnings) > /tmp/sotto-smoke-clippy.txt 2>&1 || exit_code=$?
CLIPPY_END=$(date +%s)
CLIPPY_ELAPSED=$((CLIPPY_END - CLIPPY_START))

if [ "$exit_code" -eq 0 ]; then
  pass "cargo clippy clean (${CLIPPY_ELAPSED}s)"
else
  fail "cargo clippy found warnings/errors (${CLIPPY_ELAPSED}s) — see /tmp/sotto-smoke-clippy.txt"
  tail -20 /tmp/sotto-smoke-clippy.txt | sed 's/^/      /'
fi

# ── Check 7: Frontend type check ─────────────
printf " ...  npm run check (running)\r"
exit_code=0
npm run check > /tmp/sotto-smoke-check.txt 2>&1 || exit_code=$?

if [ "$exit_code" -eq 0 ]; then
  pass "npm run check passed"
else
  fail "npm run check failed — see /tmp/sotto-smoke-check.txt"
  tail -20 /tmp/sotto-smoke-check.txt | sed 's/^/      /'
fi

# ── Check 8: Rust tests ──────────────────────
printf " ...  cargo test (running)\r"
TEST_START=$(date +%s)
exit_code=0
(cd src-tauri && cargo test --no-default-features --features custom-protocol,llm-cleanup) > /tmp/sotto-smoke-test.txt 2>&1 || exit_code=$?
TEST_END=$(date +%s)
TEST_ELAPSED=$((TEST_END - TEST_START))

if [ "$exit_code" -eq 0 ]; then
  TEST_COUNT=$(grep -oE '[0-9]+ test(s)? passed' /tmp/sotto-smoke-test.txt | head -1 || true)
  if [ -z "$TEST_COUNT" ]; then
    # Try the "test result: ok. X passed" format
    TEST_COUNT=$(grep -oE 'test result: ok\. [0-9]+ passed' /tmp/sotto-smoke-test.txt | grep -oE '[0-9]+ passed' | head -1 || true)
  fi
  if [ -n "$TEST_COUNT" ]; then
    pass "cargo test passed ($TEST_COUNT, ${TEST_ELAPSED}s)"
  else
    pass "cargo test passed (${TEST_ELAPSED}s)"
  fi
else
  fail "cargo test failed (${TEST_ELAPSED}s) — see /tmp/sotto-smoke-test.txt"
  tail -20 /tmp/sotto-smoke-test.txt | sed 's/^/      /'
fi

# ── Check 9: No hardcoded versions ───────────
ESCAPED_VER=$(echo "$VERSION" | sed 's/\./\\./g')
HARDCODED=$(grep -rn "v${ESCAPED_VER}" src/ \
  --include='*.svelte' --include='*.ts' 2>/dev/null \
  | grep -v '//.*v[0-9]' | grep -v 'http' || true)

if [ -z "$HARDCODED" ]; then
  pass "No hardcoded versions in frontend"
else
  fail "Hardcoded version 'v$VERSION' found in frontend source:"
  echo "$HARDCODED" | while IFS= read -r line; do
    printf "      %s\n" "$line"
  done
fi

# ── Check 10: Sidecar script ─────────────────
SIDECAR="src-tauri/sidecar/llm_cleanup.py"
if [ ! -f "$SIDECAR" ]; then
  fail "Sidecar script not found: $SIDECAR"
else
  exit_code=0
  python3 -c "
import py_compile, sys
try:
    py_compile.compile('$SIDECAR', doraise=True)
except py_compile.PyCompileError as e:
    print(str(e))
    sys.exit(1)
" 2>&1 || exit_code=$?

  if [ "$exit_code" -eq 0 ]; then
    pass "Sidecar script is valid Python"
  else
    fail "Sidecar script has syntax errors"
  fi
fi

# ── Interactive checks ────────────────────────
if [ "$AUTO_ONLY" = "false" ]; then
  printf "\n${BOLD}── Interactive Checks ──────────────────────${RESET}\n\n"

  # Launch the app for interactive testing
  TAURI_DEV_PID=""
  printf "  Launching SottoASR via cargo tauri dev...\n"
  printf "  (This may take a moment on first run.)\n\n"
  (cd src-tauri && cargo tauri dev) > /tmp/sotto-smoke-dev.txt 2>&1 &
  TAURI_DEV_PID=$!

  # Give it time to start
  printf "  Waiting for app to launch"
  for i in $(seq 1 30); do
    if ! kill -0 "$TAURI_DEV_PID" 2>/dev/null; then
      printf "\n"
      printf "  ${RED}App process exited unexpectedly. See /tmp/sotto-smoke-dev.txt${RESET}\n"
      TAURI_DEV_PID=""
      break
    fi
    printf "."
    sleep 2
  done
  printf "\n\n"

  if [ -n "$TAURI_DEV_PID" ]; then
    printf "  SottoASR should now be running in your menu bar.\n"
    ask_user "  Is the app visible in the menu bar?" && rc=$? || rc=$?
    if [ "$rc" -ne 0 ]; then
      printf "  ${YELLOW}Check /tmp/sotto-smoke-dev.txt for errors.${RESET}\n"
      warn "App launch: not confirmed"
    fi
  fi

  printf "\n"

  # ── Check 11: Permission status ──────────────
  printf "  Checking permissions...\n"

  # Try to query TCC database for Accessibility
  ACC_STATUS=""
  ACC_STATUS=$(sqlite3 \
    "$HOME/Library/Application Support/com.apple.TCC/TCC.db" \
    "SELECT auth_value FROM access WHERE service='kTCCServiceAccessibility' AND client='com.sottoasr.app';" \
    2>/dev/null || true)

  case "$ACC_STATUS" in
    2) printf "    Accessibility permission: ${GREEN}granted${RESET}\n" ;;
    0) printf "    Accessibility permission: ${RED}denied${RESET}\n" ;;
    *) printf "    Accessibility permission: ${YELLOW}unable to query TCC database${RESET}\n"
       printf "    Check manually: System Settings > Privacy & Security > Accessibility\n" ;;
  esac

  # Try to query TCC database for Microphone
  MIC_STATUS=""
  MIC_STATUS=$(sqlite3 \
    "$HOME/Library/Application Support/com.apple.TCC/TCC.db" \
    "SELECT auth_value FROM access WHERE service='kTCCServiceMicrophone' AND client='com.sottoasr.app';" \
    2>/dev/null || true)

  case "$MIC_STATUS" in
    2) printf "    Microphone permission: ${GREEN}granted${RESET}\n" ;;
    0) printf "    Microphone permission: ${RED}denied${RESET}\n" ;;
    *) printf "    Microphone permission: ${YELLOW}unable to query (will prompt on first use)${RESET}\n" ;;
  esac

  ask_user "  Are both permissions granted?" && rc=$? || rc=$?
  if [ "$rc" -eq 0 ]; then
    int_pass "Permissions: confirmed by user"
  elif [ "$rc" -eq 1 ]; then
    int_fail "Permissions: user reported issue"
  else
    warn "Permissions: skipped"
  fi

  # ── Check 12: ASR model available ────────────
  MODEL_DIR="$HOME/Library/Application Support/FluidAudio/Models"
  if [ -d "$MODEL_DIR" ]; then
    MODEL_SIZE=$(du -sh "$MODEL_DIR" 2>/dev/null | cut -f1)
    MODEL_COUNT=$(find "$MODEL_DIR" -type f | wc -l | tr -d ' ')
    printf "  FluidAudio models found: %s files, %s\n" "$MODEL_COUNT" "$MODEL_SIZE"
    int_pass "FluidAudio models found ($MODEL_SIZE)"
  else
    printf "  FluidAudio model directory not found at:\n"
    printf "    %s\n" "$MODEL_DIR"
    printf "    Models are downloaded on first use (~500 MB).\n"
    ask_user "  Continue without models?" && rc=$? || rc=$?
    if [ "$rc" -eq 0 ]; then
      warn "ASR model: not found (skipped)"
    else
      int_fail "ASR model: not found"
    fi
  fi

  # ── Check 13: Recording smoke test ───────────
  printf "\n  Recording smoke test:\n"
  printf "    1. Press your configured hotkey (default: Cmd+Shift+Space)\n"
  printf "    2. Say \"hello world\"\n"
  printf "    3. Release the hotkey\n"
  printf "    4. Check that a transcription appeared in the overlay\n\n"

  ask_user "  Did the transcription appear?" && rc=$? || rc=$?
  if [ "$rc" -eq 0 ]; then
    int_pass "Recording smoke test: user confirmed"
  elif [ "$rc" -eq 1 ]; then
    int_fail "Recording smoke test: user reported failure"
  else
    warn "Recording smoke test: skipped"
  fi

  # ── Check 14: Paste verification ─────────────
  printf "\n  Paste verification:\n"
  printf "    1. Open TextEdit (or any text editor)\n"
  printf "    2. Place your cursor in the document\n"
  printf "    3. Press your hotkey, say a short phrase, release\n"
  printf "    4. Check that the transcribed text appeared at the cursor\n\n"

  ask_user "  Did transcribed text appear at the cursor?" && rc=$? || rc=$?
  if [ "$rc" -eq 0 ]; then
    int_pass "Paste verification: user confirmed"
  elif [ "$rc" -eq 1 ]; then
    int_fail "Paste verification: user reported failure"
  else
    warn "Paste verification: skipped"
  fi

  # ── Check 15: Settings round-trip ────────────
  printf "\n  Settings round-trip:\n"
  printf "    1. Open Settings from the tray menu (or press Cmd+,)\n"
  printf "    2. Change a setting (e.g., toggle a checkbox or change the hotkey)\n"
  printf "    3. Close the Settings window\n"
  printf "    4. Reopen Settings\n"
  printf "    5. Check that your change was preserved\n\n"

  ask_user "  Did the setting persist?" && rc=$? || rc=$?
  if [ "$rc" -eq 0 ]; then
    int_pass "Settings round-trip: user confirmed"
  elif [ "$rc" -eq 1 ]; then
    int_fail "Settings round-trip: user reported failure"
  else
    warn "Settings round-trip: skipped"
  fi

  # ── Cleanup: stop the dev server ──────────────
  if [ -n "$TAURI_DEV_PID" ] && kill -0 "$TAURI_DEV_PID" 2>/dev/null; then
    printf "\n  Stopping SottoASR...\n"
    kill "$TAURI_DEV_PID" 2>/dev/null || true
    wait "$TAURI_DEV_PID" 2>/dev/null || true
  fi
fi

# ── Summary ───────────────────────────────────
printf "\n${BOLD}── Summary ─────────────────────────────────${RESET}\n\n"

TOTAL_PASS=$((AUTO_PASS + INT_PASS))
TOTAL_FAIL=$((AUTO_FAIL + INT_FAIL))
TOTAL_WARN=$INT_WARN

printf " Automated:   %d passed, %d failed\n" "$AUTO_PASS" "$AUTO_FAIL"
if [ "$AUTO_ONLY" = "false" ]; then
  printf " Interactive:  %d passed, %d failed, %d warning(s)\n" "$INT_PASS" "$INT_FAIL" "$INT_WARN"
fi
printf " Total:       %d passed, %d failed, %d warning(s)\n" "$TOTAL_PASS" "$TOTAL_FAIL" "$TOTAL_WARN"

echo ""
if [ "$AUTO_FAIL" -gt 0 ]; then
  printf "${RED}${BOLD}One or more automated checks failed. Fix the issues above before tagging a release.${RESET}\n"
  exit 1
else
  printf "${GREEN}${BOLD}All automated checks passed.${RESET}\n"
  exit 0
fi
