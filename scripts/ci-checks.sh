#!/usr/bin/env bash
# CI Configuration Assertions for SottoASR
# Catches drift between version files, capability configs, and IPC bindings.
# Exit non-zero if any check fails; runs all checks before exiting.
set -uo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
BOLD='\033[1m'
RESET='\033[0m'

FAILURES=0

pass() {
  echo -e "  ${GREEN}✓${RESET} $1"
}

fail() {
  echo -e "  ${RED}✗${RESET} $1"
  FAILURES=$((FAILURES + 1))
}

# Resolve repo root (script lives in scripts/)
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# ─────────────────────────────────────────────────────────────
# Check 1: Version Consistency
# ─────────────────────────────────────────────────────────────
echo -e "\n${BOLD}Check 1: Version Consistency${RESET}"

V_PKG=$(jq -r '.version' "$REPO_ROOT/package.json")
V_LOCK_ROOT=$(jq -r '.version' "$REPO_ROOT/package-lock.json")
V_LOCK_PKG=$(jq -r '.packages[""].version' "$REPO_ROOT/package-lock.json")
V_TAURI=$(jq -r '.version' "$REPO_ROOT/src-tauri/tauri.conf.json")

# Cargo.toml: extract version from [package] section only
V_CARGO=$(sed -n '/^\[package\]/,/^\[/p' "$REPO_ROOT/src-tauri/Cargo.toml" \
  | grep -E '^version\s*=' | head -1 \
  | sed 's/.*"\(.*\)".*/\1/')

# Cargo.lock: version line immediately after name = "sottoasr"
V_CARGOLOCK=$(awk '/^name = "sottoasr"/{getline; print}' "$REPO_ROOT/src-tauri/Cargo.lock" \
  | sed 's/.*"\(.*\)".*/\1/')

CANONICAL="$V_PKG"
ALL_MATCH=true

for pair in \
  "package.json:$V_PKG" \
  "package-lock.json (root):$V_LOCK_ROOT" \
  "package-lock.json (packages):$V_LOCK_PKG" \
  "tauri.conf.json:$V_TAURI" \
  "Cargo.toml:$V_CARGO" \
  "Cargo.lock:$V_CARGOLOCK"; do
  FILE="${pair%%:*}"
  VER="${pair#*:}"
  if [ "$VER" = "$CANONICAL" ]; then
    pass "$FILE = $VER"
  else
    fail "$FILE = $VER (expected $CANONICAL)"
    ALL_MATCH=false
  fi
done

if [ "$ALL_MATCH" = true ]; then
  pass "All 6 version sources match: $CANONICAL"
fi

# ─────────────────────────────────────────────────────────────
# Check 2: Capability Window Completeness
# ─────────────────────────────────────────────────────────────
echo -e "\n${BOLD}Check 2: Capability Window Completeness${RESET}"

# Extract window labels from Rust code.
# Labels often appear on a DIFFERENT line than the function call, so we use
# grep -A to grab context and then extract quoted strings from the block.

# WebviewWindowBuilder::new(handle, "label", ...) — label is on next line(s)
BUILDER_LABELS=$(
  grep -rA3 'WebviewWindowBuilder::new(' "$REPO_ROOT/src-tauri/src/" \
    | grep -v '^\s*//' \
    | grep -oE '"[a-z_]+"' \
    | tr -d '"' \
    | sort -u
)

# open_or_focus_window(app, "label", ...) — label is the first quoted string after the call
OPEN_LABELS=$(
  grep -rA2 'open_or_focus_window(' "$REPO_ROOT/src-tauri/src/" \
    | grep -v '^\s*//' \
    | grep -v 'pub fn open_or_focus_window' \
    | grep -v 'label:' \
    | grep -oE '"[a-z_]+"' \
    | tr -d '"' \
    | sort -u
)

# Combine and deduplicate, filtering out non-label strings (html filenames etc.)
# Window labels are simple words; filter out anything containing a dot (like "overlay.html")
ALL_RUST_WINDOWS=$(
  printf '%s\n' $BUILDER_LABELS $OPEN_LABELS \
    | grep -v '\.' \
    | sort -u \
    | grep -v '^$'
)

# Extract windows from capabilities JSON
CAP_WINDOWS=$(jq -r '.windows[]' "$REPO_ROOT/src-tauri/capabilities/default.json" | sort -u)

MISSING_FROM_CAP=false
for w in $ALL_RUST_WINDOWS; do
  if echo "$CAP_WINDOWS" | grep -qx "$w"; then
    pass "Window '$w' listed in capabilities"
  else
    fail "Window '$w' created in Rust but MISSING from capabilities/default.json"
    MISSING_FROM_CAP=true
  fi
done

if [ "$MISSING_FROM_CAP" = false ]; then
  pass "All Rust-created windows are in capabilities"
fi

# ─────────────────────────────────────────────────────────────
# Check 3: Command Registration Completeness
# ─────────────────────────────────────────────────────────────
echo -e "\n${BOLD}Check 3: Command Registration Completeness${RESET}"

# Find all #[tauri::command] functions
# The fn signature is on the line after #[tauri::command], so use grep -A1.
# Context lines from grep -A have a "filename-" prefix, so match fn anywhere in the line.
COMMAND_FNS=$(
  grep -rA1 '#\[tauri::command\]' "$REPO_ROOT/src-tauri/src/" \
    | grep -E '(pub\s+)?(async\s+)?fn\s+[a-z_]+' \
    | grep -oE 'fn [a-z_]+' \
    | sed 's/fn //' \
    | sort -u
)

# Extract handler registrations from generate_handler![]
HANDLER_FNS=$(
  sed -n '/generate_handler!\[/,/\]/p' "$REPO_ROOT/src-tauri/src/lib.rs" \
    | grep -v '^\s*//' \
    | grep -v 'generate_handler' \
    | grep -oE '[a-z_:]+::[a-z_]+|[a-z_]+' \
    | while IFS= read -r entry; do
        # Extract just the function name (last segment after ::)
        echo "$entry" | grep -oE '[a-z_]+$'
      done \
    | sort -u
)

MISSING_HANDLERS=false
for fn in $COMMAND_FNS; do
  if echo "$HANDLER_FNS" | grep -qx "$fn"; then
    pass "Command '$fn' registered in generate_handler!"
  else
    fail "Command '$fn' has #[tauri::command] but is NOT in generate_handler!"
    MISSING_HANDLERS=true
  fi
done

if [ "$MISSING_HANDLERS" = false ]; then
  pass "All #[tauri::command] functions are registered"
fi

# ─────────────────────────────────────────────────────────────
# Check 4: Frontend IPC Alignment
# ─────────────────────────────────────────────────────────────
echo -e "\n${BOLD}Check 4: Frontend IPC Alignment${RESET}"

# Extract all invoke('command_name') calls from frontend
INVOKE_CMDS=$(
  grep -rnoE "invoke\(['\"][a-z_]+['\"]" "$REPO_ROOT/src/" \
    --include='*.ts' --include='*.svelte' \
    | grep -oE "['\"][a-z_]+['\"]" \
    | tr -d "\"'" \
    | sort -u
)

# Known plugin commands that are NOT custom handlers (provided by Tauri plugins)
PLUGIN_COMMANDS="restart"

MISSING_IPC=false
for cmd in $INVOKE_CMDS; do
  # Skip known plugin commands
  if echo "$PLUGIN_COMMANDS" | grep -qw "$cmd"; then
    pass "invoke('$cmd') — plugin command (skipped)"
    continue
  fi
  if echo "$HANDLER_FNS" | grep -qx "$cmd"; then
    pass "invoke('$cmd') maps to registered handler"
  else
    fail "invoke('$cmd') called in frontend but NOT registered in generate_handler!"
    MISSING_IPC=true
  fi
done

if [ "$MISSING_IPC" = false ]; then
  pass "All frontend invoke() calls have matching handlers"
fi

# ─────────────────────────────────────────────────────────────
# Check 5: CHANGELOG Entry
# ─────────────────────────────────────────────────────────────
echo -e "\n${BOLD}Check 5: CHANGELOG Entry${RESET}"

if grep -qE "^## \[$CANONICAL\]" "$REPO_ROOT/CHANGELOG.md"; then
  pass "CHANGELOG.md has entry for [$CANONICAL]"
else
  fail "CHANGELOG.md is missing entry for [$CANONICAL]"
fi

# ─────────────────────────────────────────────────────────────
# Check 6: No Hardcoded Versions in Frontend
# ─────────────────────────────────────────────────────────────
echo -e "\n${BOLD}Check 6: No Hardcoded Versions in Frontend${RESET}"

# Escape dots for regex
ESCAPED_VER=$(echo "$CANONICAL" | sed 's/\./\\./g')

HARDCODED=$(grep -rn "$ESCAPED_VER" "$REPO_ROOT/src/" \
  --include='*.ts' --include='*.svelte' \
  || true)

if [ -z "$HARDCODED" ]; then
  pass "No hardcoded version '$CANONICAL' found in src/"
else
  fail "Hardcoded version '$CANONICAL' found in frontend source:"
  echo "$HARDCODED" | while IFS= read -r line; do
    echo "    $line"
  done
fi

# ─────────────────────────────────────────────────────────────
# Summary
# ─────────────────────────────────────────────────────────────
echo ""
if [ "$FAILURES" -eq 0 ]; then
  echo -e "${GREEN}${BOLD}All CI checks passed.${RESET}"
  exit 0
else
  echo -e "${RED}${BOLD}$FAILURES check(s) failed.${RESET}"
  exit 1
fi
