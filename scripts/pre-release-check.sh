#!/usr/bin/env bash
# Pre-Release Smoke Tests for SottoASR
# Runs the automated checks PR CI runs (.github/workflows/ci.yml), checks that
# every version file matches the release version, then offers optional
# interactive checks (permissions, recording, paste) before tagging a release.
# It uses the installed node_modules; run `npm ci` first for a clean install.
#
# Usage:
#   ./scripts/pre-release-check.sh                          # Full run
#   ./scripts/pre-release-check.sh --auto-only              # Skip interactive
#   ./scripts/pre-release-check.sh --version X.Y.Z          # Override version
#   ./scripts/pre-release-check.sh --auto-only --version X.Y.Z
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

# run_step "label" log-name command [args...]: run one check and keep its output
# in /tmp/sotto-smoke-<log-name>.txt.
run_step() {
  local label="$1" log="/tmp/sotto-smoke-$2.txt" start elapsed exit_code=0
  shift 2
  printf " ...  %s (running)\r" "$label"
  start=$(date +%s)
  "$@" > "$log" 2>&1 || exit_code=$?
  elapsed=$(( $(date +%s) - start ))
  printf "\033[K"
  if [ "$exit_code" -eq 0 ]; then
    pass "$label (${elapsed}s)"
  else
    fail "$label failed (${elapsed}s) — see $log"
    tail -20 "$log" | sed 's/^/      /'
  fi
}

# Run a command in src-tauri/, like the CI steps with working-directory: src-tauri.
in_tauri() {
  (cd src-tauri && "$@")
}

# Same as the CI audit step: the zip ignores in src-tauri/.cargo/audit.toml hold
# only while zip is not built for macOS.
audit_rust() {
  local zip_dependents
  if ! grep -qx 'name = "zip"' Cargo.lock; then
    echo "zip is no longer in Cargo.lock. Delete its ignores from src-tauri/.cargo/audit.toml and the zip checks in .github/workflows and scripts/pre-release-check.sh."
  else
    zip_dependents=$(cargo tree --locked -i zip --target aarch64-apple-darwin) || return 1
    if [ -n "$zip_dependents" ]; then
      printf '%s\n' "$zip_dependents"
      echo "zip is now built for aarch64-apple-darwin, so the zip ignores in src-tauri/.cargo/audit.toml no longer hold. Update zip and remove them."
      return 1
    fi
  fi
  cargo audit
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
for tool in python3 cargo npm node jq; do
  if ! command -v "$tool" &>/dev/null; then
    MISSING_TOOLS="$MISSING_TOOLS $tool"
  fi
done
# Cargo subcommands for the audit and packaging checks
if command -v cargo &>/dev/null; then
  for subcommand in audit tauri; do
    if ! cargo "$subcommand" --version &>/dev/null; then
      MISSING_TOOLS="$MISSING_TOOLS cargo-$subcommand"
    fi
  done
fi

if [ -n "$MISSING_TOOLS" ]; then
  echo "Error: required tools not found on PATH:$MISSING_TOOLS"
  echo "Install them before running pre-release checks."
  case "$MISSING_TOOLS" in *cargo-audit*) echo "  cargo install cargo-audit --version 0.22.2 --locked" ;; esac
  case "$MISSING_TOOLS" in *cargo-tauri*) echo "  cargo install tauri-cli --version '^2' --locked" ;; esac
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

# ── Version consistency with the release version ──
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

# ── PR CI commands, in the order ci.yml runs them ──
PARAKEET_FEATURES="custom-protocol,asr-parakeet"
UNSIGNED_BUNDLE_CONFIG='{"bundle":{"createUpdaterArtifacts":false,"macOS":{"signingIdentity":"-"}}}'

run_step "Configuration assertions (scripts/ci-checks.sh)" ci-checks bash scripts/ci-checks.sh
run_step "npm run build" npm-build npm run build
run_step "npm run check" npm-check npm run check
run_step "npm test" npm-test npm test
run_step "cargo test (CoreML backend)" cargo-test in_tauri cargo test --locked
run_step "cargo check (Parakeet backend)" parakeet-check \
  in_tauri cargo check --locked --no-default-features --features "$PARAKEET_FEATURES"
run_step "Resampler tests (Parakeet backend)" parakeet-test \
  in_tauri cargo test --locked --no-default-features --features "$PARAKEET_FEATURES" --lib audio::resample
run_step "Pinned Swift correctness regressions" swift-regressions python3 scripts/check-swift-regressions.py
run_step "cargo clippy" clippy in_tauri cargo clippy --locked --all-targets -- -D warnings
run_step "Rust dependency audit" cargo-audit in_tauri audit_rust
run_step "npm audit --omit=dev" npm-audit npm audit --omit=dev
run_step "Sidecar Python tests" sidecar-tests python3 -m unittest discover -s src-tauri/sidecar -p 'test_*.py'
run_step "Script Python tests" script-tests python3 -m unittest discover -s scripts -p 'test_*.py'
# CI=true as on GitHub runners: the DMG bundler then skips its Finder layout script.
run_step "Unsigned macOS packaging" package \
  env CI=true cargo tauri build --target aarch64-apple-darwin --bundles app,dmg --config "$UNSIGNED_BUNDLE_CONFIG"

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

  # ── Permission status ────────────────────────
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

  # ── ASR model available ──────────────────────
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

  # ── Recording smoke test ─────────────────────
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

  # ── Paste verification ───────────────────────
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

  # ── Settings round-trip ──────────────────────
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
