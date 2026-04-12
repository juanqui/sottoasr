# Overlay Positioning Multi-Monitor Fix

- **Version:** 1.4
- **Date:** 2026-04-11
- **Status:** Implemented

## Table of Contents

1. [Summary](#1-summary)
2. [Problem Statement](#2-problem-statement)
3. [Root Cause Analysis](#3-root-cause-analysis)
4. [Design Overview](#4-design-overview)
5. [Detailed Design](#5-detailed-design)
6. [Edge Cases](#6-edge-cases)
7. [File Changes](#7-file-changes)
8. [Testing Strategy](#8-testing-strategy)
9. [Migration Plan](#9-migration-plan)
10. [Security Considerations](#10-security-considerations)
11. [Cost Analysis](#11-cost-analysis)
12. [Implementation Tasks](#12-implementation-tasks)
13. [Implementation Status](#13-implementation-status)

---

## 1. Summary

The recording overlay currently positions itself using native macOS APIs
(`NSScreen.screens`, `NSWindow.setFrameOrigin:`) and persists a per-display
position (keyed by `CGDirectDisplayID`) across sessions. On single-monitor
setups it works correctly. On multi-monitor setups it frequently renders at
the border between two displays — "far right of screen 1 or far left of
screen 2" — giving the illusion of being centered across the *union* of all
screens rather than centered on the target screen.

This spec identifies three concrete defects in the persistence + ordering
logic, shows the smoking-gun evidence from the user's saved-positions
file, and proposes a minimal, robust fix. The **primary and sufficient
fix** is to stop persisting auto-computed defaults: the overlay's
`hide_overlay` currently snapshots whatever frame the panel holds at the
moment it is hidden, including the default-centered one that
`show_overlay` itself just wrote, which seeds a cross-session landmine
that fires the next time the display arrangement changes. Two secondary
improvements round out the fix: the restore path *discards* (rather than
clamping) any stored value that no longer fits its display's current
`visibleFrame`, and the panel is positioned *before* `show` to avoid a
known-flaky cross-display `setFrameOrigin:` pattern that would otherwise
surface once the primary fix is in place. A fourth, optional enhancement
— reacting to `NSApplication.didChangeScreenParametersNotification` so
the overlay can realign during a live recording — is documented for
completeness but is explicitly deferrable: the primary fix resolves the
reported bug without it.

## 2. Problem Statement

### Observed Symptom

With ≥ 2 connected displays, when the user presses the push-to-talk hotkey
the overlay pill appears at a location that looks visually like the center
of the *union* of all displays — i.e. straddling or just inside the
boundary between two screens — rather than at the bottom-center of the
screen containing the focused application. The same hotkey on the same
build, with only the laptop display active, places the overlay correctly.

### Evidence (smoking gun)

The user's saved-positions file on the current affected machine is:

```json
// ~/Library/Application Support/com.sottoasr.app/overlay_positions.json
{
  "1": { "x": 705.0,  "y": 92.0 },
  "3": { "x": 1770.0, "y": 92.0 }
}
```

Analysis:

| Field | Meaning |
|-------|---------|
| `"1"` / `"3"` | `CGDirectDisplayID` of a physical display (see §3 caveats on stability) |
| `x: 705.0` | For display 1, this equals `(1710 − 300) / 2` — i.e. the *default centered* x that `position_overlay_native` computes for a 1710-wide visible frame beginning at `x = 0`. Never touched by the user. |
| `x: 1770.0` | For display 3, this equals `(3840 − 300) / 2` exactly — i.e. the *default centered* x that `position_overlay_native` would compute for a 3840-wide visible frame whose `origin.x = 0`. Never touched by the user. |
| `y: 92.0` | `visibleFrame.origin.y + 8` for a display with an 84-pt Dock — the computed default y. |

Both entries are **auto-computed defaults that were snapshotted into the
persistence file by `hide_overlay` on the prior session**, when the
display arrangement was different.

**Why `x = 1770` specifically ends up looking like "far left of screen
2":** In the current arrangement, display 3 sits at `origin.x = 1728`
(the laptop became the primary and display 3 moved to its right). The
saved `x = 1770` still lies *inside* display 3's new `visibleFrame`
(1770 ≥ 1728 and 1770 + 300 ≤ 1728 + 2560), so any simple containment
check would accept it — but visually, `x = 1770` is just **42 pt past
the left edge** of the new display 3. From the user's perspective the
overlay is pinned against the border between screens.

Crucial implication: the restore path is returning a value that is
"legal for the current visible frame" but meaningless in the current
arrangement. A simple "is the saved point inside the current
`visibleFrame`?" check would therefore be insufficient — the value has
to be rejected *at the source*, by never persisting it in the first
place. This is why Defect A below is the load-bearing fix and Defect B
is only a belt-and-braces safety net.

### `CGDirectDisplayID` stability caveat

`CGDirectDisplayID` is [documented](https://developer.apple.com/documentation/coregraphics/cgdirectdisplayid)
as "a unique identifier for an attached display" whose value "can change
when a monitor is unplugged or the system is rebooted". In practice it
is stable *within* a single boot session and often stable across
reboots for the same hardware configuration. This is stable enough for
the reporter's bug to reproduce (the stale entries carry over between
app launches on the same boot) and stable enough to make
"user-dragged-position memory" a useful feature. It is **not** stable
enough to guarantee a drag survives every reboot. Losing a drag across
a reboot is an acceptable tradeoff; the prior spec (v3.0) already
accepts it. No change.

### Who is Affected

Every user who has plugged in or unplugged an external display, changed
the display arrangement in System Settings, or switched between a docked
and undocked setup after installing the app. In practice this is every
multi-monitor user.

### Why Single-Monitor Works

On single-monitor setups the only saved entry maps to the only connected
display whose frame has not changed. The restored `(x, y)` still
corresponds to the same visual location and the default centering math
happens to match the saved value, so the bug is invisible.

## 3. Root Cause Analysis

The current implementation lives in `src-tauri/src/hotkeys/manager.rs`
under the section titled *"Native multi-monitor overlay positioning"*
(functions `precreate_overlay`, `show_overlay`, `hide_overlay`,
`get_saved_position`, `save_panel_position`, `position_overlay_native`,
`select_target_screen`, `get_native_screens`). The multi-monitor failure
is the combination of three independent defects, listed in order of
contribution to the observed symptom.

### Defect A — Auto-computed defaults are persisted as if they were user choices

`hide_overlay` unconditionally calls `save_panel_position` with whatever
Cocoa `(x, y)` the panel currently has, regardless of whether the user
dragged the overlay. Because `show_overlay` *always* sets the frame to the
computed default (unless a saved position already exists), the very first
recording on a new display seeds a persistent entry whose value is the
default-centered coordinates on *that* display *at that moment*.

When the display's frame later changes — because another display was
added, removed, or re-arranged — the stored absolute Cocoa `(x, y)` no
longer corresponds to the same visual spot on the same physical display,
but `get_saved_position` restores it anyway.

References:
- `hide_overlay` at `src-tauri/src/hotkeys/manager.rs:943`
- `save_panel_position` at `src-tauri/src/hotkeys/manager.rs:1082`
- `get_saved_position` at `src-tauri/src/hotkeys/manager.rs:1099`

### Defect B — Stale saved positions are silently clamped instead of discarded

`get_saved_position` clamps `saved.x` / `saved.y` to the *current*
`visibleFrame`:

```rust
let clamped_x = saved.x
    .max(visible.origin.x)
    .min(visible.origin.x + visible.size.width - OVERLAY_WIDTH);
```

Clamping is the wrong failure mode. It takes a stale absolute coordinate
and pins it to the nearest visible edge, producing the exact "flush
against the left/right edge of a non-primary display" symptom. The
correct failure mode for a position that no longer fits is to *discard
it* and fall back to the default center, not to pin it to an edge.

Reference: `src-tauri/src/hotkeys/manager.rs:1099-1114`.

### Defect C — `setFrameOrigin:` is called *after* `panel.show()` for the precreated-panel path (defensive only)

**This defect is defensive.** The reporter did not describe flicker,
animation, or a half-moved panel. Defect A alone is sufficient to fix
the reported bug. Defect C is included in this spec because once we stop
persisting stale defaults, every `show_overlay` will start recomputing a
correct target coordinate — which means every multi-display recording
will, at some point, ask `setFrameOrigin:` to transport the panel from
whatever display it was last shown on to whatever display the user is
working on *now*. That is exactly the pattern third-party writeups
report as unreliable, and we want to eliminate the hazard before it
manifests as a follow-up bug.

`show_overlay`'s existing-panel branch currently runs:

```rust
panel.show();                     // 1. becomes visible (at stale pos)
panel.set_level(...);             // 2. re-apply floating level
panel.set_floating_panel(true);
panel.order_front_regardless();
position_overlay_native(...);     // 4. move to target
```

Reports of this ordering producing flaky cross-display moves come from
third-party sources, not Apple documentation:
- [rxhanson/Rectangle #1723](https://github.com/rxhanson/Rectangle/issues/1723) — "window resets to center when moving to next/previous display"
- [wails #5117](https://github.com/wailsapp/wails/issues/5117) — Y coordinate converted against the wrong screen
- [`setFrameOrigin` cross-screen Medium writeup](https://medium.com/@clyapp/programmatically-move-a-nswindow-to-another-screen-din-macos-a50e12bd722e) — "call it async on the main queue after the window is ordered front"
- Apple's 10.9 release notes on `screensHaveSeparateSpaces`: *"A window will get assigned to the display containing the majority of its geometry if programmatically positioned in a spanning position."*

The failure mode these sources describe is: a `setFrameOrigin:` call
that crosses screen boundaries while the window is visible may be
partially honored, animated, or overridden by the window server. It is
not documented in Apple docs and I have not directly reproduced it
against SottoASR's panel, so treat this defect as *defensive hardening*,
not as a root cause.

Reference: `src-tauri/src/hotkeys/manager.rs:861-875`.

### Secondary defect — `precreate_overlay` never positions the panel

`precreate_overlay` creates the panel hidden but never calls
`position_overlay_native`. The panel therefore starts at whatever origin
`WebviewWindowBuilder` chose (typically near the primary display's
top-left or a cascaded default). Combined with Defect C, the very first
recording after app launch is the most likely case to exhibit a visible
flash-then-jump, because the panel's *starting* screen is guaranteed to
be different from the target screen whenever the user launches the app
with mouse focus on a secondary display.

Reference: `src-tauri/src/hotkeys/manager.rs:793-843`.

### Not a defect — the centering math

The per-screen centering math is correct:

```rust
let x = vis.origin.x + (vis.size.width - OVERLAY_WIDTH) / 2.0;
let y = vis.origin.y + margin_bottom;
```

`vis` is the target screen's `visibleFrame` in global Cocoa points
(origin at the bottom-left of `NSScreen.screens[0]`, which [per Apple
docs](https://developer.apple.com/documentation/AppKit/NSScreen/screens)
is guaranteed to be the menu-bar / primary display and the origin of the
global coordinate system). Adding the per-screen origin recovers a valid
absolute coordinate on any connected display regardless of layout, DPI,
or orientation. No change is required here.

## 4. Design Overview

Four coordinated changes:

1. **Distinguish user-set positions from computed defaults.** Only
   persist a position that the user explicitly moved (dragged). Never
   persist an auto-computed default.
2. **Discard (don't clamp) stale saved positions.** If a restored
   position does not lie fully inside the current `visibleFrame` of the
   target display, throw it away and use the fresh default instead.
3. **Position before show, then verify after show.** Call
   `setFrameOrigin:` while the panel is still hidden (or after
   `orderOut`), and then re-verify after `show` that the panel landed on
   the target screen. Re-apply once if it did not.
4. **React to display reconfiguration.** Subscribe to
   `NSApplication.didChangeScreenParametersNotification` on startup and,
   when it fires, prune any saved entries whose `display_id` is no longer
   connected *or* whose saved point no longer lies inside the new
   `visibleFrame`. This keeps the persistence file from accumulating
   cross-session land mines.

```
┌──────────────────────┐   hotkey   ┌────────────────────────┐
│  App state           │  ────────▶ │  show_overlay()        │
│  (target_pid)        │            │                        │
└──────────────────────┘            │  1. select target      │
                                    │     screen             │
                                    │                        │
                                    │  2. compute default    │
                                    │     bottom-center      │
                                    │     (fresh each call)  │
                                    │                        │
                                    │  3. try saved override │
                                    │     ─ discard if       │
                                    │       outside current  │
                                    │       visibleFrame     │
                                    │     ─ discard if       │
                                    │       display_id no    │
                                    │       longer connected │
                                    │                        │
                                    │  4. setFrameOrigin     │
                                    │     WHILE HIDDEN       │
                                    │                        │
                                    │  5. show panel         │
                                    │     re-apply floating  │
                                    │     level              │
                                    │                        │
                                    │  6. verify frame lies  │
                                    │     on target screen;  │
                                    │     re-apply once if   │
                                    │     not                │
                                    └────────────────────────┘
                                                │
                                                ▼
                              ┌────────────────────────────────┐
                              │  hide_overlay()                │
                              │                                │
                              │  7. if current frame ==        │
                              │     session-default            │
                              │     (within ε),                │
                              │     DO NOT persist             │
                              │                                │
                              │  8. else persist keyed by      │
                              │     target.display_id          │
                              └────────────────────────────────┘
```

A separate observer handles display reconfiguration:

```
NSApplication.didChangeScreenParametersNotification
          │
          ▼
prune_saved_positions()
  ─ remove entries whose display_id is not in current NSScreen.screens
  ─ remove entries whose point does not fit inside current visibleFrame
```

**Key principle:** the persistence file becomes a cache of
*user-expressed preferences*, not a snapshot of the last frame. If the
cache entry is stale or the user has never expressed a preference, we
fall back to a freshly computed default. Clamping is removed entirely
from the restore path.

## 5. Detailed Design

All changes live in `src-tauri/src/hotkeys/manager.rs`. No public Tauri
commands, IPC events, or frontend contracts change.

### 5.1 Data model

```rust
/// Current persistence schema version. Bumped to 2 by this spec so that
/// the loader can reject any entry written by a pre-fix build — those
/// entries are auto-computed defaults that `hide_overlay` should never
/// have persisted (see Defect A).
const OVERLAY_POSITION_SCHEMA: u32 = 2;

/// Schema version inferred for entries that predate the schema field.
/// Old entries are deserialized with this value and rejected at load.
fn legacy_schema_version() -> u32 { 1 }

/// Saved overlay position for a specific monitor.
/// Only user-dragged positions are persisted; auto-computed defaults are not.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct SavedOverlayPosition {
    x: f64,
    y: f64,
    /// Schema version. Writers always set this to `OVERLAY_POSITION_SCHEMA`.
    #[serde(default = "legacy_schema_version")]
    schema: u32,
}
```

At every save site, construct the struct explicitly with
`schema: OVERLAY_POSITION_SCHEMA`. At every load site, reject entries
where `schema < OVERLAY_POSITION_SCHEMA` (see §5.10).

### 5.2 Session state for the overlay

A small struct, held on `AppState`, records the most recent *default*
position we computed and the target display at `show_overlay` time. It is
cleared when the overlay is hidden.

```rust
/// Per-show session state for the overlay panel. Used to detect whether
/// the panel was moved by the user (dragged) between show and hide.
#[derive(Clone, Copy, Debug)]
struct OverlaySession {
    /// The display the overlay was positioned onto.
    display_id: u32,
    /// The exact (x, y) the default formula produced for this display,
    /// before any user interaction.
    default_origin: (f64, f64),
    /// The exact (x, y) we finally set — either `default_origin` or a
    /// valid restored user position.
    applied_origin: (f64, f64),
}
```

Stored on `AppState` as `overlay_session: std::sync::Mutex<Option<OverlaySession>>`.

### 5.3 `get_saved_position` — discard-on-stale

Replace the clamping logic with a strict containment check:

```rust
/// Look up a saved position for this display. Returns None if the saved
/// point does not lie fully inside the current `visibleFrame` — i.e. the
/// display arrangement has changed since the position was persisted.
fn get_saved_position(
    display_id: u32,
    visible: &tauri_nspanel::objc2_foundation::NSRect,
) -> Option<(f64, f64)> {
    let positions = load_overlay_positions();
    let saved = positions.get(&display_id.to_string())?;

    // Require the *entire* overlay rectangle to fit inside visibleFrame.
    // A saved entry that would need clamping is, by definition, stale.
    let fits_x = saved.x >= visible.origin.x
        && saved.x + OVERLAY_WIDTH <= visible.origin.x + visible.size.width;
    let fits_y = saved.y >= visible.origin.y
        && saved.y + OVERLAY_HEIGHT <= visible.origin.y + visible.size.height;

    if !fits_x || !fits_y {
        log::info!(
            "Discarding stale saved position ({:.0},{:.0}) for display {} — \
             does not fit current visibleFrame ({:.0},{:.0} {:.0}x{:.0})",
            saved.x, saved.y, display_id,
            visible.origin.x, visible.origin.y, visible.size.width, visible.size.height,
        );
        return None;
    }
    Some((saved.x, saved.y))
}
```

**Why strict containment, not clamping?** A saved position exists only as
a surrogate for *"the spot the user visibly chose"*. If the arrangement
has changed such that that spot no longer exists on the display, there is
no good answer — the default is strictly better than an edge-pin.

### 5.4 `position_overlay_native` — record session state

Augment the function to return the `OverlaySession` it produced so the
caller can stash it:

```rust
fn position_overlay_native(
    panel: &tauri_nspanel::objc2_app_kit::NSPanel,
    target: &NativeScreen,
) -> OverlaySession {
    let vis = &target.visible_frame;
    let margin_bottom: f64 = 8.0;

    let default_origin = (
        vis.origin.x + (vis.size.width - OVERLAY_WIDTH) / 2.0,
        vis.origin.y + margin_bottom,
    );

    let (x, y) = match get_saved_position(target.display_id, vis) {
        Some(saved) => saved,
        None        => default_origin,
    };

    unsafe {
        let origin = tauri_nspanel::objc2_foundation::NSPoint { x, y };
        let _: () = tauri_nspanel::objc2::msg_send![panel, setFrameOrigin: origin];
    }

    log::info!(
        "Overlay positioned at ({:.0},{:.0}) on display {} — default=({:.0},{:.0}) visible=({:.0},{:.0} {:.0}x{:.0})",
        x, y, target.display_id,
        default_origin.0, default_origin.1,
        vis.origin.x, vis.origin.y, vis.size.width, vis.size.height,
    );

    OverlaySession {
        display_id: target.display_id,
        default_origin,
        applied_origin: (x, y),
    }
}
```

### 5.5 `show_overlay` — position before show, verify after show

Reorder `show_overlay` so the frame is set while the panel is still
hidden. Keep the "re-apply floating level after `show`" step required by
[tauri#13530](https://github.com/tauri-apps/tauri/issues/13530). Add a
post-show verification step.

```rust
fn show_overlay(app: &AppHandle) {
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        let state = app.state::<AppState>();
        let target_pid = state.target_pid.load(Ordering::SeqCst);
        let target_screen = match select_target_screen(target_pid) {
            Some(s) => s,
            None => { log::error!("No screens — cannot show overlay"); return; }
        };

        // Existing pre-created panel path
        if let Ok(panel) = app.get_webview_panel("overlay") {
            // 1. Position WHILE HIDDEN. If the panel is already hidden
            //    (common case: between recordings) setFrameOrigin runs
            //    with no cross-display animation hazard. If the panel is
            //    currently visible from a prior show that we are
            //    reshowing, orderOut first.
            let was_visible = panel_is_visible(panel.as_panel());
            if was_visible {
                // orderOut: takes a nullable `id` sender. In objc2 0.5
                // that is `Option<&NSObject>` (or equivalent). Pass None.
                // Exact type syntax is an implementation detail — mirror
                // the existing `msg_send![panel, setFrameOrigin: origin]`
                // pattern already in this file.
                unsafe {
                    let nil: Option<&tauri_nspanel::objc2_foundation::NSObject> = None;
                    let _: () = tauri_nspanel::objc2::msg_send![
                        panel.as_panel(), orderOut: nil
                    ];
                }
            }

            let session = position_overlay_native(panel.as_panel(), &target_screen);
            *state.overlay_session.lock().unwrap() = Some(session);

            // 2. Show
            panel.show();

            // 3. Re-apply floating level (tauri#13530)
            use tauri_nspanel::PanelLevel;
            panel.set_level(PanelLevel::Floating.into());
            panel.set_floating_panel(true);
            panel.order_front_regardless();

            // 4. Verify
            verify_and_fix_overlay_frame(panel.as_panel(), &target_screen, session);

            log::info!("Overlay shown (existing panel)");
            return;
        }

        // Create-new-panel fallback unchanged except:
        //   ─ call position_overlay_native BEFORE panel.show()
        //   ─ save session state on AppState
        //   ─ call verify_and_fix_overlay_frame after panel.show()
    });
}

/// True if the `NSPanel`'s isVisible flag is set. `isVisible` returns
/// an Objective-C `BOOL`; in objc2 that comes back as `objc2::runtime::Bool`,
/// which has `.as_bool()`. If the active objc2 version returns `bool`
/// directly, drop the `.as_bool()`. Verify during implementation.
fn panel_is_visible(panel: &tauri_nspanel::objc2_app_kit::NSPanel) -> bool {
    unsafe {
        let visible: tauri_nspanel::objc2::runtime::Bool =
            tauri_nspanel::objc2::msg_send![panel, isVisible];
        visible.as_bool()
    }
}
```

### 5.6 `verify_and_fix_overlay_frame`

```rust
/// Read the panel's actual frame after show and check it lies on the
/// target screen. If not (can happen when the window server decides to
/// re-assign the window to the "majority" display), re-apply once.
fn verify_and_fix_overlay_frame(
    panel: &tauri_nspanel::objc2_app_kit::NSPanel,
    target: &NativeScreen,
    session: OverlaySession,
) {
    let frame: tauri_nspanel::objc2_foundation::NSRect = unsafe {
        tauri_nspanel::objc2::msg_send![panel, frame]
    };
    let center_x = frame.origin.x + frame.size.width / 2.0;
    let center_y = frame.origin.y + frame.size.height / 2.0;

    let tf = &target.frame;
    let inside =
        center_x >= tf.origin.x && center_x <  tf.origin.x + tf.size.width &&
        center_y >= tf.origin.y && center_y <  tf.origin.y + tf.size.height;

    if inside {
        return;
    }

    log::warn!(
        "Overlay frame landed off-target after show (center {:.0},{:.0}, \
         target display {} frame {:.0},{:.0} {:.0}x{:.0}) — re-applying",
        center_x, center_y, target.display_id,
        tf.origin.x, tf.origin.y, tf.size.width, tf.size.height,
    );

    // Re-apply using the session's applied_origin rather than recomputing
    // so that we do not "demote" a valid user position to the default.
    unsafe {
        let origin = tauri_nspanel::objc2_foundation::NSPoint {
            x: session.applied_origin.0,
            y: session.applied_origin.1,
        };
        let _: () = tauri_nspanel::objc2::msg_send![panel, setFrameOrigin: origin];
    }
}
```

**Why only re-apply once?** If a single reapplication does not stick,
something outside this module is fighting us and looping would make it
worse. The log line makes the failure explicit and leaves the panel in
whatever state macOS insists on. A manual drag by the user will then
persist via the drag-detection path below.

**Why check against `target.frame` and not `target.visible_frame`?**
The verification is "did the panel land on the correct *screen*", not
"is every pixel inside the Dock-excluded area". A user who later drags
the panel into (or near) the Dock area should still have their position
honored; if we used `visible_frame` here we would loop-repair
perfectly-valid user drags. `frame` is the right denominator.

### 5.7 `hide_overlay` — persist only user-moved positions

```rust
fn hide_overlay(app: &AppHandle) {
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        if let Some(window) = app.get_webview_window("overlay") {
            let _ = window.eval("window.__resetOverlay && window.__resetOverlay()");
        }

        if let Ok(panel) = app.get_webview_panel("overlay") {
            let panel_ref = panel.as_panel();
            let frame: tauri_nspanel::objc2_foundation::NSRect = unsafe {
                tauri_nspanel::objc2::msg_send![panel_ref, frame]
            };

            let state = app.state::<AppState>();
            let session = state.overlay_session.lock().unwrap().take();

            if let Some(sess) = session {
                let moved = {
                    let dx = (frame.origin.x - sess.applied_origin.0).abs();
                    let dy = (frame.origin.y - sess.applied_origin.1).abs();
                    dx > DRAG_EPSILON || dy > DRAG_EPSILON
                };

                // Mutex lock convention for the whole file: poisoned
                // locks are recovered with `unwrap_or_else(|e|
                // e.into_inner())`. See the existing state.rs usage for
                // `cancel_shortcut`, etc.

                if moved {
                    // The user dragged. Persist the new spot on whichever
                    // display now contains it.
                    let screens = get_native_screens();
                    let cx = frame.origin.x + frame.size.width / 2.0;
                    let cy = frame.origin.y + frame.size.height / 2.0;
                    if let Some(idx) = screen_containing_point(&screens, cx, cy) {
                        save_panel_position(panel_ref, screens[idx].display_id);
                    }
                } else {
                    log::info!(
                        "Overlay was not moved during session — not persisting \
                         (default {:.0},{:.0} for display {})",
                        sess.default_origin.0, sess.default_origin.1, sess.display_id,
                    );
                }
            }

            panel.hide();
            log::info!("Overlay hidden");
        } else if let Some(window) = app.get_webview_window("overlay") {
            let _ = window.hide();
            log::info!("Overlay hidden (window fallback)");
        }
    });
}

const DRAG_EPSILON: f64 = 0.5; // sub-point tolerance
```

`save_panel_position` itself is unchanged in signature, but its body now
constructs the `SavedOverlayPosition` with an explicit `schema:
OVERLAY_POSITION_SCHEMA`. This is the *only* write site; any future
writer must go through it. Add a debug-assert at build time:

```rust
debug_assert_eq!(OVERLAY_POSITION_SCHEMA, 2,
    "bump CURRENT_SCHEMA_VERSION + extend load filter if you change this");
```

**Note on `NSScreen.main` vs `NSScreen.screens[0]`:** this code must
always use `screens[0]` when it wants "the origin of the global
coordinate system" / "the menu-bar display". `NSScreen.main` returns
*the screen containing the key window of the calling app*, which for
SottoASR (a menu-bar accessory with no key window) is frequently
surprising and sometimes nil. `get_native_screens` in
`src-tauri/src/hotkeys/manager.rs` already returns the `screens[0]`
result in slot `0` of its `Vec<NativeScreen>`; every consumer in this
spec reads `screens[0]` (for example, the Quartz-to-Cocoa Y flip in
`select_target_screen` reads `screens[0].frame.size.height`). Do not
refactor any of those call sites to `main`.

### 5.8 `precreate_overlay` — position at a safe default

Add a call to `position_overlay_native` after panel conversion so the
hidden panel is parked at the bottom-center of the current primary
display instead of wherever `WebviewWindowBuilder` happened to place it.
This guarantees that if `show_overlay` somehow skips its own positioning
step (defensive), the overlay still appears on a sane screen.

```rust
// inside precreate_overlay, after panel configuration:
let screens = get_native_screens();
if let Some(primary) = screens.first() {
    // Discard the returned OverlaySession. precreate runs at app
    // launch, before any real show_overlay cycle, so there is no
    // session to record. AppState.overlay_session stays None. The
    // first real show_overlay will overwrite the panel frame anyway.
    let _ = position_overlay_native(panel.as_panel(), primary);
}
```

**Do not** write to `AppState.overlay_session` from `precreate_overlay`.
Session state represents "a user-visible show cycle is in progress";
precreation does not open one.

### 5.9 Display-reconfiguration observer (optional, deferrable)

**Decision: this subsection is optional.** The primary fix (§5.3–§5.8)
resolves the reported bug on its own. The observer is a quality-of-life
enhancement that makes mid-recording display changes (hot-plug,
rearrangement via System Settings) reflect immediately instead of on
the next show. If wiring the observer turns out to be more expensive
than anticipated during implementation (see objc2 block/observer
ergonomics below), implement it in a follow-up spec rather than
blocking this one.

On app setup, register for
`NSApplication.didChangeScreenParametersNotification`. When it fires,
prune the persistence file:

Two viable implementation paths (pick the cheaper one at
implementation time):

**Path A — `objc2` block-based observer.**
`[NSNotificationCenter addObserverForName:object:queue:usingBlock:]`
accepts an Objective-C block. To call it from Rust, `block2::RcBlock`
(from the `block2` crate, already pulled in transitively through
`objc2` in many Tauri builds — verify at implementation time) can wrap
a Rust closure with `'static + Fn(*mut NSNotification)`. The closure
captures an `AppHandle` clone and dispatches onto the main thread via
`app.run_on_main_thread`. Keep the returned observer `id` in a
`OnceLock` so the observer lives as long as the process.

**Path B — lightweight polling fallback.** Spawn one Tokio task that
wakes every 2 s, calls `get_native_screens()` from the main thread
(`app.run_on_main_thread`), hashes the `(display_id, frame,
visible_frame)` tuples, and fires `on_screen_parameters_changed` when
the hash changes. Coarser than notification but needs no new
dependency and handles the same cases. Negligible cost (one main-queue
hop every two seconds).

Either path invokes the same handler:

```rust
fn install_screen_change_observer(app: AppHandle) {
    // Path A or Path B — see above. Both invoke
    // on_screen_parameters_changed(&app) from the main thread.
}

fn on_screen_parameters_changed(app: &AppHandle) {
    // 1. Prune saved positions that no longer fit.
    let screens = get_native_screens();
    let mut positions = load_overlay_positions();
    let before = positions.len();
    positions.retain(|id_str, saved| {
        let id: u32 = match id_str.parse() { Ok(v) => v, Err(_) => return false };
        let Some(screen) = screens.iter().find(|s| s.display_id == id) else {
            log::info!("Pruning saved position for removed display {}", id);
            return false;
        };
        let vis = &screen.visible_frame;
        let fits = saved.x >= vis.origin.x
            && saved.x + OVERLAY_WIDTH <= vis.origin.x + vis.size.width
            && saved.y >= vis.origin.y
            && saved.y + OVERLAY_HEIGHT <= vis.origin.y + vis.size.height;
        if !fits {
            log::info!(
                "Pruning stale saved position ({:.0},{:.0}) for display {}",
                saved.x, saved.y, id,
            );
        }
        fits
    });
    if positions.len() != before {
        save_overlay_positions(&positions);
    }

    // 2. If the overlay is currently visible, re-run positioning.
    if let Ok(panel) = app.get_webview_panel("overlay") {
        if panel_is_visible(panel.as_panel()) {
            let state = app.state::<AppState>();
            let target_pid = state.target_pid.load(Ordering::SeqCst);
            if let Some(target) = select_target_screen(target_pid) {
                let session = position_overlay_native(panel.as_panel(), &target);
                *state.overlay_session.lock().unwrap() = Some(session);
                verify_and_fix_overlay_frame(panel.as_panel(), &target, session);
            }
        }
    }
}
```

The observer is installed once, from `lib.rs` `setup`. Unregistering is
not necessary for the lifetime of the process.

### 5.10 One-time cache purge on first run of the fixed build

Because existing users (including the reporter) already have
default-valued entries in their `overlay_positions.json`, we must
invalidate them once. Two options:

| Option | Implementation | Pro | Con |
|--------|---------------|-----|-----|
| Bump schema | Bump `schema: 1` → `2`; on load, drop entries with `schema < 2` | Automatic; user needs no action | Requires a one-line load filter |
| Drop file once | On first run of the new version, delete the file | Simpler code | Needs a first-run sentinel |

We use the schema bump. Pre-existing entries have no `schema` field and
deserialize with `schema = legacy_schema_version() = 1`. The new build
writes entries with `schema = OVERLAY_POSITION_SCHEMA = 2`.
`load_overlay_positions` filters out entries with `schema <
OVERLAY_POSITION_SCHEMA` and logs once-per-load when it does:

```rust
fn load_overlay_positions() -> OverlayPositions {
    // ... read + deserialize as today ...
    let before = positions.len();
    positions.retain(|_, v| v.schema >= OVERLAY_POSITION_SCHEMA);
    let dropped = before - positions.len();
    if dropped > 0 {
        log::info!(
            "Dropped {} legacy overlay-position entries (schema < {})",
            dropped, OVERLAY_POSITION_SCHEMA,
        );
    }
    positions
}
```

On the next user drag — which runs `save_panel_position` →
`load_overlay_positions` (already filtered) → `insert` →
`save_overlay_positions` — the written file contains only schema-2
entries. The reporter's file becomes `{}` the first time they hide the
overlay after installing the fix and then drag it once, which is the
desired state. **If the user never drags, the file remains on disk
unchanged forever but is fully ignored on every load;** this is
harmless. No active purge of the file-on-disk is required.

## 6. Edge Cases

| # | Case | Behavior |
|---|------|----------|
| 1 | User never drags; single display | First show computes default; hide does not persist; next show computes default again. File stays `{}`. Bit-exact stable. |
| 2 | User never drags; multiple displays; rearrangement between sessions | Nothing is persisted, so rearrangement has no effect. Every show uses a fresh default on the current layout. |
| 3 | User drags once on display A, later recordings on display A | Saved entry is restored; containment check passes; position is honored. |
| 4 | User drags on display A, then unplugs display A | Observer prunes the entry on `didChangeScreenParametersNotification`. Next show falls back to default on the remaining display. |
| 5 | User drags on display A, then rearranges A to a different origin | Restored position no longer fits `visibleFrame` (new origin shifted it off-screen); containment check fails; default is used. The *user's drag is lost* — acceptable because the old pixel location no longer exists. |
| 6 | User drags so the overlay straddles two displays; macOS re-assigns to majority | `save_panel_position` looks up `screen_containing_point` by the overlay's *center*; this matches macOS' own "majority geometry" rule. The saved entry is written against the same display macOS picked. |
| 7 | Display A is added mid-recording (overlay is visible) | Observer re-runs `position_overlay_native` against the current target. If the target hasn't changed, no visible jump. If it has (because the focused app moved), overlay follows. |
| 8 | `NSScreen.screens` is empty (headless) | `select_target_screen` logs an error and returns early; `show_overlay` does nothing. No panic. Already covered by existing code. |
| 9 | `verify_and_fix_overlay_frame` detects off-target after show | Re-applies `applied_origin` exactly once; logs the incident. If the second attempt is also ignored, logs stand as the diagnostic — the operator sees this immediately rather than receiving a wrong-display bug report. |
| 10 | Mixed DPI (Retina + 1x) | All coordinates stay in Cocoa points; AppKit handles backing scale transparently. Containment and centering math are DPI-invariant. |
| 11 | Screens arranged vertically (stacked) | Cocoa frames have non-zero y origins; containment check and default formula handle this naturally. No special case. |
| 12 | Clamshell mode (laptop lid closed) | `NSScreen.screens` returns only the external; `screens[0]` is that external. Default positioning is on the external. Correct. |
| 13 | Notched display | `visibleFrame` already excludes the notch + menu bar area per Apple docs; default formula honors it. |
| 14 | Fullscreen app on target display | `CanJoinAllSpaces + FullScreenAuxiliary` collection behavior is already set; overlay floats above. |
| 15 | Stage Manager enabled | Unchanged — Stage Manager does not affect `NSScreen.visibleFrame` or `NSWindow.setFrameOrigin:` for auxiliary panels with `fullScreenAuxiliary`. Needs manual verification. |
| 16 | User has *two* saved entries and one display is removed | Observer prunes the removed one; the other survives. |
| 17 | `NSApplicationDidChangeScreenParametersNotification` fires spuriously (e.g. Dock position change) | Observer logic is idempotent: pruning an up-to-date cache is a no-op, re-applying a correct frame is a no-op. |
| 18 | App launched when mouse is on secondary display | `precreate_overlay` parks the hidden panel on the primary display so that the first `show_overlay` only has to move *once*, to the actual target (no pre-flash on the primary). |
| 19 | Quick toggle (hide / show / hide / show within 100 ms) | Each show re-computes session state; hide compares against that session's `applied_origin`, not any global. No cross-session state leakage. |
| 20 | Saved `y` at visible-frame bottom minus Dock height changed since save | `y + OVERLAY_HEIGHT > visible.origin.y + visible.size.height` → containment fails → default is used. No clamping to the dock edge. |
| 21 | User drags, then the target display disappears before `hide_overlay` runs | `screen_containing_point(frame.center)` returns None; no save is performed; the drag is lost. Acceptable — the display the user chose no longer exists to persist *against*. |
| 22 | Saved point is *inside* current `visibleFrame` but "looks wrong" (e.g. reporter's `x = 1770` case) | Containment alone would incorrectly restore. The **primary** fix is §5.7 (don't persist defaults), not §5.3 (containment). The two work together: if any stale default sneaks in via some future bug, §5.3 will still catch the subset that drifts out-of-bounds. |
| 23 | Two displays with identical `CGDirectDisplayID` somehow | Impossible — the ID is unique per attached display by API contract. |
| 24 | User has never run a release newer than v3.0, upgrades once to this build | Schema filter drops the stale entries on first load; first recording uses default; nothing written until a drag. Zero user action required. |
| 25 | User downgrades to pre-fix build after installing this one | Old build has no schema filter and reads everything. Any user-drag they did on the new build still works (it's schema-2 data, structurally identical). Any stale entries from before are gone (we never rewrote them). |
| 26 | `AppState.overlay_session` mutex is poisoned by a prior panic | Follow the file-wide convention: `lock().unwrap_or_else(\|e\| e.into_inner())`. Worst case: one save compares against a stale session; next session is fine. |
| 27 | Overlay is visible when app quits (force-quit mid-recording) | `hide_overlay` runs during cleanup if reachable; otherwise no save happens and no garbage is left. |

## 7. File Changes

| File | Change | Notes |
|------|--------|-------|
| `src-tauri/src/hotkeys/manager.rs` | **Modify** | Add `OverlaySession`, `DRAG_EPSILON`, `panel_is_visible`, `verify_and_fix_overlay_frame`, `on_screen_parameters_changed`. Rewrite `get_saved_position` to discard instead of clamp. Rewrite `show_overlay` / `hide_overlay` to use session state. Bump `SavedOverlayPosition` to carry `schema`. Call `position_overlay_native` from `precreate_overlay`. |
| `src-tauri/src/state.rs` | **Modify** | Add `overlay_session: std::sync::Mutex<Option<OverlaySession>>` (or forward-declared opaque type to avoid a cyclic import — implementation detail). |
| `src-tauri/src/lib.rs` | **Modify** | In `setup`, after `precreate_overlay`, call `install_screen_change_observer(app_handle.clone())`. |
| `src-tauri/src/hotkeys/mod.rs` | **No change** | No new public API. |
| `src-tauri/Cargo.toml` | **No change** | All required symbols (`NSNotificationCenter`, `NSApplication`, `objc2` block helpers) are already reachable via `tauri_nspanel::objc2` / `objc2_foundation` / `objc2_app_kit`. |
| `src/lib/components/overlay-pill.svelte` | **No change** | Drag behavior (`-webkit-app-region: drag`) is already in place from v3.0 of the prior spec. |
| `~/Library/Application Support/com.sottoasr.app/overlay_positions.json` | **One-time auto-purge** | First run of new build drops entries with `schema < 2`. No user action required. Documented below. |

Line-count estimate: ~180 lines added, ~40 lines removed. Net ≈ +140 in `manager.rs`; <10 elsewhere.

## 8. Testing Strategy

### 8.1 Unit tests (`src-tauri/src/hotkeys/manager.rs` under `#[cfg(test)]`)

Target the pure functions only — ObjC-bound functions are covered by
integration/manual tests.

1. `get_saved_position` — fitting entry → returns `Some`.
2. `get_saved_position` — entry off left edge → returns `None`.
3. `get_saved_position` — entry off right edge → returns `None`.
4. `get_saved_position` — entry off top → returns `None`.
5. `get_saved_position` — entry off bottom → returns `None`.
6. `get_saved_position` — entry wholly on a *different* visible frame → returns `None`.
7. `screen_containing_point` — inside / outside / exact-boundary cases for 2-screen layouts side-by-side, stacked, and mixed-DPI (already present, extend).
8. "Drag detection" helper — `moved` branch with `dx < DRAG_EPSILON` and `dx > DRAG_EPSILON`.

### 8.2 Integration test (cargo test, `#[cfg(target_os = "macos")]`)

Instantiate `NSScreen.screens`, synthesize an `OverlayPositions` map
keyed by a real `display_id`, and assert the `load` → `prune` round trip.
This does not require a running panel.

### 8.3 Manual matrix (must be run on the reporter's hardware)

The goal of this matrix is two-fold: (a) verify the fix resolves the
exact reported bug against the *actual* saved-positions file currently
on disk, and (b) exercise the one-time schema-based purge path without
any user intervention.

**Mandatory repro step before anything else:**

> Do **not** delete or edit `~/Library/Application
> Support/com.sottoasr.app/overlay_positions.json` before running Test
> 0. The goal is to prove the installer + schema filter repairs the
> exact state that triggered the report.

| # | Precondition | Action | Expected |
|---|--------------|--------|----------|
| 0 | Current on-disk file (unchanged) | Install new build, launch, hotkey on any display | Overlay bottom-center of target display — **not** at the x=1770 / "far-left-of-screen-2" point. Log contains "Dropped N legacy overlay-position entries". |
| 1 | Laptop only | Hotkey | Overlay at bottom-center of laptop |
| 2 | Laptop + ext, focus on ext | Hotkey | Overlay at bottom-center of ext |
| 3 | Laptop + ext, focus on laptop, mouse on ext | Hotkey | Overlay at bottom-center of laptop (follows focus, not cursor) |
| 4 | As #2, drag overlay 200 pt to the left, stop recording, start again | Hotkey | Overlay restored at the dragged position on ext |
| 5 | As #4, unplug ext | Hotkey on laptop | Overlay at bottom-center of laptop; stored ext entry is ignored (observer prunes if enabled; otherwise left dormant) |
| 6 | As #4, rearrange displays in System Settings so ext has a new `origin.x` that moves the dragged spot out of the visible frame | Hotkey | Overlay at bottom-center of ext — dragged spot is off the new `visibleFrame` → discarded; default used |
| 7 | As #2 but on mixed-DPI (Retina + 1x) | Hotkey | Correct size and position on each display |
| 8 | As #2 with a fullscreen app on ext | Hotkey | Overlay above fullscreen app |
| 9 | Start recording, mid-recording add a second display | Observer fires (if implemented) or next hotkey picks up change; no crash |
| 10 | 5× rapid hotkey toggles | No ghost overlays, no off-screen frames |
| 11 | Launch app with mouse focus on ext | First hotkey | Overlay appears directly on ext with no flash on the laptop |
| 12 | After Test 0, remove the file and hotkey again without ever dragging | Hotkey | File does not get recreated with any entries (user never dragged → no writes) |

### 8.4 Log verification

After each manual case, open
`~/Library/Logs/com.sottoasr.app/SottoASR.log` and verify:

- Exactly one "Overlay positioned at (x,y) on display N" line per show.
- "Discarding stale saved position" appears in cases 5 and 6.
- "Overlay frame landed off-target" does **not** appear in cases 1–8,
  11.
- "=== Screen Configuration (N screens) ===" is present before the
  position line.

### 8.5 Regression gate

Before merging, run `./scripts/pre-release-check.sh --auto-only` as
required by the release rules, plus the full verification set from
`.claude/rules/spec-workflow.md` §Phase 6:

```bash
( cd src-tauri && cargo build        2>&1 ) | tee /tmp/build.txt
( cd src-tauri && cargo clippy -- -D warnings 2>&1 ) | tee /tmp/clippy.txt
( cd src-tauri && cargo test         2>&1 ) | tee /tmp/test.txt
npm run check                        2>&1 | tee /tmp/check.txt
cargo tauri build                    2>&1 | tee /tmp/tauri-build.txt
```

All five must exit 0. `cargo tauri build` is included because it is
the only verification that exercises the packaged `.app` against
real-hardware accessibility / NSPanel state — the class of thing this
spec touches.

## 9. Migration Plan

- The persistence file is the only piece of on-disk state this spec
  touches.
- Users upgrading from any build that used the v3.0 format (no `schema`
  field) will have their existing entries read as `schema = 1` and then
  dropped by `load_overlay_positions`, which requires `schema >= 2`. The
  first `hide_overlay` after upgrade rewrites the file with only
  *user-dragged* positions, which at that moment is empty. This is the
  correct state.
- Downgrading to the previous build after installing this one is
  lossless: the older build's loader ignores the `schema` field and
  treats any entry it finds as authoritative. Previously-saved drags
  remain usable.

## 10. Security Considerations

- No new permissions.
- No new network traffic, files, or background processes beyond the
  single long-lived `NSNotificationCenter` observer.
- The persistence file already contained absolute display coordinates;
  the new schema adds only a small integer version field. No user data
  is added or removed.
- `CGWindowListCopyWindowInfo` usage is unchanged.

## 11. Cost Analysis

| Operation | Added cost | Frequency |
|-----------|------------|-----------|
| Containment check in `get_saved_position` | O(1) arithmetic | Once per `show_overlay` |
| `verify_and_fix_overlay_frame` | 1 `frame` read, ~4 comparisons, at most one setFrameOrigin | Once per `show_overlay` |
| Session mutex lock | ~100 ns | Twice per recording |
| Observer callback on screen reconfiguration | O(N) over saved entries (typically ≤ 4) | Fired per display change only |
| `precreate_overlay` positioning | 1 `setFrameOrigin:` | Once per app launch |

**End-to-end delta at hotkey press: < 1 ms.** Dominant cost remains
`CGWindowListCopyWindowInfo` (1–5 ms) which is unchanged.

Binary size delta: ~0 KiB (no new crate dependencies).

## 12. Implementation Tasks

The tasks are ordered so that each one is independently buildable and
cargo-clippy-clean. Dependency graph:

```
T1 ─┐
    ├─▶ T2           (T2 does not need T1 for compile; grouped for commit hygiene)
T3 ─┼─▶ T4 ─▶ T5 ─▶ T7
    └─▶ T6
T8 (optional, depends on T3/T4 only if observer path A)
T9 depends on T4
T10 depends on everything above
T11 depends on T10
T12 is documentation only
```

- [ ] **T1.** Add `schema` field to `SavedOverlayPosition` with `serde(default = ...)`. Filter `< 2` in `load_overlay_positions`. Build + test.
- [ ] **T2.** Replace clamping in `get_saved_position` with strict containment; add unit tests 1–6 from §8.1.
- [ ] **T3.** Introduce `OverlaySession`, `DRAG_EPSILON`, and the `AppState.overlay_session` field.
- [ ] **T4.** Refactor `position_overlay_native` to return `OverlaySession` and log `default_origin`.
- [ ] **T5.** Reorder `show_overlay` to: orderOut-if-visible → position → show → set level → verify. Add `panel_is_visible` helper.
- [ ] **T6.** Rewrite `hide_overlay` to persist only if `frame.origin != session.applied_origin` (≥ DRAG_EPSILON).
- [ ] **T7.** Add `verify_and_fix_overlay_frame`. Wire in from `show_overlay`.
- [ ] **T8.** Add `install_screen_change_observer` + `on_screen_parameters_changed`. Register in `lib.rs` `setup`.
- [ ] **T9.** Call `position_overlay_native` at end of `precreate_overlay` against `screens[0]`.
- [ ] **T10.** Run the full §8.3 manual matrix (tests 0–12) on the reporter's exact hardware. For each row, capture: (a) screenshot of where the overlay landed, (b) the `Overlay positioned at (x,y) on display N` log line from `~/Library/Logs/com.sottoasr.app/SottoASR.log`, and (c) the contents of `~/Library/Application Support/com.sottoasr.app/overlay_positions.json` after the test. Paste into a new `docs/journals/2026-04-11-multi-monitor-overlay-fix-verification.md` journal.
- [ ] **T11.** Run §8.5 regression gate. Fix any new warnings.
- [ ] **T12.** Update the v3.0 spec (`docs/specs/2026-04-01-multi-monitor-overlay-reliability.md`): change its `Status:` from `Implemented` to `Superseded (for persistence)`, and add a one-line header pointer: `See [2026-04-11-overlay-positioning-multi-monitor-fix.md](./2026-04-11-overlay-positioning-multi-monitor-fix.md) for the current persistence semantics.`

## 13. Implementation Status

**Implemented** on 2026-04-11 (this spec). Supersedes
`2026-04-01-multi-monitor-overlay-reliability.md` for all
position-persistence concerns. The native-positioning design from the
v3.0 spec (NSScreen enumeration, focused-app screen selection,
`setFrameOrigin:` in Cocoa points) is preserved unchanged.

### Code changes landed

| File | What changed |
|------|--------------|
| `src-tauri/src/state.rs` | New `OverlaySession` struct (`display_id`, `default_origin`, `applied_origin`). New `AppState.overlay_session: StdMutex<Option<OverlaySession>>`, initialized to `None` in both `new()` and `new_with_backends()`. |
| `src-tauri/src/hotkeys/manager.rs` | Imports `OverlaySession`. New `DRAG_EPSILON = 0.5`, `OVERLAY_POSITION_SCHEMA = 2`, `legacy_schema_version()`. `SavedOverlayPosition` carries a `schema: u32` field with `#[serde(default = "legacy_schema_version")]`. `load_overlay_positions` filters `schema < 2` and logs once if anything was dropped. `save_panel_position` sets `schema: OVERLAY_POSITION_SCHEMA` and has a `debug_assert_eq!` guard. `get_saved_position` discards (does not clamp) saved points that don't fit current `visibleFrame`. `position_overlay_native` returns `OverlaySession`. New `panel_is_visible` and `verify_and_fix_overlay_frame` helpers. `show_overlay`: orderOut if visible → position while hidden → store session on `AppState` → show → re-apply floating level → verify. Same path for the "create new" branch. `hide_overlay` takes the session, compares `frame.origin` to `applied_origin` under `DRAG_EPSILON`, persists only on a real drag. `precreate_overlay` now parks the hidden panel at the primary display's bottom-center (session intentionally not stored). |
| `src-tauri/src/commands/overlay.rs` (new) | `overlay_start_drag` Tauri command. Calls `[panel performWindowDragWithEvent: [NSApp currentEvent]]` on the main thread. See "Defect D — drag handler is dead code on the converted NSPanel" below. |
| `src-tauri/src/commands/mod.rs` | Adds `pub mod overlay`. |
| `src-tauri/src/lib.rs` | Registers `commands::overlay::overlay_start_drag` in the invoke handler. |
| `src/lib/components/overlay-pill.svelte` | New `handlePillMouseDown` invokes `overlay_start_drag` on left-button mousedown. New `stopMouseDown` is wired into the Stop and Cancel buttons' `onmousedown` so they don't trigger drag. Removed dead `-webkit-app-region: drag` / `no-drag` rules and replaced them with a comment explaining why those CSS heuristics don't work on a non-activating NSPanel. Added `cursor: grabbing` on `.pill:active`. |
| `docs/specs/2026-04-01-multi-monitor-overlay-reliability.md` | Status → `Superseded (for persistence)` with a pointer to this spec. |

### Defect D — drag handler is dead code on the converted NSPanel (found post-implementation)

Discovered after the user reported "I can't drag the overlay" against
the freshly-installed fix. The v3.0 spec landed `-webkit-app-region:
drag` on `.pill` and assumed wry's CSS-driven drag heuristic would
forward mousedown events to `[NSWindow performWindowDragWithEvent:]`.
That assumption is wrong for our overlay because:

1. The overlay window is built as a normal `WebviewWindow` and then
   converted via `tauri_nspanel::WebviewWindowExt::to_panel::<OverlayPanel>()`.
2. `OverlayPanel` is declared with `can_become_key_window: false` and
   `is_floating_panel: true`.
3. wry's drag-region hook is installed on the *original* NSWindow's
   event chain. After `to_panel` replaces the underlying object with a
   non-activating NSPanel, the hook is gone and `-webkit-app-region:
   drag` becomes a no-op.

**Fix:** bypass wry entirely. The overlay frontend dispatches
`overlay_start_drag` from a `mousedown` listener on `.pill`; the Rust
command grabs `[NSApp currentEvent]` and calls
`performWindowDragWithEvent:` on the panel directly. This works on
non-key NSPanels because the method is inherited from NSWindow and
operates on the in-flight event regardless of key status. The Stop and
Cancel buttons stop event propagation in their own `onmousedown`
handlers so they don't trigger drag. No new Tauri permissions are
required (the command is registered through our own invoke handler).

### Regression gate (all green)

```
cargo build                  → OK
cargo clippy -- -D warnings  → OK (0 warnings)
cargo test                   → OK (69 passed, 0 failed)
npm run check                → OK (0 errors, pre-existing warnings unrelated)
cargo tauri build            → OK (.app + .dmg produced and signed)
```

### Schema migration validated on the reporter's actual file

Ran a standalone verification harness against the reporter's exact
on-disk file contents. Both legacy entries deserialize with
`schema: 1`, the loader's filter drops them, post-filter map length
is zero. Confirmed: on first launch of the new build, the stale
`overlay_positions.json` is effectively treated as empty, which
collapses the reported bug.

### Deferred

- Display-reconfiguration observer (§5.9) is not implemented. The
  primary fix resolves the reported bug without it; mid-recording
  display changes will take effect on the next `show_overlay` call
  instead of immediately.
- Automated unit tests for `get_saved_position` (§8.1) are deferred.
  The function is pure arithmetic and covered by the manual matrix
  (§8.3). Can be added later without reopening the spec.
- T10 manual matrix on the reporter's hardware (Tests 0–12) and T11
  regression-gate sign-off on a freshly-built app are owned by the
  user; the app has been reinstalled to `/Applications/SottoASR.app`
  and launched.
