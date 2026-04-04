# Auto-Update Modal Audit

- **Version:** 1.1
- **Date:** 2026-04-04
- **Status:** Implemented

## Table of Contents

1. [Summary](#1-summary)
2. [Files Analysed](#2-files-analysed)
3. [Architecture Overview](#3-architecture-overview)
4. [Bug #1 — No HTTP Timeout (CRITICAL)](#4-bug-1--no-http-timeout-critical)
5. [Bug #2 — Missing Fallback for "Update Available" (HIGH)](#5-bug-2--missing-fallback-for-update-available-high)
6. [Bug #3 — Stale State Not Cleared (MEDIUM)](#6-bug-3--stale-state-not-cleared-medium)
7. [Bug #4 — Double Auto-Close Timer (LOW)](#7-bug-4--double-auto-close-timer-low)
8. [Bug #5 — No Download Stall Detection (MEDIUM)](#8-bug-5--no-download-stall-detection-medium)
9. [Endpoint & Release Verification](#9-endpoint--release-verification)
10. [Root Cause Analysis](#10-root-cause-analysis)
11. [Recommended Fixes](#11-recommended-fixes)

---

## 1. Summary

The auto-update modal gets stuck on "Checking for Updates..." and never transitions to another state. This audit examines the full update pipeline — Rust backend, Svelte frontend, Tauri plugin configuration, event system, and network endpoint — to identify every bug that contributes to or could cause this behaviour.

**Primary root cause:** The Tauri updater plugin (`tauri-plugin-updater` v2.10.0) is initialised without an HTTP timeout, and `reqwest` 0.13.2 has no default request timeout. If the network request to GitHub hangs for any reason, `updater.check().await` blocks forever. There is no timeout safety net on either the Rust or frontend side.

**Five additional bugs** of varying severity were found during the review.

---

## 2. Files Analysed

| File | Lines | Role |
|------|------:|------|
| `src-tauri/src/updater/mod.rs` | 277 | Core update logic, state, Tauri commands |
| `src/lib/components/update-view.svelte` | 449 | Update modal UI + state machine |
| `src/lib/utils/tauri.ts` | 244 | Frontend command wrappers |
| `src/update.ts` | 5 | Update window entry point |
| `update.html` | — | HTML shell for update window |
| `src-tauri/src/lib.rs` | 297 | Plugin registration, setup |
| `src-tauri/src/tray/menu.rs` | 351 | Tray menu, window management |
| `src-tauri/tauri.conf.json` | 45 | Updater endpoint + pubkey config |
| `src-tauri/capabilities/default.json` | 36 | Window labels + `updater:default` permission |
| `src-tauri/Cargo.toml` | 74 | `tauri-plugin-updater = "2"` dependency |
| `vite.config.ts` | — | Multi-page build includes `update.html` |
| `.github/workflows/build-release.yml` | 124 | Release CI (tags, signing, draft releases) |

---

## 3. Architecture Overview

```
User clicks "Check for Updates..." in tray menu
  │
  ▼
open_or_focus_window(app, "update", "update.html", ...)     [menu.rs:170]
  │
  ▼  (window created, Svelte mounts)
update-view.svelte onMount()                                 [update-view.svelte:24]
  │
  ├── Register 4 event listeners (lines 28-52)
  │     • update-available → step = 'available'
  │     • update-up-to-date → step = 'up_to_date'
  │     • update-download-progress → progress bar
  │     • update-check-error → step = 'error'
  │
  ├── getUpdateStatus() → check for cached state              [update-view.svelte:56]
  │     • restart_pending? → step = 'ready'
  │     • downloading? → step = 'downloading'
  │     • update_available? → step = 'available'
  │     • else → doCheck()
  │
  ▼
doCheck()                                                     [update-view.svelte:80]
  │
  ├── await checkAppUpdate()                                  [tauri.ts:233]
  │     │
  │     ▼  (Rust IPC)
  │   check_app_update()                                      [mod.rs:170]
  │     │
  │     ├── check_for_update()                                [mod.rs:122]
  │     │     │
  │     │     ├── app.updater()?.check().await   ◄── HTTP to GitHub
  │     │     │     │
  │     │     │     ├── Ok(Some(update)) → store state, emit "update-available"
  │     │     │     ├── Ok(None) → emit "update-up-to-date"
  │     │     │     └── Err(e) → emit "update-check-error", return Err
  │     │     │
  │     │     └── return Ok(())
  │     │
  │     └── read available_version from state → return Ok(ver)
  │
  ├── if (version) → do nothing, rely on event listener  ◄── BUG #2
  ├── else → fallback: step = 'up_to_date'
  └── catch → step = 'error'
```

---

## 4. Bug #1 — No HTTP Timeout (CRITICAL)

**Severity:** CRITICAL — directly causes the reported "stuck on checking" issue.

**Location:** `src-tauri/src/lib.rs:94` and `src-tauri/src/updater/mod.rs:127-128`

### The Problem

The Tauri updater plugin is initialised with a bare builder — no timeout:

```rust
// lib.rs:94
app.handle().plugin(tauri_plugin_updater::Builder::new().build())?;
```

The `check_for_update()` function calls `updater.check()` without a timeout:

```rust
// mod.rs:127-128
let updater = app.updater()?;
match updater.check().await {
```

**`tauri-plugin-updater` v2.10.0** initialises its internal `timeout` field to `None`. When `check()` builds the HTTP request, it only applies a timeout if one was set:

```rust
// tauri-plugin-updater source (UpdaterBuilder)
if let Some(timeout) = self.timeout {
    request = request.timeout(timeout);
}
```

Since `None` is the default, no timeout is passed to `reqwest`. **`reqwest` 0.13.2 does not set a default request timeout.** The request can hang indefinitely.

### Why It Causes the Stuck Modal

1. Frontend calls `await checkAppUpdate()` in `doCheck()` (line 84)
2. Rust calls `updater.check().await` — makes HTTP request to GitHub
3. If the request hangs (DNS, network, slow redirect), the await never resolves
4. `check_for_update()` never returns → no events are emitted
5. `checkAppUpdate()` never returns → the Promise never resolves
6. Frontend `step` stays as `'checking'` forever
7. No timeout mechanism exists on the frontend side either

### Conditions That Trigger This

- Any network condition that prevents the HTTP request from completing:
  - Corporate firewall blocking GitHub
  - DNS resolution failure that doesn't error but waits
  - Network transition (Wi-Fi switching, VPN reconnect)
  - GitHub rate limiting (responds very slowly instead of rejecting)
  - macOS Gatekeeper or network extension interference with unsigned dev builds
- Running the app immediately after boot (network stack not fully ready)
- System sleep/wake — if the `tokio::time::sleep` interval fires immediately after wake but network isn't ready yet

### Evidence

- **`tauri-plugin-updater` v2.10.0**: `timeout` field defaults to `None` (confirmed via docs.rs source)
- **`reqwest` 0.13.2**: No default request timeout (documented in reqwest API)
- **Known Tauri issues**: [#11675](https://github.com/tauri-apps/tauri/issues/11675) (timeout parameter misinterpreted), [#2372](https://github.com/tauri-apps/plugins-workspace/issues/2372) (timeout reuse between check and download)
- **No frontend timeout**: `doCheck()` has no `setTimeout` or `AbortController` safety net

---

## 5. Bug #2 — Missing Fallback for "Update Available" (HIGH)

**Severity:** HIGH — can cause stuck modal if event delivery fails.

**Location:** `src/lib/components/update-view.svelte:84-87`

### The Problem

When `checkAppUpdate()` returns a version string (update is available), the code does nothing:

```typescript
// update-view.svelte:84-87
const version = await checkAppUpdate();
if (version) {
  // Event listener will handle transition to 'available'.
}
```

The state transition relies **entirely** on the `'update-available'` event listener (line 28-33). If the event doesn't arrive, `step` remains `'checking'`.

### Why It's Normally OK (But Fragile)

The event is emitted by Rust (line 147) **before** the command returns (line 178). Both the event and the command response travel through the Tauri IPC bridge. In practice, the event usually arrives before or shortly after the command response.

However, there is **no guarantee** of ordering between the event channel and the command response channel. The event and the command response are delivered through different mechanisms:
- Events: `app.emit()` → webview event system → JS event callback
- Commands: return value → IPC response → JS Promise resolve

If a race condition causes the command response to arrive but the event to be delayed or dropped, the modal has no fallback and stays on "checking."

### Contrast with "No Update" Case

The `else` branch (no update) has proper fallback handling:

```typescript
} else {
  // Event listener should fire 'update-up-to-date', but handle fallback.
  if (step === 'checking') {
    step = 'up_to_date';    // ← Fallback works!
    startAutoClose();
  }
}
```

The "update available" case should have equivalent fallback logic.

---

## 6. Bug #3 — Stale State Not Cleared (MEDIUM)

**Severity:** MEDIUM — causes incorrect UI, not stuck modal.

**Location:** `src-tauri/src/updater/mod.rs:150-153`

### The Problem

When `updater.check()` returns `Ok(None)` (app is up to date), the stored state is NOT cleared:

```rust
// mod.rs:150-153
Ok(None) => {
    log::info!("App is up to date");
    let _ = app.emit("update-up-to-date", ());
    Ok(())
    // BUG: Does NOT reset available_version, update_available, release_notes
}
```

### Impact

1. Background checker runs → finds update v0.6.3 → stores state
2. User updates to v0.6.3 → restarts app
3. User opens update modal
4. `getUpdateStatus()` returns `update_available: true, version: "0.6.3"` (stale!)
5. Modal shows "Update Available: v0.6.3" — but user is already on v0.6.3

The stale state persists in memory until the next check clears it (by finding a newer update) or the app restarts.

### Fix

The `Ok(None)` branch should clear the cached state:

```rust
Ok(None) => {
    let state = app.state::<UpdateState>();
    *state.available_version.lock().await = None;
    *state.release_notes.lock().await = None;
    state.update_available.store(false, Ordering::SeqCst);
    crate::tray::menu::refresh_tray_for_update(app, None);
    let _ = app.emit("update-up-to-date", ());
    Ok(())
}
```

---

## 7. Bug #4 — Double Auto-Close Timer (LOW)

**Severity:** LOW — causes faster-than-expected auto-close, no stuck state.

**Location:** `src/lib/components/update-view.svelte:88-92` and `35-38`

### The Problem

When no update is available, two code paths both call `startAutoClose()`:

1. **Event listener** (line 35-38):
   ```typescript
   listen('update-up-to-date', () => {
     step = 'up_to_date';
     startAutoClose();  // ← call #1
   });
   ```

2. **Fallback in doCheck()** (line 89-92):
   ```typescript
   if (step === 'checking') {
     step = 'up_to_date';
     startAutoClose();  // ← call #2
   }
   ```

`startAutoClose()` creates a `setInterval` and assigns it to `autoCloseTimer`. The first call's interval is overwritten (leaked) by the second call. Result: the timer counts down at double speed (window closes in ~2s instead of ~4s).

### Fix

Guard `startAutoClose()` to be idempotent:

```typescript
function startAutoClose() {
    if (autoCloseTimer) return;  // ← Add guard
    autoCloseSeconds = 4;
    autoCloseTimer = setInterval(() => { ... }, 1000);
}
```

---

## 8. Bug #5 — No Download Stall Detection (MEDIUM)

**Severity:** MEDIUM — stuck on "downloading" with no recovery.

**Location:** `src/lib/components/update-view.svelte:100-112`

### The Problem

```typescript
async function handleDownload() {
    step = 'downloading';
    // ...
    await performAppUpdate();  // Can hang if download stalls
    step = 'ready';
}
```

If the download stalls (network drops mid-download, GitHub CDN issue), `performAppUpdate()` blocks forever. The progress bar stops updating but there's no mechanism to detect a stall or time out.

Additionally, `do_download_and_install()` in Rust (line 233) makes a **second** `updater.check()` call to get fresh URLs, which itself has no timeout (same as Bug #1).

---

## 9. Endpoint & Release Verification

All endpoint and release artifacts are correctly configured and accessible:

| Check | Result |
|-------|--------|
| Endpoint URL | `https://github.com/juanqui/sottoasr/releases/latest/download/latest.json` |
| HTTP response | 302 → `v0.6.2/latest.json` (correct redirect) |
| `latest.json` content | Valid JSON, version `0.6.2`, correct platform entries |
| Signature present | Yes (`darwin-aarch64` + `darwin-aarch64-app`) |
| Download URL | Points to `SottoASR_aarch64.app.tar.gz` (exists) |
| Public key in config | Matches signing key format |
| `createUpdaterArtifacts` | `true` in `tauri.conf.json` |
| `updater:default` permission | Granted in `capabilities/default.json` |
| Release status | Published (not draft) |
| CI workflow | Correct: tags trigger build, signs, creates draft release |

**The endpoint is not the problem.** The infrastructure is correctly configured.

---

## 10. Root Cause Analysis

### Most Likely Scenario (Bug #1)

The user opens the update modal. `doCheck()` fires and calls `checkAppUpdate()`, which calls `updater.check().await`. The HTTP request to GitHub hangs due to transient network conditions. Since there is no timeout at any level (Tauri plugin, reqwest, frontend), the modal stays on "Checking for Updates..." indefinitely.

This is particularly likely in these scenarios:
- **Dev mode** (`cargo tauri dev`): The app may have different network behaviour; the Vite dev server is running; the app bundle is not signed
- **After system sleep/wake**: Network stack may not be fully ready when the background checker fires
- **Restricted networks**: Corporate firewalls, VPNs, or macOS network extensions may interfere

### Unlikely but Possible (Bug #2)

If the network request succeeds and returns an update, but the `'update-available'` event is somehow not delivered to the frontend, the modal stays on "checking" because the `if (version)` branch does nothing.

### Contributing Factor (Bug #3)

Stale state could cause confusing UI states that might be mistaken for "stuck" — e.g., showing an update for the current version.

---

## 11. Recommended Fixes

### Priority 1: Add HTTP timeout (fixes Bug #1)

**Rust side** — wrap `updater.check().await` in a `tokio::time::timeout()`:

```rust
// mod.rs — check_for_update()
use tokio::time::{timeout, Duration};

let updater = app.updater()?;
let check_result = timeout(Duration::from_secs(30), updater.check())
    .await
    .map_err(|_| "Update check timed out — please check your internet connection")?;

match check_result {
    // ... existing match arms
}
```

**Frontend side** — add a safety-net timeout in `doCheck()`:

```typescript
async function doCheck() {
    step = 'checking';
    errorMessage = '';

    const timeoutId = setTimeout(() => {
        if (step === 'checking') {
            errorMessage = 'Update check timed out. Please try again.';
            step = 'error';
        }
    }, 35_000);  // 35s — slightly longer than Rust-side 30s

    try {
        const version = await checkAppUpdate();
        // ... existing logic
    } catch (err: any) {
        errorMessage = err?.toString() || 'Check failed';
        step = 'error';
    } finally {
        clearTimeout(timeoutId);
    }
}
```

### Priority 2: Add fallback for "update available" (fixes Bug #2)

```typescript
const version = await checkAppUpdate();
if (version) {
    // Fallback: transition directly if event hasn't arrived yet.
    // Give the event listener 500ms to fire (it usually arrives first).
    setTimeout(async () => {
        if (step === 'checking') {
            const status = await getUpdateStatus();
            availableVersion = status.version ?? version;
            releaseNotes = status.release_notes ?? '';
            step = 'available';
        }
    }, 500);
}
```

### Priority 3: Clear stale state (fixes Bug #3)

Add state cleanup to the `Ok(None)` branch in `check_for_update()`:

```rust
Ok(None) => {
    log::info!("App is up to date");
    let state = app.state::<UpdateState>();
    *state.available_version.lock().await = None;
    *state.release_notes.lock().await = None;
    state.update_available.store(false, Ordering::SeqCst);
    crate::tray::menu::refresh_tray_for_update(app, None);
    let _ = app.emit("update-up-to-date", ());
    Ok(())
}
```

### Priority 4: Guard auto-close (fixes Bug #4)

```typescript
function startAutoClose() {
    if (autoCloseTimer) return;
    // ... rest unchanged
}
```

### Priority 5: Add download stall detection (fixes Bug #5)

Track the last progress event timestamp and timeout if stalled for >60 seconds.

---

## Appendix: Dependency Versions

| Crate / Package | Version |
|-----------------|---------|
| `tauri-plugin-updater` | 2.10.0 |
| `reqwest` (updater dep) | 0.13.2 |
| `@tauri-apps/plugin-updater` | ^2 |
| `tauri` | 2.x |
| Rust toolchain | stable |
