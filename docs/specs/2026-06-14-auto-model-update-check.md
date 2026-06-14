# Auto-check AI model updates with tray icon indicator

- **Version:** 2.0
- **Date:** 2026-06-14
- **Status:** Implemented

---

## 1. Summary

Extend the existing app update system to also auto-check for AI model updates and show the tray icon visual indicator when a new model version is available. Today, model updates are only detected on manual check from the Settings panel. Users who enable LLM cleanup have no awareness that a better model exists until they open Settings.

## 2. Problem Statement

When the LLM cleanup feature is enabled, the app checks for model updates only when the user opens Settings and calls `check_llm_update()` manually. The tray icon's visual indicator (the dot/badge on `tray-icon-updateTemplate.png`) only activates for app updates — model updates are invisible at the system level. Users miss model improvements because there is no passive notification surface.

## 3. Design Overview

Reuse the existing periodic update checker loop (`updater/mod.rs`) to also check for model updates on the same ~4-hour cadence. When a model update is detected, activate the same tray icon visual indicator used for app updates. App updates take priority over model updates for the tray icon.

```
┌─────────────────────────────────────────────────────────────────┐
│ Existing: start_update_checker() (4h loop)                       │
│  → check_for_update() (app)                                      │
│  → [NEW] llm::engine::check_model_update() (model)               │
│  → refresh_tray_from_state() (reads UpdateState, single call)    │
└─────────────────────────────────────────────────────────────────┘
```

**Key design decisions:**

- **Dependency direction:** Model check logic lives in `llm::engine` (not `updater`), so both `commands::llm` (Tauri IPC) and `updater` (background loop) call the same function. This avoids `updater` → `commands` which crosses a layer boundary.
- **Single tray refresh function:** `refresh_tray_from_state()` reads `UpdateState` directly — one canonical entry point, no parameter passing, no TOCTOU races.
- **Error propagation:** `check_for_model_update()` returns `Err` on failure (matching the app-check pattern), so the loop's `log::warn!()` branch is live code.
- **Event naming:** `llm-update-available` / `llm-update-up-to-date` — uses the `llm-` prefix to match all other LLM events and avoid collision with ASR `model-*` events.

## 4. Detailed Design

### 4.1. Extend `UpdateState` with model update fields

**File:** `src-tauri/src/updater/mod.rs`

Add two fields to `UpdateState`:

```rust
pub struct UpdateState {
    pub update_available: AtomicBool,
    pub available_version: Mutex<Option<String>>,
    pub release_notes: Mutex<Option<String>>,
    pub downloading: AtomicBool,
    pub restart_pending: AtomicBool,
    // NEW — model update tracking
    pub model_update_available: AtomicBool,
    pub model_update_consecutive_errors: AtomicU32,
}
```

The `model_update_consecutive_errors` counter implements a TTL: after 3 consecutive errors (~12 hours), the `model_update_available` flag is cleared to prevent a permanently stale indicator if HuggingFace becomes unreachable.

**State management note:** LLM operational state (`llm_engine`, `llm_pid`, `llm_last_status`) lives in `AppState` (`state.rs`). Model *update availability* lives in `UpdateState` because it is consumed by the tray icon and update window — the same surfaces that consume app update state. This is an intentional split: `AppState` holds process/runtime state, `UpdateState` holds UI notification state. To avoid a split truth, `get_llm_status()` (see §4.9) reads `model_update_available` from `UpdateState` rather than hardcoding `false`.

### 4.2. Extract `check_model_update()` to `llm::engine`

**File:** `src-tauri/src/llm/engine.rs`

Extract the core update-check logic so both the Tauri command and the background loop share the same implementation:

```rust
/// Check if a newer model is available on HuggingFace.
/// Returns Ok(true) if update available, Ok(false) if up to date, Err on failure.
/// Does NOT load the MLX model — only reads refs/main and calls repo_info().
pub async fn check_model_update(app: &AppHandle) -> Result<bool, String> {
    // Fast path: reuse existing sidecar if running (avoids process spawn).
    // Acquire lock, take sidecar, DROP GUARD, do work, re-acquire to store.
    // This minimizes lock hold time so the cleanup pipeline is not blocked.
    {
        let mut guard = state.llm_engine.lock().await;
        if let Some(llm) = guard.take() {
            drop(guard); // Release lock BEFORE spawn_blocking
            let result = tokio::task::spawn_blocking(move || {
                llm.request_raw(&serde_json::json!({"action": "check_update"}))
            }).await
                .map_err(|e| format!("Check panicked: {}", e))?
                .map_err(|e| format!("Sidecar error: {}", e))?;
            let available = result.get("update_available")
                .and_then(|u| u.as_bool())
                .unwrap_or(false);
            // Re-acquire lock to store sidecar back.
            let mut guard = state.llm_engine.lock().await;
            *guard = Some(llm);
            return Ok(available);
        }
    }

    // Slow path: spawn temporary sidecar (no MLX/model load).
    tokio::task::spawn_blocking(move || {
        let mut e = LlmEngine::spawn()?;
        let result = e.request_raw(&serde_json::json!({"action": "check_update"}))?;
        e.quit();
        Ok(result.get("update_available")
            .and_then(|u| u.as_bool())
            .unwrap_or(false))
    }).await
        .map_err(|e| format!("Check panicked: {}", e))?
}
```

**Lock contention fix:** The guard is dropped BEFORE `spawn_blocking`, so the cleanup pipeline can acquire the lock during the ~10s `repo_info()` call. The existing `ensure_running()` already uses this take→drop→work→restore pattern.

### 4.3. Update `commands::llm::check_llm_update()` to delegate

**File:** `src-tauri/src/commands/llm.rs`

Rewire the Tauri command to call the extracted function:

```rust
#[tauri::command]
pub async fn check_llm_update(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    engine::check_model_update(&app).await
}
```

This eliminates code duplication and ensures the Tauri IPC path and the background loop path use identical logic.

### 4.4. Add `check_for_model_update()` to updater module

**File:** `src-tauri/src/updater/mod.rs`

```rust
/// Check if a newer AI model is available on HuggingFace.
/// Returns Err on persistent failures so the loop can log warnings.
async fn check_for_model_update(app: &AppHandle) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // 1. Compile-time feature guard (runtime check, matches lib.rs:195 pattern)
    if !crate::llm::engine::is_feature_compiled() {
        let updater = app.state::<UpdateState>();
        updater.model_update_available.store(false, Ordering::SeqCst);
        return Ok(());
    }

    // 2. Runtime settings guard — use try_lock to match read_auto_check_setting() pattern
    let state = app.try_state::<crate::state::AppState>()
        .ok_or("AppState not available")?;
    let settings = state.settings.try_lock()
        .map_err(|_| "Settings lock contended — skipping model check this cycle")?;
    let llm_enabled = settings.llm_cleanup_enabled;
    drop(settings);

    if !llm_enabled {
        let updater = app.state::<UpdateState>();
        updater.model_update_available.store(false, Ordering::SeqCst);
        updater.model_update_consecutive_errors.store(0, Ordering::SeqCst);
        return Ok(());
    }

    // 3. Delegate to llm::engine (NOT commands::llm — avoids layer crossing)
    let result = crate::llm::engine::check_model_update(app).await;

    let updater = app.state::<UpdateState>();
    match result {
        Ok(available) => {
            // Reset error counter on success
            updater.model_update_consecutive_errors.store(0, Ordering::SeqCst);
            updater.model_update_available.store(available, Ordering::SeqCst);
            if available {
                log::info!("AI model update available");
                let _ = tauri::Emitter::emit(app, "llm-update-available", ());
            } else {
                let _ = tauri::Emitter::emit(app, "llm-update-up-to-date", ());
            }
        }
        Err(e) => {
            // Increment error counter; clear flag after 3 consecutive errors (~12h)
            let errors = updater.model_update_consecutive_errors.fetch_add(1, Ordering::SeqCst) + 1;
            if errors >= 3 {
                updater.model_update_available.store(false, Ordering::SeqCst);
                updater.model_update_consecutive_errors.store(0, Ordering::SeqCst);
                log::warn!("Model update check failed {} times — clearing stale flag: {}", errors, e);
            } else {
                log::debug!("Model update check failed ({} / 3): {}", errors, e);
            }
            // Propagate error so loop can log warning (matches app-check pattern)
            return Err(e.into());
        }
    }

    Ok(())
}
```

**Error propagation:** Returns `Err` on failure so the loop's `log::warn!()` branch is live code. This matches the existing `check_for_update()` pattern.

### 4.5. Integrate into the periodic update loop

**File:** `src-tauri/src/updater/mod.rs` (in `start_update_checker()`)

```rust
pub fn start_update_checker(app: &AppHandle) {
    if is_app_translocated() {
        log::warn!("App is running from an App Translocation path — auto-update is disabled.");
        return;
    }

    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;

        loop {
            let auto_check = read_auto_check_setting(&handle).unwrap_or(true);
            if auto_check {
                // Panic isolation: wrap each check in tokio::task::spawn
                // so a panic in one check doesn't kill the entire loop.
                let app_check = tokio::spawn({
                    let h = handle.clone();
                    async move { check_for_update(&h).await }
                });
                let model_check = tokio::spawn({
                    let h = handle.clone();
                    async move { check_for_model_update(&h).await }
                });

                if let Err(e) = app_check.await {
                    log::warn!("App update check panicked: {}", e);
                }
                if let Err(e) = model_check.await {
                    log::warn!("Model update check panicked: {}", e);
                }

                // Refresh tray from canonical state (reads UpdateState directly)
                crate::tray::menu::refresh_tray_from_state(&handle);
            } else {
                log::debug!("Auto-update check disabled by user setting");
            }
            tokio::time::sleep(std::time::Duration::from_secs(4 * 60 * 60)).await;
        }
    });
}
```

**Panic isolation:** Each check runs in its own `tokio::spawn` so a panic in one doesn't kill the loop. JoinErrors are logged as warnings.

**Tray refresh:** `refresh_tray_from_state()` reads `UpdateState` directly — no parameters, no TOCTOU.

### 4.6. Canonical tray refresh: `refresh_tray_from_state()`

**File:** `src-tauri/src/tray/menu.rs`

Replace `refresh_tray_for_update()` and `refresh_tray_for_restart()` with a single function that reads `UpdateState` directly:

```rust
/// Single canonical tray refresh. Reads UpdateState directly.
/// Call from anywhere — periodic loop, manual check, download complete.
pub fn refresh_tray_from_state(app: &AppHandle) {
    let state = match app.try_state::<crate::updater::UpdateState>() {
        Some(s) => s,
        None => return,
    };

    let app_update = state.update_available.load(Ordering::SeqCst);
    let model_update = state.model_update_available.load(Ordering::SeqCst);
    let restart = state.restart_pending.load(Ordering::SeqCst);
    let version = state.available_version.lock().unwrap().clone();

    // Icon: any update → show indicator
    let has_any_update = app_update || model_update || restart;
    let _ = set_tray_icon(app, has_any_update);

    // Menu priority: restart > app update > model update > normal
    let tray_state = if restart {
        TrayState::RestartPending
    } else if let Some(v) = version {
        let with_model = if model_update {
            format!("{} (+ model)", v)
        } else {
            v
        };
        TrayState::UpdateAvailable(with_model)
    } else if model_update {
        TrayState::ModelUpdateAvailable
    } else {
        TrayState::Normal
    };
    let _ = build_tray_menu(app, tray_state);
}
```

**Both-updates label:** When both app and model updates are available, the menu shows `"Update Available — v{version} (+ model)"` so the user knows both exist.

**Deprecated functions:** `refresh_tray_for_update()` and `refresh_tray_for_restart()` are removed. All callers (periodic loop, `check_for_update()`, `perform_app_update()`) call `refresh_tray_from_state()`.

### 4.7. Update `TrayState` enum and menu labels

**File:** `src-tauri/src/tray/menu.rs`

```rust
enum TrayState {
    Normal,
    UpdateAvailable(String),   // version string, may include " (+ model)" suffix
    RestartPending,
    ModelUpdateAvailable,
}
```

Menu label logic:

```rust
let update_label = match &state {
    TrayState::Normal => "Check for Updates...",
    TrayState::UpdateAvailable(version) => &format!("Update Available — {}", version),
    TrayState::RestartPending => "Restart to Update",
    TrayState::ModelUpdateAvailable => "AI Model Update Available...",
};
```

**Window title:** When the `"check_updates"` handler fires, the window title is chosen based on `TrayState`:

```rust
"check_updates" => {
    // Open update window with context-appropriate title
    let title = if matches!(state, TrayState::ModelUpdateAvailable) {
        "SottoASR — Model Update"
    } else {
        "SottoASR — Software Update"
    };
    open_or_focus_window(app, "update", "update.html", title, 420.0, 480.0);
}
```

### 4.8. Wire tray refresh into all code paths

**File:** `src-tauri/src/updater/mod.rs`

- `check_for_update()`: After writing to `UpdateState`, call `refresh_tray_from_state(app)` instead of `refresh_tray_for_update()`. This ensures manual "Check for Updates" from the tray menu also refreshes correctly.
- `perform_app_update()`: After setting `restart_pending = true`, call `refresh_tray_from_state(app)` instead of `refresh_tray_for_restart()`.

### 4.9. Sync `get_llm_status()` with `UpdateState`

**File:** `src-tauri/src/commands/llm.rs`

Replace the hardcoded `update_available: false` with a read from `UpdateState`:

```rust
#[tauri::command]
pub fn get_llm_status(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<LlmStatus, String> {
    // ... existing logic ...

    // Read model update availability from UpdateState (single source of truth)
    let model_update_available = app.try_state::<crate::updater::UpdateState>()
        .map(|u| u.model_update_available.load(Ordering::SeqCst))
        .unwrap_or(false);

    Ok(LlmStatus {
        // ...
        update_available: model_update_available, // Was: false
        // ...
    })
}
```

### 4.10. Clear `model_update_available` after successful model update

**File:** `src-tauri/src/commands/llm.rs`

In `update_llm_model()`, after `download::download_model()` succeeds:

```rust
#[tauri::command]
pub async fn update_llm_model(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    // ... shutdown sidecar, download model ...
    download::download_model(&app).await?;

    // Clear the model update flag now that the model is current
    if let Some(updater) = app.try_state::<crate::updater::UpdateState>() {
        updater.model_update_available.store(false, Ordering::SeqCst);
        updater.model_update_consecutive_errors.store(0, Ordering::SeqCst);
    }
    // Refresh tray to remove indicator
    crate::tray::menu::refresh_tray_from_state(&app);
    Ok(())
}
```

### 4.11. Extend the update window to show model update status

**File:** `src/lib/components/update-view.svelte`

Add model states to the existing state machine:

```typescript
type UpdateStep =
  | 'checking' | 'up_to_date' | 'available' | 'downloading' | 'ready' | 'error'
  | 'model_available' | 'model_downloading' | 'model_ready' | 'model_error'; // NEW
```

**State transitions:**

| Trigger | From | To |
|---------|------|-----|
| Mount, app current + model update | initial | `model_available` |
| Click "Update Model" | `model_available` | `model_downloading` |
| `llm-download-complete` event | `model_downloading` | `model_ready` |
| `llm-download-error` event | `model_downloading` | `model_error` |
| Click "Done" | `model_ready` | `up_to_date` (auto-close) |
| Click "Try Again" | `model_error` | `model_available` |

**Model download UI:** The `model_downloading` state shows an indeterminate progress spinner (the existing `download_model()` does not emit byte-level progress events). A 60s stall detection timer fires `model_error` if no `llm-download-complete` or `llm-download-error` event arrives within 60s.

```svelte
{#if step === 'model_downloading'}
  <div class="state-downloading">
    <Spinner />
    <h2>Updating AI Model</h2>
    <p>Downloading the latest cleanup model (~233 MB)...</p>
  </div>
{/if}
```

### 4.12. Extend `get_update_status()` and TypeScript interface

**File:** `src-tauri/src/updater/mod.rs`

```rust
UpdateStatus {
    // ... existing fields ...
    model_update_available: state.model_update_available.load(Ordering::SeqCst),
}
```

**File:** `src/lib/utils/tauri.ts`

```typescript
export interface UpdateStatus {
  // ... existing fields ...
  model_update_available: boolean;
}
```

### 4.13. "Auto-check for updates" setting hint

**File:** `src/lib/components/settings-panel.svelte`

Make the hint conditional on LLM availability:

```svelte
<span class="toggle-hint">
  {#if llmStatus?.available}
    Check for app and model updates periodically
  {:else}
    Check for new versions periodically
  {/if}
</span>
```

### 4.14. Frontend events

**Event names** (use `llm-` prefix, NOT `model-`):

| Event | Emitted When | Payload |
|-------|-------------|---------|
| `llm-update-available` | Model check returns `true` | `()` |
| `llm-update-up-to-date` | Model check returns `false` | `()` |

The Settings panel can listen for these events to update the "Update Available" badge in the LLM section without waiting for the next `refreshLlmStatus()` call.

## 5. Edge Cases

| Edge Case | Handling |
|-----------|----------|
| LLM feature not compiled | `is_feature_compiled()` returns `false` at top of `check_for_model_update()` — clears flag, returns `Ok(())` |
| LLM cleanup disabled in settings | `check_for_model_update()` clears `model_update_available` and error counter, returns `Ok(())` |
| Model not downloaded yet | Sidecar's `check_update_available()` returns `false` when no local revision — no false positive |
| Sidecar spawn fails | `check_model_update()` returns `Err` — error counter increments; flag cleared after 3 failures |
| Both app AND model updates available | Icon shows indicator; menu shows `"Update Available — v{version} (+ model)"` |
| App translocated | `start_update_checker()` returns early — both checks skipped |
| `auto_check_updates` disabled | Both checks skipped |
| User enables LLM cleanup mid-cycle | Next 4-hour cycle picks it up |
| HF API timeout (10s) | Returns `Err` — error counter increments |
| Settings lock contended | `try_lock()` returns `Err` — check skipped this cycle, logged as warning |
| Panic in check | `tokio::spawn` isolates panic — loop continues, JoinError logged |
| `update_llm_model()` succeeds | `model_update_available` cleared immediately, tray refreshed |
| 3+ consecutive errors (~12h) | `model_update_available` cleared to prevent stale indicator |
| Manual "Check for Updates" from tray | Calls `check_for_update()` → writes `UpdateState` → `refresh_tray_from_state()` reads full state (includes model flag) |

## 6. File Changes

| File | Action | Description |
|------|--------|-------------|
| `src-tauri/src/updater/mod.rs` | Modify | Add model fields to `UpdateState`; add `check_for_model_update()`; integrate into loop with panic isolation; replace tray refresh calls with `refresh_tray_from_state()`; extend `get_update_status()` |
| `src-tauri/src/tray/menu.rs` | Modify | Add `ModelUpdateAvailable` to `TrayState`; add `refresh_tray_from_state()` (replaces `refresh_tray_for_update()` and `refresh_tray_for_restart()`); update menu labels; dynamic window title |
| `src-tauri/src/llm/engine.rs` | Modify | Extract `check_model_update()` from `commands::llm`; drop guard before `spawn_blocking` to reduce lock contention |
| `src-tauri/src/commands/llm.rs` | Modify | Delegate `check_llm_update()` to `engine::check_model_update()`; sync `get_llm_status()` with `UpdateState`; clear flag in `update_llm_model()` |
| `src/lib/utils/tauri.ts` | Modify | Add `model_update_available` to `UpdateStatus` |
| `src/lib/components/update-view.svelte` | Modify | Add `model_available`, `model_downloading`, `model_ready`, `model_error` states |
| `src/lib/components/settings-panel.svelte` | Modify | Conditional hint text; listen for `llm-update-available` event |
| `src-tauri/sidecar/llm_cleanup.py` | No change | Reuses existing `check_update_available()` |

## 7. Testing Strategy

### 7.1. Unit Tests (Rust)

- `check_model_update()` with sidecar running → reuses sidecar, drops guard before work
- `check_model_update()` with no sidecar → spawns temporary, quits after check
- `check_for_model_update()` with feature disabled → clears flag, returns `Ok(())`
- `check_for_model_update()` with LLM disabled → clears flag, returns `Ok(())`
- `check_for_model_update()` with settings lock contended → returns `Err`
- `check_for_model_update()` on 3rd consecutive error → clears flag, logs warning
- `refresh_tray_from_state()` with app update only → `TrayState::UpdateAvailable(version)`
- `refresh_tray_from_state()` with model update only → `TrayState::ModelUpdateAvailable`
- `refresh_tray_from_state()` with both → `TrayState::UpdateAvailable("vX.Y.Z (+ model)")`
- `refresh_tray_from_state()` with restart pending → `TrayState::RestartPending`
- `get_llm_status()` reads `model_update_available` from `UpdateState`

Use the existing `LlmBackend` trait for mocking (see `src-tauri/src/test_support.rs`).

### 7.2. Integration Tests

- Periodic loop calls both checks (panic-isolated) then refreshes tray once
- Manual `check_for_update()` → tray refresh includes model state
- `update_llm_model()` success → `model_update_available` cleared, tray refreshed
- `get_update_status()` returns `model_update_available` field correctly

### 7.3. Manual Verification

1. Enable LLM cleanup, simulate model update → tray icon shows dot, menu says "AI Model Update Available..."
2. Simulate both app + model updates → menu says "Update Available — vX.Y.Z (+ model)"
3. Click "Check for Updates" manually → tray refreshes correctly (doesn't erase model state)
4. Update model from update window → indicator disappears immediately
5. Disable `auto_check_updates` → no model checks run
6. Disable LLM cleanup → `model_update_available` clears, tray returns to normal
7. Simulate 3 consecutive HF failures → flag clears after ~12h
8. Open update window for model-only update → title says "SottoASR — Model Update"

## 8. Migration Plan

No data migration needed. New fields default to zero/false. Existing installs perform their first model check at the next 4-hour cycle.

## 9. Security Considerations

- Reuses existing `check_llm_update()` security model (sidecar `repo_info()` with 10s timeout)
- No new network endpoints — same HuggingFace API (`GET /api/models/{id}`)
- No automatic model download — only the check runs; download requires explicit user action
- `auto_check_updates` setting gates both app and model checks

## 10. Cost Analysis

- **Network:** One additional HuggingFace API call per 4-hour cycle (~1 KB response). Negligible.
- **CPU:** Reuses existing sidecar if running; spawns lightweight temporary Python process only if no sidecar exists (no MLX/model load). Cold-path adds 2-5s (process spawn + Python startup) to the first check after launch.
- **Worst-case per cycle:** 30s (app timeout) + ~15s (model cold path) = ~45s of work per 4h cycle (~0.3% overhead).
- **Memory:** Two `AtomicBool`/`AtomicU32` fields in `UpdateState`.
- **Disk:** No disk impact — only reads `refs/main`.
- **Dependencies:** No new dependencies.

## 11. Implementation Tasks

1. **Extract `check_model_update()` to `llm::engine`** — drop guard before `spawn_blocking`, two-path sidecar strategy
2. **Rewire `commands::llm::check_llm_update()`** — delegate to `engine::check_model_update()`
3. **Add model fields to `UpdateState`** — `model_update_available`, `model_update_consecutive_errors`
4. **Implement `check_for_model_update()` in updater** — feature guard, settings guard (try_lock), error counter, event emission
5. **Integrate into periodic loop** — panic-isolated `tokio::spawn`, call `refresh_tray_from_state()`
6. **Implement `refresh_tray_from_state()` in tray/menu.rs** — reads `UpdateState` directly, replaces `refresh_tray_for_update()` and `refresh_tray_for_restart()`
7. **Update `TrayState` enum** — add `ModelUpdateAvailable`, " (+ model)" suffix for combined state
8. **Dynamic window title** — "SottoASR — Model Update" for model-only path
9. **Wire `refresh_tray_from_state()` into `check_for_update()` and `perform_app_update()`** — replace old calls
10. **Sync `get_llm_status()` with `UpdateState`** — read `model_update_available` instead of hardcoding `false`
11. **Clear flag in `update_llm_model()`** — after successful download, clear flag + refresh tray
12. **Extend `get_update_status()` and TypeScript interface** — add `model_update_available`
13. **Add model states to update-view.svelte** — `model_available`, `model_downloading`, `model_ready`, `model_error` with stall detection
14. **Conditional settings hint** — show "model updates" only when LLM is available
15. **Write unit tests** — all scenarios from §7.1
16. **Manual verification** — all scenarios from §7.3

## 12. Implementation Status

Not started.

---

## A. Review Change Log

### v1.0 → v2.0 changes (driven by 2 rounds of adversarial review, 25+ findings)

**P0 fixes:**
- Extracted `check_model_update()` to `llm::engine` (fixed wrong dependency direction: `updater` → `commands`)
- Single canonical `refresh_tray_from_state()` that reads `UpdateState` directly (fixed 3 tray refresh entry points causing races)
- `update_llm_model()` clears `model_update_available` after success (fixed permanent state leak)
- `get_llm_status()` reads from `UpdateState` (fixed split truth with hardcoded `false`)

**P1 fixes:**
- Model download state machine specified in update-view (`model_available` → `model_downloading` → `model_ready`/`model_error`)
- Manual "Check for Updates" path preserved — `check_for_update()` calls `refresh_tray_from_state()` which reads full state
- Runtime feature guard via `is_feature_compiled()` (not `cfg!` — modules are compiled unconditionally)
- Error propagation from `check_for_model_update()` (loop's `log::warn!()` is live code, not dead code)
- `try_lock()` for settings read (matches `read_auto_check_setting()` pattern)

**P2 fixes:**
- Combined state label: `"Update Available — vX.Y.Z (+ model)"` (model update no longer silently suppressed)
- Dynamic window title: "SottoASR — Model Update" for model-only path
- Lock contention: guard dropped before `spawn_blocking` in `check_model_update()`
- Event naming: `llm-update-available` / `llm-update-up-to-date` (avoids collision with ASR `model-*`)
- Error TTL: `model_update_consecutive_errors` counter, clears flag after 3 failures (~12h)
- Panic isolation: each check wrapped in `tokio::spawn`
- Conditional settings hint text (only mentions "model" when LLM is available)
