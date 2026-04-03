//! Tray icon occlusion detection for macOS.
//!
//! macOS silently hides menu bar icons when the bar is too crowded (especially
//! on MacBooks with a notch). There is no public API to detect this reliably,
//! but we can check the `occlusionState` of the status item's window as a
//! heuristic — the same approach Tailscale uses in production.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::AppHandle;

/// Whether we've already warned the user in this session.
static OCCLUSION_WARNED: AtomicBool = AtomicBool::new(false);

/// Start a background monitor that periodically checks whether the tray icon's
/// window is occluded (hidden behind the notch or pushed out by other icons).
///
/// If occlusion is detected, a macOS notification is shown once per session
/// (resets if the icon becomes visible again).
pub fn start_occlusion_monitor(app: &AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || {
        // Give the system time to settle after launch.
        std::thread::sleep(Duration::from_secs(15));

        loop {
            let occluded = unsafe { is_status_item_occluded() };

            if occluded && !OCCLUSION_WARNED.swap(true, Ordering::Relaxed) {
                log::warn!(
                    "Tray icon appears to be occluded (hidden behind notch or menu bar overflow)"
                );
                show_occlusion_notification(&handle);
            } else if !occluded && OCCLUSION_WARNED.load(Ordering::Relaxed) {
                OCCLUSION_WARNED.store(false, Ordering::Relaxed);
                log::info!("Tray icon is visible again");
            }

            std::thread::sleep(Duration::from_secs(30));
        }
    });
}

/// Check whether our app's status bar window is occluded.
///
/// Iterates `[NSApp windows]` looking for an `NSStatusBarWindow`, then reads
/// its `occlusionState`. Returns `true` if the window exists but is NOT
/// visible, `false` otherwise (including when no status bar window is found).
///
/// # Safety
/// Uses raw `objc_msgSend` to access AppKit classes. Must be called while a
/// run-loop is active (guaranteed in a Tauri app).
unsafe fn is_status_item_occluded() -> bool {
    use std::ffi::{CStr, c_char, c_void};
    use std::mem::transmute;

    extern "C" {
        fn objc_getClass(name: *const c_char) -> *const c_void;
        fn sel_registerName(name: *const c_char) -> *const c_void;
        fn objc_msgSend();
    }

    type SendObj = unsafe extern "C" fn(*const c_void, *const c_void) -> *const c_void;
    type SendU64 = unsafe extern "C" fn(*const c_void, *const c_void) -> u64;
    type SendObjIdx =
        unsafe extern "C" fn(*const c_void, *const c_void, u64) -> *const c_void;
    type SendStr = unsafe extern "C" fn(*const c_void, *const c_void) -> *const c_char;

    let send_obj: SendObj = transmute(objc_msgSend as *const c_void);
    let send_u64: SendU64 = transmute(objc_msgSend as *const c_void);
    let send_obj_idx: SendObjIdx = transmute(objc_msgSend as *const c_void);
    let send_str: SendStr = transmute(objc_msgSend as *const c_void);

    // [NSApplication sharedApplication]
    let nsapp_cls = objc_getClass(c"NSApplication".as_ptr());
    if nsapp_cls.is_null() {
        return false;
    }
    let app = send_obj(nsapp_cls, sel_registerName(c"sharedApplication".as_ptr()));
    if app.is_null() {
        return false;
    }

    // [app windows]
    let windows = send_obj(app, sel_registerName(c"windows".as_ptr()));
    if windows.is_null() {
        return false;
    }

    let count = send_u64(windows, sel_registerName(c"count".as_ptr()));
    let sel_object_at = sel_registerName(c"objectAtIndex:".as_ptr());
    let sel_class_name = sel_registerName(c"className".as_ptr());
    let sel_utf8 = sel_registerName(c"UTF8String".as_ptr());
    let sel_occlusion = sel_registerName(c"occlusionState".as_ptr());

    // NSWindowOcclusionStateVisible = 1 << 1 = 2
    const NS_WINDOW_OCCLUSION_STATE_VISIBLE: u64 = 1 << 1;

    for i in 0..count {
        let window = send_obj_idx(windows, sel_object_at, i);
        if window.is_null() {
            continue;
        }

        // Get the class name string
        let cls_ns_str = send_obj(window, sel_class_name);
        if cls_ns_str.is_null() {
            continue;
        }
        let cls_cstr = send_str(cls_ns_str, sel_utf8);
        if cls_cstr.is_null() {
            continue;
        }

        let class_name = CStr::from_ptr(cls_cstr).to_string_lossy();
        if class_name.contains("StatusBar") {
            let occlusion_state = send_u64(window, sel_occlusion);
            let is_visible =
                (occlusion_state & NS_WINDOW_OCCLUSION_STATE_VISIBLE) != 0;
            return !is_visible;
        }
    }

    // No status bar window found at all — not necessarily occluded,
    // could be a timing issue on launch.
    false
}

/// Show a macOS notification informing the user that the tray icon is hidden.
fn show_occlusion_notification(app: &AppHandle) {
    let settings = crate::commands::settings::load_persisted_settings();
    let shortcut = settings.open_settings_shortcut;

    let message = format!(
        "Your menu bar may be too crowded. Press {} to open Settings, or rearrange your menu bar icons.",
        shortcut_display_name(&shortcut)
    );

    // Use osascript for a simple notification — no extra plugin dependency.
    let script = format!(
        "display notification \"{}\" with title \"SottoASR\" subtitle \"Menu bar icon hidden\"",
        message.replace('\"', "\\\"")
    );
    if let Err(e) = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
    {
        log::error!("Failed to show occlusion notification: {}", e);
    }

    // Also emit an event so the frontend can react if a window is open.
    let _ = tauri::Emitter::emit(app, "tray-icon-occluded", true);
}

/// Convert a Tauri shortcut string to a human-readable form.
/// e.g. "CommandOrControl+Shift+Comma" → "⌘⇧,"
fn shortcut_display_name(shortcut: &str) -> String {
    shortcut
        .split('+')
        .map(|part| match part {
            "CommandOrControl" | "Command" | "Super" => "⌘",
            "Shift" => "⇧",
            "Alt" | "Option" => "⌥",
            "Control" | "Ctrl" => "⌃",
            "Comma" => ",",
            "Period" => ".",
            "Space" => "Space",
            other => other,
        })
        .collect::<Vec<_>>()
        .join("")
}
