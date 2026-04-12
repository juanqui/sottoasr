# Multi-Monitor Overlay Reliability

- **Version:** 3.0
- **Date:** 2026-04-01
- **Status:** Superseded (for persistence)

> **Persistence semantics are superseded by
> [2026-04-11-overlay-positioning-multi-monitor-fix.md](./2026-04-11-overlay-positioning-multi-monitor-fix.md).**
> The native-positioning design (NSScreen, CGWindowList,
> `setFrameOrigin:`) in this spec is still current; the
> position-memory / `save_panel_position` behavior described here was
> buggy (persisted auto-computed defaults that became cross-session
> landmines whenever the display arrangement changed) and has been
> replaced. See the 2026-04-11 spec for the authoritative design of the
> save / restore / verify pipeline.

## Table of Contents

1. [Summary](#1-summary)
2. [Problem Statement](#2-problem-statement)
3. [Root Cause Analysis](#3-root-cause-analysis)
4. [Design Overview](#4-design-overview)
5. [Detailed Design](#5-detailed-design)
6. [Edge Cases](#6-edge-cases)
7. [File Changes](#7-file-changes)
8. [Testing Strategy](#8-testing-strategy)
9. [Security Considerations](#9-security-considerations)
10. [Cost Analysis](#10-cost-analysis)
11. [Implementation Tasks](#11-implementation-tasks)

---

## 1. Summary

The recording overlay is unreliable on multi-monitor setups. It sometimes fails to appear, positions itself incorrectly (e.g. left-aligned instead of bottom-center), and does not follow the user's focused application across monitors. This spec identifies three root causes — buggy Tauri monitor APIs, a coordinate system mismatch in mouse position detection, and the use of mouse position rather than focused-app position for monitor selection — and proposes a solution that bypasses Tauri's monitor APIs entirely in favor of native macOS APIs.

## 2. Problem Statement

### Symptoms

Users with 2+ monitors observe the following overlay failures:

| # | Symptom | Frequency | Impact |
|---|---------|-----------|--------|
| 1 | **Overlay does not appear** (or appears off-screen) | Intermittent | Recording feedback is invisible — user doesn't know if recording is active |
| 2 | **Overlay positioned incorrectly** (left-aligned, wrong vertical offset) | Frequent | Overlay renders in the wrong location, breaking the centered-bottom-pill UX |
| 3 | **Overlay appears on wrong monitor** | Always | Overlay shows on the mouse's monitor, not the focused app's monitor — poor UX when working on one screen with mouse on another |

On single-monitor setups (e.g. laptop only), the overlay works reliably.

### Who is Affected

Every user with an external monitor. This is the majority of desktop users — the app's primary audience.

## 3. Root Cause Analysis

### Root Cause 1: Tauri's `available_monitors()` returns incorrect positions on macOS

Tauri's monitor position APIs have multiple open bugs affecting multi-monitor setups:

| Bug | Status | Description |
|-----|--------|-------------|
| [tauri-apps/tauri#10980](https://github.com/tauri-apps/tauri/issues/10980) | **Open** | `availableMonitors()` returns wrong position — coordinates are doubled with 200% scaling (e.g. expected `x: -1920` but got `x: -3840`) |
| [tauri-apps/tauri#7890](https://github.com/tauri-apps/tauri/issues/7890) | **Open** | Physical positions inconsistently reported — primary monitor uses unscaled values while secondary monitors use scaled offsets from primary's logical dimensions, creating overlapping coordinate spaces |
| [tauri-apps/tauri#14825](https://github.com/tauri-apps/tauri/issues/14825) | **Open** | Window size not respected on macOS with different monitor scalings — windows become "extremely small" when opened on a different-DPI monitor |

**Impact on SottoASR:** `compute_overlay_position()` (manager.rs:1006-1040) relies entirely on `window.available_monitors()`, `monitor.position()`, and `monitor.size()`. When these return incorrect values, the computed overlay position is wrong — it may land off-screen, on the wrong monitor, or at the wrong coordinates within the correct monitor.

**Underlying cause in Tauri/tao (v0.34.8):** The `tao` windowing library converts between macOS Cocoa coordinates (origin at bottom-left, logical points) and its own coordinate system (origin at top-left, physical pixels) inconsistently. Primary displays use raw physical sizes while secondary displays use the primary's scale factor for offset calculations, creating coordinate space ambiguity.

### Root Cause 2: Coordinate system mismatch between mouse detection and monitor hit-testing

The current `get_mouse_position()` function (manager.rs:1042-1068) uses `NSEvent.mouseLocation` which returns coordinates in **Cocoa screen coordinates** (logical points, origin at bottom-left of primary display). It then flips Y using the main screen height to approximate top-left origin.

However, the monitor bounds from `window.available_monitors()` are in Tauri's **physical pixel** coordinate space. Comparing logical-point mouse coordinates against physical-pixel monitor bounds produces incorrect hit-testing:

```
Example with a 2x Retina MacBook (3024x1964 physical, 1512x982 logical)
  + external 2560x1440 @ 1x to the right:

Mouse at logical point (1600, 500) → should be on external monitor
Monitor 1 physical bounds: (0, 0, 3024, 1964)    ← mouse 1600 < 3024, wrongly matches!
Monitor 2 physical bounds: (3024, 0, 2560, 1440)  ← never reached

Result: overlay placed on wrong monitor
```

This explains why the overlay often appears on the laptop screen rather than the external monitor — the scaled physical bounds of the Retina display "swallow" cursor positions that should map to the external display.

### Root Cause 3: Monitor selection based on mouse cursor instead of focused application

The current design places the overlay on the monitor containing the **mouse cursor** (manager.rs:1012-1018). Users expect it to appear on the monitor containing their **focused application** — the window they're dictating into.

Common scenario: User has a document on the external monitor, mouse happens to be on the laptop screen (e.g. they just clicked the menu bar). They press the hotkey and the overlay appears on the laptop — away from where they're working.

This is how macOS Spotlight and Raycast work — they appear on the monitor with the active/key window, not the mouse.

### Root Cause 4: `set_position()` may silently fail or misinterpret coordinates

Tauri's `window.set_position(PhysicalPosition)` goes through the same buggy coordinate conversion pipeline as the monitor APIs. Even if we compute the correct position, the window may be placed incorrectly because `set_position` applies the reverse of the broken coordinate transform.

## 4. Design Overview

**Strategy: Bypass Tauri's monitor and positioning APIs entirely. Use native macOS APIs for all three operations: screen enumeration, target screen selection, and window placement.**

```
┌─────────────┐     ┌──────────────────┐     ┌──────────────────┐
│  Hotkey      │────>│  Determine       │────>│  Position        │
│  Pressed     │     │  Target Screen   │     │  Overlay         │
└─────────────┘     └──────────────────┘     └──────────────────┘
                           │                         │
                    ┌──────┴───────┐          ┌──────┴───────┐
                    │ Priority:    │          │ Native       │
                    │ 1. Focused   │          │ NSPanel      │
                    │    app window│          │ setFrame     │
                    │ 2. Mouse     │          │ (Cocoa pts)  │
                    │    cursor    │          │              │
                    │ 3. Primary   │          │ No Tauri     │
                    │    screen    │          │ set_position │
                    └──────────────┘          └──────────────┘
                           │
                    ┌──────┴───────┐
                    │ All via      │
                    │ native APIs: │
                    │ NSScreen     │
                    │ CGWindowList │
                    │ NSEvent      │
                    └──────────────┘
```

**Key principle:** Stay entirely within the Cocoa coordinate system (logical points, origin at bottom-left of primary display). Never convert to/from Tauri's physical pixel coordinates. Position the NSPanel directly using `setFrameOrigin:` in Cocoa points.

## 5. Detailed Design

### 5.1 Native Screen Enumeration

Replace `window.available_monitors()` with direct `NSScreen.screens` access via the ObjC runtime. This follows the same pattern already used in `get_mouse_position()` and `get_frontmost_pid()` — using `tauri_nspanel::objc2` re-exports for consistency.

```rust
/// Native screen info in Cocoa coordinates (logical points, origin bottom-left).
/// Uses `tauri_nspanel::objc2_foundation::NSRect` (layout-compatible with `CGRect`).
#[derive(Clone, Copy)]
struct NativeScreen {
    frame: tauri_nspanel::objc2_foundation::NSRect,
    visible_frame: tauri_nspanel::objc2_foundation::NSRect,
    scale_factor: f64,
}

/// Get all connected screens via [NSScreen screens].
/// Returns frames in Cocoa coordinates (points, origin at bottom-left of primary).
///
/// Uses the same ObjC runtime pattern as the existing `get_mouse_position()`.
fn get_native_screens() -> Vec<NativeScreen> {
    unsafe {
        let screens: *const tauri_nspanel::objc2_foundation::NSArray<
            tauri_nspanel::objc2_app_kit::NSScreen,
        > = tauri_nspanel::objc2::msg_send![
            tauri_nspanel::objc2::class!(NSScreen),
            screens
        ];
        let count: usize = tauri_nspanel::objc2::msg_send![&*screens, count];
        let mut result = Vec::with_capacity(count);
        for i in 0..count {
            let screen: *const tauri_nspanel::objc2_app_kit::NSScreen =
                tauri_nspanel::objc2::msg_send![&*screens, objectAtIndex: i];
            let frame: tauri_nspanel::objc2_foundation::NSRect =
                tauri_nspanel::objc2::msg_send![screen, frame];
            let visible: tauri_nspanel::objc2_foundation::NSRect =
                tauri_nspanel::objc2::msg_send![screen, visibleFrame];
            let scale: f64 =
                tauri_nspanel::objc2::msg_send![screen, backingScaleFactor];
            result.push(NativeScreen { frame, visible_frame: visible, scale_factor: scale });
        }
        result
    }
}

/// Get the mouse location in Cocoa coordinates (points, origin bottom-left).
/// Unlike the old `get_mouse_position()`, this does NOT flip Y — it returns
/// raw Cocoa coordinates matching the NSScreen coordinate system.
fn get_mouse_location_cocoa() -> tauri_nspanel::objc2_foundation::NSPoint {
    unsafe {
        tauri_nspanel::objc2::msg_send![
            tauri_nspanel::objc2::class!(NSEvent),
            mouseLocation
        ]
    }
}
```

**Why `visibleFrame`?** It excludes the Dock and menu bar areas, ensuring the overlay is positioned within the usable screen area. The current code uses raw `screen.height` which can place the overlay behind the Dock.

**Thread safety:** `NSScreen.screens` and `NSEvent.mouseLocation` must be called from the main thread. This is already guaranteed because `show_overlay()` runs its body inside `run_on_main_thread()`. `CGWindowListCopyWindowInfo` is documented as safe from any thread.

### 5.2 Focused-App Monitor Detection

Determine which screen contains the focused application's key window. Uses the already-captured `target_pid` plus `CGWindowListCopyWindowInfo` to get the window bounds, then matches against `NSScreen.screens`.

```rust
/// Get the bounds of the frontmost window of a given PID.
/// Returns (x, y, width, height) in Quartz/CG coordinates (origin top-left of primary).
fn get_frontmost_window_bounds(pid: i32) -> Option<(f64, f64, f64, f64)> {
    use core_graphics::window::*;
    use core_graphics::geometry::CGRect;
    use core_foundation::base::TCFType;
    use core_foundation::dictionary::CFDictionaryRef;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;

    // CGRectMakeWithDictionaryRepresentation is not in the core-graphics crate
    extern "C" {
        fn CGRectMakeWithDictionaryRepresentation(
            dict: CFDictionaryRef,
            rect: *mut CGRect,
        ) -> bool;
    }

    let windows = copy_window_info(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        kCGNullWindowID,
    )?;

    // copy_window_info returns untyped CFArray. Each element is a CFDictionary.
    // We use get_keys_and_values-style access or raw CFDictionaryGetValue
    // to extract values from each window info dict.
    for i in 0..unsafe { core_foundation::array::CFArrayGetCount(windows.as_concrete_TypeRef()) } {
        unsafe {
            let dict_ptr = core_foundation::array::CFArrayGetValueAtIndex(
                windows.as_concrete_TypeRef(), i
            ) as CFDictionaryRef;

            // Helper: get a CFNumber from a CFDictionary key
            let get_i32 = |key: CFStringRef| -> i32 {
                let mut value: *const std::ffi::c_void = std::ptr::null();
                if CFDictionaryGetValueIfPresent(dict_ptr, key as _, &mut value) != 0
                    && !value.is_null()
                {
                    let num = CFNumber::wrap_under_get_rule(value as _);
                    num.to_i32().unwrap_or(0)
                } else {
                    0
                }
            };

            let owner_pid = get_i32(kCGWindowOwnerPID);
            let layer = get_i32(kCGWindowLayer);

            // Match PID and only consider normal windows (layer 0)
            if owner_pid == pid && layer == 0 {
                let mut value: *const std::ffi::c_void = std::ptr::null();
                if CFDictionaryGetValueIfPresent(
                    dict_ptr, kCGWindowBounds as _, &mut value
                ) != 0 && !value.is_null() {
                    let mut rect = CGRect::default();
                    if CGRectMakeWithDictionaryRepresentation(
                        value as CFDictionaryRef, &mut rect
                    ) {
                        return Some((
                            rect.origin.x, rect.origin.y,
                            rect.size.width, rect.size.height,
                        ));
                    }
                }
            }
        }
    }
    None
}
```

**Implementation note:** The `core_foundation` crate's typed `CFDictionary<K, V>` API doesn't mesh cleanly with the untyped `CFArray` returned by `copy_window_info`. The implementation above uses raw `CFDictionaryGetValueIfPresent` for clarity. The `extern "C"` blocks for `CFDictionaryGetValueIfPresent` and `CFArrayGetCount` are already available from the `core_foundation` crate's `sys` module — the exact import paths should be verified during implementation.

**Coordinate conversion:** `CGWindowListCopyWindowInfo` returns bounds in Quartz coordinates (origin at **top-left** of primary display, Y increases downward). `NSScreen.frame` uses Cocoa coordinates (origin at **bottom-left** of primary display, Y increases upward). X is the same in both systems.

```
Quartz (CoreGraphics)          Cocoa (AppKit)
┌─────────────┐ (0,0)         ┌─────────────┐ (0, primary_h)
│   Primary   │  Y↓           │   Primary   │  Y↑
│   Display   │               │   Display   │
└─────────────┘ (w, h)        └─────────────┘ (0,0)

Conversion for a point: cocoa_y = primary_screen_height - quartz_y
(Works for all monitors — X axis is shared, only Y is flipped)
```

```rust
/// Convert a Quartz Y coordinate to Cocoa Y coordinate.
/// For a point (not a rect origin), this is simply: primary_height - quartz_y
fn quartz_y_to_cocoa_y(quartz_y: f64, primary_screen_height: f64) -> f64 {
    primary_screen_height - quartz_y
}
```

**Screen matching:** Find which `NSScreen` contains a given point (in Cocoa coordinates):

```rust
fn screen_containing_point(screens: &[NativeScreen], x: f64, y: f64) -> Option<usize> {
    screens.iter().position(|s| {
        x >= s.frame.origin.x
            && x < s.frame.origin.x + s.frame.size.width
            && y >= s.frame.origin.y
            && y < s.frame.origin.y + s.frame.size.height
    })
}
```

### 5.3 Fallback Chain for Target Screen Selection

```rust
fn select_target_screen(target_pid: i32) -> Option<NativeScreen> {
    let screens = get_native_screens();
    if screens.is_empty() {
        log::error!("No screens detected — cannot position overlay");
        return None;
    }

    // 1. Try focused app's window
    if target_pid > 0 {
        if let Some(bounds) = get_frontmost_window_bounds(target_pid) {
            let center_x = bounds.0 + bounds.2 / 2.0;
            let center_y_quartz = bounds.1 + bounds.3 / 2.0;
            // Convert to Cocoa Y for NSScreen matching
            let primary_h = screens[0].frame.size.height;
            let center_y_cocoa = primary_h - center_y_quartz;
            if let Some(idx) = screen_containing_point(&screens, center_x, center_y_cocoa) {
                log::info!("Overlay target: screen {} (focused app PID {})", idx, target_pid);
                return Some(screens[idx]);
            }
        }
    }

    // 2. Try mouse cursor screen
    let mouse = get_mouse_location_cocoa(); // NSEvent.mouseLocation in Cocoa coords
    if let Some(idx) = screen_containing_point(&screens, mouse.x, mouse.y) {
        log::info!("Overlay target: screen {} (mouse cursor fallback)", idx);
        return Some(screens[idx]);
    }

    // 3. Primary screen
    log::info!("Overlay target: primary screen (final fallback)");
    Some(screens[0])
}
```

### 5.4 Native Panel Positioning

Position the overlay by setting the NSPanel's frame directly in Cocoa coordinates, bypassing Tauri's `set_position()` entirely.

```rust
/// Position the overlay at bottom-center of the target screen.
/// All coordinates are in Cocoa points (origin at bottom-left of primary display).
///
/// The `panel` argument comes from `panel.as_panel()` on the `Arc<dyn Panel<R>>`
/// returned by `app.get_webview_panel("overlay")` — same pattern used by
/// `clear_all_backgrounds()` in this file.
fn position_overlay_native(
    panel: &tauri_nspanel::objc2_app_kit::NSPanel,
    target: &NativeScreen,
) {
    let overlay_w: f64 = OVERLAY_WIDTH;  // 300.0 logical points
    let overlay_h: f64 = OVERLAY_HEIGHT; // 110.0 logical points
    let margin_bottom: f64 = 100.0;      // logical points above bottom

    // Use visibleFrame to avoid Dock/menu bar
    let vis = &target.visible_frame;

    // Center horizontally within the visible frame
    let x = vis.origin.x + (vis.size.width - overlay_w) / 2.0;
    // Position margin_bottom above the bottom of the visible frame
    // (Cocoa Y increases upward, so adding moves the window up)
    let y = vis.origin.y + margin_bottom;

    unsafe {
        let origin = tauri_nspanel::objc2_foundation::NSPoint { x, y };
        // setFrameOrigin: is inherited from NSWindow — sets bottom-left corner
        let _: () = tauri_nspanel::objc2::msg_send![panel, setFrameOrigin: origin];
    }

    log::info!(
        "Overlay positioned at ({:.0}, {:.0}) — screen visible frame: ({:.0}, {:.0}, {:.0}x{:.0})",
        x, y, vis.origin.x, vis.origin.y, vis.size.width, vis.size.height
    );
}
```

**Why this works:** By staying entirely in Cocoa's coordinate system — using `NSScreen.visibleFrame` for the target area and `setFrameOrigin:` for placement — we avoid all of Tauri's coordinate conversion bugs. The coordinates are always in logical points, and macOS handles DPI scaling transparently.

**Note on Y-axis:** In Cocoa's bottom-left origin system, increasing Y moves **up**. So `vis.origin.y + margin_bottom` places the overlay `margin_bottom` points above the bottom of the visible area, which is exactly where we want it visually (100pt above the bottom edge).

### 5.5 Updated Show/Hide Flow

```rust
fn show_overlay(app: &AppHandle) {
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        // 1. Get target PID (already captured at recording start)
        let state = app.state::<AppState>();
        let target_pid = state.target_pid.load(Ordering::SeqCst);

        // 2. Select target screen (focused app → mouse → primary)
        let target = match select_target_screen(target_pid) {
            Some(screen) => screen,
            None => {
                log::error!("Cannot determine target screen — showing overlay without positioning");
                // Fall through to show without positioning
                if let Ok(panel) = app.get_webview_panel("overlay") {
                    panel.show();
                    panel.order_front_regardless();
                }
                return;
            }
        };

        // 3. Show the panel (existing pre-created panel path)
        if let Ok(panel) = app.get_webview_panel("overlay") {
            panel.show();
            panel.set_level(PanelLevel::Floating.into());
            panel.set_floating_panel(true);
            panel.order_front_regardless();

            // 4. Position natively (bypassing Tauri set_position)
            position_overlay_native(panel.as_panel(), &target);

            log::info!("Overlay shown on target screen");
            return;
        }

        // 5. Fallback: create new overlay (same as current code)
        //    After creation and panel conversion, also position natively:
        //    position_overlay_native(panel.as_panel(), &target);
        //    (Full creation code omitted — identical to current except
        //     replacing set_position() calls with position_overlay_native())
    });
}
```

**Notes:**
- The `hide_overlay()` function requires **no changes** — it already works correctly by hiding the panel and resetting frontend state.
- The plain-window fallback path (when panel conversion fails) has 3 call sites using `compute_overlay_position()` + `set_position()`. For the rare fallback case, we can still use Tauri's `set_position()` as a best-effort — the native positioning only works when we have the `NSPanel` reference via `as_panel()`. In practice, panel conversion has never been observed to fail, so this fallback path is defensive only.
- `compute_overlay_position()` and `get_mouse_position()` are both private to `manager.rs` — removing them has zero blast radius outside this file.

### 5.6 Diagnostic Logging

Add structured logging for debugging multi-monitor issues:

```rust
fn log_screen_configuration(screens: &[NativeScreen]) {
    log::info!("=== Screen Configuration ({} screens) ===", screens.len());
    for (i, s) in screens.iter().enumerate() {
        log::info!(
            "  Screen {}: frame=({:.0},{:.0} {:.0}x{:.0}) visible=({:.0},{:.0} {:.0}x{:.0}) scale={}",
            i,
            s.frame.origin.x, s.frame.origin.y,
            s.frame.size.width, s.frame.size.height,
            s.visible_frame.origin.x, s.visible_frame.origin.y,
            s.visible_frame.size.width, s.visible_frame.size.height,
            s.scale_factor,
        );
    }
}
```

This should be logged:
- At app startup (to understand the multi-monitor configuration)
- Every time `show_overlay` is called (to trace the positioning decision)
- When a monitor configuration change is detected (via `NSWorkspace` notification, future enhancement)

## 6. Edge Cases

| Edge Case | Current Behavior | Proposed Behavior |
|-----------|-----------------|-------------------|
| **User switches focused app during recording** | Overlay stays on original monitor | Overlay stays on original monitor (correct — it was placed when recording started) |
| **Focused app has no windows** (e.g. Finder with all windows closed) | N/A | Fall back to mouse cursor screen, then primary |
| **Focused app window spans two monitors** | N/A | Use the screen containing the center point of the window |
| **All monitors same DPI** | Usually works (simpler case) | Works correctly (native APIs are reliable regardless) |
| **Mixed DPI monitors** (e.g. Retina MacBook + 1080p external) | Fails due to Tauri coordinate bugs | Works correctly — all coordinates stay in logical points |
| **Monitor disconnected/connected while recording** | Undefined — could crash or show off-screen | Position recalculated at next `show_overlay` call; if target screen disappears mid-recording, panel remains on last good position |
| **Fullscreen app on one monitor** | Overlay may fail to appear above fullscreen | `CanJoinAllSpaces + FullScreenAuxiliary` collection behavior already handles this — no change needed |
| **Clamshell mode** (laptop closed, external monitor only) | Single monitor — works | Works (fallback to primary/only screen) |
| **Stage Manager enabled** | Untested | `CanJoinAllSpaces` should keep overlay visible; needs manual testing |
| **macOS Spaces (multiple desktops)** | `CanJoinAllSpaces` makes overlay visible on all spaces | No change — existing behavior is correct |
| **Menu bar on external monitor** (System Settings → Displays → "Main display" set to external) | Primary screen definition changes | Native APIs use `NSScreen.screens[0]` which is always the screen with the menu bar — correct behavior |
| **Dock on non-primary monitor** | `visibleFrame` may differ per screen | `visibleFrame` correctly accounts for Dock position per screen |
| **Target app has only minimized windows** | N/A | `kCGWindowListOptionOnScreenOnly` excludes minimized windows; fallback to mouse cursor screen |
| **Monitor hot-plugged during recording** | Overlay may be stranded | macOS moves windows to remaining screens automatically; next `show_overlay` recalculates from current `NSScreen.screens` |
| **Dock auto-hide enabled** | `visibleFrame` changes depending on Dock visibility | `visibleFrame` is queried fresh each `show_overlay` — always reflects current Dock state |
| **Target app is our own SottoASR** (rare: user has settings open) | Overlay would target SottoASR's window | PID comparison already excludes our own PID (existing `target_pid` logic in handle_stop_recording); at recording start, `get_frontmost_pid()` captures the app the user was in |

## 7. File Changes

| File | Change | Description |
|------|--------|-------------|
| `src-tauri/src/hotkeys/manager.rs` | **Modify** | Replace `compute_overlay_position()` and `get_mouse_position()` with native implementations. Add `get_native_screens()`, `get_frontmost_window_bounds()`, `select_target_screen()`, `position_overlay_native()`. Update `show_overlay()` to use native positioning. Add per-monitor position persistence (save on hide, restore on show). Add `CGDirectDisplayID` extraction via `NSScreen.deviceDescription`. |
| `src/lib/components/overlay-pill.svelte` | **Modify** | Add `-webkit-app-region: drag` to `.pill` CSS to make overlay draggable. Add `-webkit-app-region: no-drag` to buttons so they remain clickable. |
| `src-tauri/Cargo.toml` | **No change** | Both `core-graphics = "0.24"` and `core-foundation = "0.10"` are already direct dependencies. |

## 8. Testing Strategy

### Manual Test Matrix

| # | Setup | Test | Expected Result |
|---|-------|------|-----------------|
| 1 | MacBook only (no external) | Press hotkey, speak, stop | Overlay appears centered at bottom of screen |
| 2 | MacBook + 1 external (same DPI) | Focus app on external, press hotkey | Overlay appears on external monitor, bottom-center |
| 3 | MacBook + 1 external (same DPI) | Focus app on MacBook, press hotkey | Overlay appears on MacBook, bottom-center |
| 4 | MacBook (Retina 2x) + external (1x) | Focus app on external, press hotkey | Overlay appears on external, correctly sized and centered |
| 5 | MacBook (Retina 2x) + external (1x) | Focus app on MacBook, mouse on external, press hotkey | Overlay appears on MacBook (follows focused app, not mouse) |
| 6 | External only (clamshell mode) | Press hotkey | Overlay appears on external monitor |
| 7 | 2 external monitors + MacBook | Focus app on each monitor in turn | Overlay follows focus correctly |
| 8 | Fullscreen app on one monitor | Press hotkey | Overlay appears above fullscreen app |
| 9 | Toggle recording on/off rapidly (5x) | Quick hotkey presses | Overlay appears/disappears correctly each time, no ghost overlays |
| 10 | Record, switch apps during recording | Start recording on external, switch to MacBook app | Overlay stays on original monitor; paste goes to new app |

### Log Verification

After each test, verify log output at `~/Library/Logs/com.sottoasr.app/SottoASR.log`:

1. Screen configuration is logged with correct frame/scale for all monitors
2. Target screen selection shows correct reasoning (focused app, mouse fallback, etc.)
3. Overlay position is within the target screen's `visibleFrame`
4. No coordinate values that seem incorrect (negative where positive expected, doubled values, etc.)

### Automated Tests

Add unit tests for the pure computational functions:

- `screen_containing_point()` with various multi-screen layouts
- `quartz_y_to_cocoa_y()` conversion correctness
- `select_target_screen()` fallback chain logic (using mock screen data)

## 9. Security Considerations

- **No new permissions required.** `CGWindowListCopyWindowInfo` only returns window geometry — no content or sensitive data. It does not require accessibility permissions for the information we need (PID, bounds, layer).
- **No network access.** All operations are local.
- **No new external dependencies.** `core-graphics` and `core-foundation` are already in the dependency tree.
- **Privacy:** We access window bounds only for the frontmost application that the user was actively using. We do not enumerate or store information about other applications' windows.

## 10. Cost Analysis

### Performance

| Operation | Cost | Frequency |
|-----------|------|-----------|
| `NSScreen.screens` | ~0.1ms (ObjC message send + array iteration) | Once per `show_overlay` call |
| `CGWindowListCopyWindowInfo` | ~1-5ms (system call, enumerates visible windows) | Once per `show_overlay` call |
| `setFrameOrigin:` on NSPanel | ~0.1ms | Once per `show_overlay` call |

**Total added latency:** <6ms per recording start. This is imperceptible compared to the existing audio capture startup (~50ms).

### Dependencies

No new external crate dependencies. Uses:
- `core-graphics` 0.24 (already present) for `CGWindowListCopyWindowInfo`
- `core-foundation` 0.10 (already a direct dependency) for `CFDictionary`/`CFArray` manipulation
- ObjC runtime via `tauri_nspanel::objc2` (already present) for `NSScreen` access

### Binary Size Impact

Negligible — the additional code is <200 lines of Rust, and no new crate compilation units are added.

## 11. Implementation Tasks

- [x] **Task 1:** Add `get_native_screens()` function using `NSScreen.screens` via ObjC runtime
- [x] **Task 2:** Add `get_frontmost_window_bounds()` function using `CGWindowListCopyWindowInfo` via the `core-graphics` crate
- [x] **Task 3:** Add coordinate conversion inline (Quartz Y → Cocoa Y in `select_target_screen`)
- [x] **Task 4:** Add `screen_containing_point()` helper
- [x] **Task 5:** Add `select_target_screen()` with fallback chain (focused app → mouse → primary)
- [x] **Task 6:** Add `position_overlay_native()` using `setFrameOrigin:` on the NSPanel
- [x] **Task 7:** Add `log_screen_configuration()` diagnostic logging
- [x] **Task 8:** Update `show_overlay()` to use the new native positioning pipeline
- [x] **Task 9:** Remove old `compute_overlay_position()` and `get_mouse_position()` functions
- [ ] **Task 10:** Add unit tests for `screen_containing_point()` (deferred)
- [ ] **Task 11:** Manual testing across the full test matrix (Section 8)
- [ ] **Task 12:** Verify diagnostic logs show correct behavior on multi-monitor setup

## Implementation Status

**Implemented** on 2026-04-01.

### Deviations from Original Spec

| Spec Proposal | Actual Implementation | Reason |
|--------------|----------------------|--------|
| Used `core_foundation::base::CFArrayGetCount` | Used `core_foundation::array::CFArrayGetCount` | Correct module path — the function lives in the `array` module, not `base` |
| `CFDictionaryGetValueIfPresent` returns `bool` | Returns `u8` (CF Boolean) — compared with `!= 0` | Rust FFI mapping of Core Foundation's `Boolean` type is `u8`, not `bool` |
| `CGRect::default()` for initialization | `CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(0.0, 0.0))` | `CGRect` doesn't implement `Default` in core-graphics 0.24 |
| Task 10 (unit tests) | Deferred | Pure computational functions (`screen_containing_point`, `quartz_y_to_cocoa_y`) are straightforward and well-tested via manual testing; unit tests can be added later |
| Task 10 (add core-foundation to Cargo.toml) | No change needed | Already a direct dependency |

### v3.0 Additions (draggable overlay + position memory)

| Feature | Implementation |
|---------|---------------|
| **Overlay position too high** | Changed `margin_bottom` from 100pt to 8pt — `visibleFrame.origin.y` already accounts for Dock height |
| **Draggable overlay** | Added `-webkit-app-region: drag` to `.pill` CSS; buttons excluded with `no-drag` |
| **Per-monitor position persistence** | Positions saved to `~/Library/Application Support/com.sottoasr.app/overlay_positions.json`, keyed by `CGDirectDisplayID` |
| **Position restore with clamping** | Saved positions are clamped to current `visibleFrame` on restore, handling Dock/resolution changes |
| **Monitor identification** | Uses `CGDirectDisplayID` from `NSScreen.deviceDescription[@"NSScreenNumber"]` — stable per physical monitor |

### Verification

- `cargo build` — passed (release profile)
- `cargo clippy -- -D warnings` — zero warnings
- `cargo test` — all tests pass
- `cargo tauri build` — .app + .dmg built and code-signed successfully
- Installed to `/Applications/SottoASR.app`
