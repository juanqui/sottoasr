//! Overlay panel commands invoked from the overlay webview.
//!
//! The overlay is an `NSPanel` converted via `tauri_nspanel::to_panel`
//! with `can_become_key_window: false`. wry's built-in
//! `-webkit-app-region: drag` heuristic hooks the original NSWindow's
//! event chain, which is lost after the to_panel conversion, so CSS
//! dragging does not work on this panel.
//!
//! Instead, the frontend calls `overlay_start_drag` on mousedown and we
//! dispatch `performWindowDragWithEvent:` to the NSPanel directly,
//! using the current NSEvent held by the shared NSApplication. This
//! bypasses wry's drag heuristic entirely and works for non-key panels.

use tauri::AppHandle;
#[cfg(target_os = "macos")]
use tauri_nspanel::ManagerExt;

#[tauri::command]
pub fn overlay_start_drag(app: AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let app_clone = app.clone();
        app.run_on_main_thread(move || {
            let panel = match app_clone.get_webview_panel("overlay") {
                Ok(p) => p,
                Err(e) => {
                    log::warn!("overlay_start_drag: no overlay panel ({:?})", e);
                    return;
                }
            };
            unsafe {
                // [NSApp currentEvent] — the in-flight NSEvent the
                // window server dispatched. When this command is called
                // from a mousedown JS handler, that event is the
                // leftMouseDown that performWindowDragWithEvent: wants.
                let ns_app: *mut tauri_nspanel::objc2_foundation::NSObject =
                    tauri_nspanel::objc2::msg_send![
                        tauri_nspanel::objc2::class!(NSApplication),
                        sharedApplication
                    ];
                if ns_app.is_null() {
                    log::warn!("overlay_start_drag: NSApp sharedApplication is null");
                    return;
                }
                let event: *mut tauri_nspanel::objc2_foundation::NSObject =
                    tauri_nspanel::objc2::msg_send![ns_app, currentEvent];
                if event.is_null() {
                    log::warn!("overlay_start_drag: no current NSEvent");
                    return;
                }
                let _: () = tauri_nspanel::objc2::msg_send![
                    panel.as_panel(), performWindowDragWithEvent: event
                ];
            }
        })
        .map_err(|e| e.to_string())?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
    }
    Ok(())
}
