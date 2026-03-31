//! Key capture for the shortcut recorder.
//!
//! Uses CGEventTap to intercept ALL key events (including system-level media keys)
//! and emits them as Tauri events.

use tauri::{AppHandle, Emitter};
use std::sync::atomic::{AtomicBool, Ordering};

static CAPTURING: AtomicBool = AtomicBool::new(false);

/// Start the CGEventTap background thread. Called once at app startup.
/// The tap runs continuously but only emits events when CAPTURING is true.
/// Retries every 5 seconds if permission is not yet granted.
pub fn init_key_capture_thread(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let app_clone = app.clone();
        std::thread::spawn(move || {
            // Check if Input Monitoring is available
            unsafe {
                extern "C" {
                    fn IOHIDCheckAccess(request_type: u32) -> u32;
                }
                // kIOHIDRequestTypeListenEvent = 1
                let access = IOHIDCheckAccess(1);
                // 0 = granted, 1 = denied, 2 = unknown/not determined
                log::info!("Input Monitoring access check: {} (0=granted, 1=denied, 2=undetermined)", access);
            }

            let mut attempt = 0u32;
            loop {
                attempt += 1;
                if attempt <= 3 || attempt % 12 == 0 {
                    // Log first 3 attempts, then every ~60s
                    log::info!("CGEventTap: attempt {} to create tap...", attempt);
                }
                if try_create_cgevent_tap(&app_clone) {
                    log::warn!("CGEventTap run loop exited, retrying...");
                    attempt = 0;
                }
                std::thread::sleep(std::time::Duration::from_secs(5));
            }
        });
    }
}

#[tauri::command]
pub async fn start_key_capture() -> Result<(), String> {
    CAPTURING.store(true, Ordering::SeqCst);
    log::info!("Key capture enabled");
    Ok(())
}

#[tauri::command]
pub async fn stop_key_capture() -> Result<(), String> {
    CAPTURING.store(false, Ordering::SeqCst);
    log::info!("Key capture disabled");
    Ok(())
}

// Correct undocumented CGEventField IDs for NX_SYSDEFINED events
// (verified from working open-source implementations)
const CGEVENT_FIELD_SUBTYPE: u32 = 0x53; // 83 — event subtype
const CGEVENT_FIELD_DATA1: u32 = 0x95;   // 149 — event data1

// NX_KEYTYPE constants from IOKit/hidsystem/ev_keymap.h
const NX_KEYTYPE_SOUND_UP: i32 = 0;
const NX_KEYTYPE_SOUND_DOWN: i32 = 1;
const NX_KEYTYPE_BRIGHTNESS_UP: i32 = 2;
const NX_KEYTYPE_BRIGHTNESS_DOWN: i32 = 3;
const NX_KEYTYPE_MUTE: i32 = 7;
const NX_KEYTYPE_PLAY: i32 = 16;
const NX_KEYTYPE_NEXT: i32 = 17;
const NX_KEYTYPE_PREVIOUS: i32 = 18;
const NX_KEYTYPE_FAST: i32 = 19;
const NX_KEYTYPE_REWIND: i32 = 20;
const NX_KEYTYPE_ILLUMINATION_UP: i32 = 21;
const NX_KEYTYPE_ILLUMINATION_DOWN: i32 = 22;
const NX_KEYTYPE_ILLUMINATION_TOGGLE: i32 = 23;

// NX_SUBTYPE_AUX_CONTROL_BUTTONS — required subtype for media key events
const NX_SUBTYPE_AUX_CONTROL: i64 = 8;

/// Try to create and run a CGEventTap. Returns true if the tap was created
/// (blocks in the run loop until the tap dies). Returns false if creation fails.
#[cfg(target_os = "macos")]
fn try_create_cgevent_tap(app: &AppHandle) -> bool {
    unsafe {
        // Event mask: broad capture to catch all possible key event types
        let event_mask: u64 = (1 << 10)  // kCGEventKeyDown
            | (1 << 11)  // kCGEventKeyUp (for diagnostics)
            | (1 << 12)  // kCGEventFlagsChanged (modifier changes, some special keys)
            | (1 << 14); // NX_SYSDEFINED (media keys)

        extern "C" fn callback(
            _proxy: *const std::ffi::c_void,
            event_type_raw: u32,
            event: *const std::ffi::c_void,
            user_info: *mut std::ffi::c_void,
        ) -> *const std::ffi::c_void {
            unsafe {
                if !CAPTURING.load(Ordering::SeqCst) {
                    return event;
                }

                if user_info.is_null() {
                    return event;
                }
                let app = &*(user_info as *const AppHandle);

                extern "C" {
                    fn CGEventGetIntegerValueField(event: *const std::ffi::c_void, field: u32) -> i64;
                    fn CGEventGetFlags(event: *const std::ffi::c_void) -> u64;
                }

                // Log ALL event types for diagnostics
                let keycode_for_log = CGEventGetIntegerValueField(event, 9) as u16;
                log::info!(
                    "[keycap] event_type={}, keycode=0x{:02X} ({})",
                    event_type_raw, keycode_for_log, keycode_for_log
                );

                // Handle NX_SYSDEFINED events (media/system keys)
                if event_type_raw == 14 {
                    let subtype = CGEventGetIntegerValueField(event, CGEVENT_FIELD_SUBTYPE);
                    if subtype != NX_SUBTYPE_AUX_CONTROL {
                        return event;
                    }

                    let data1 = CGEventGetIntegerValueField(event, CGEVENT_FIELD_DATA1);
                    let key_code = ((data1 >> 16) & 0xFFFF) as i32;
                    let key_state = ((data1 & 0xFF00) >> 8) as i32;

                    // Only key down (0x0A), ignore key up (0x0B) and repeat
                    if key_state != 0x0A {
                        return event;
                    }

                    let key_name = match key_code {
                        NX_KEYTYPE_SOUND_UP => "AudioVolumeUp",
                        NX_KEYTYPE_SOUND_DOWN => "AudioVolumeDown",
                        NX_KEYTYPE_MUTE => "AudioVolumeMute",
                        NX_KEYTYPE_PLAY => "MediaPlayPause",
                        NX_KEYTYPE_NEXT => "MediaTrackNext",
                        NX_KEYTYPE_PREVIOUS => "MediaTrackPrevious",
                        NX_KEYTYPE_FAST => "MediaFastForward",
                        NX_KEYTYPE_REWIND => "MediaRewind",
                        NX_KEYTYPE_BRIGHTNESS_UP => "BrightnessUp",
                        NX_KEYTYPE_BRIGHTNESS_DOWN => "BrightnessDown",
                        NX_KEYTYPE_ILLUMINATION_UP => "KeyboardBrightnessUp",
                        NX_KEYTYPE_ILLUMINATION_DOWN => "KeyboardBrightnessDown",
                        NX_KEYTYPE_ILLUMINATION_TOGGLE => "KeyboardBrightnessToggle",
                        _ => {
                            log::info!("Unknown NX media key type: {} (data1: 0x{:X})", key_code, data1);
                            return event;
                        }
                    };

                    log::info!("Captured system key: {} (NX type {})", key_name, key_code);

                    let _ = app.emit("key-captured", serde_json::json!({
                        "code": key_name,
                        "key": key_name,
                        "source": "system",
                    }));

                    return event;
                }

                // Handle regular keyboard events
                // kCGEventKeyDown = 10, kCGEventKeyUp = 11, kCGEventFlagsChanged = 12
                if event_type_raw == 10 || event_type_raw == 11 || event_type_raw == 12 {
                    let keycode = CGEventGetIntegerValueField(event, 9) as u16;
                    let flags = CGEventGetFlags(event);

                    let has_cmd = (flags & (1 << 20)) != 0;
                    let has_shift = (flags & (1 << 17)) != 0;
                    let has_alt = (flags & (1 << 19)) != 0;
                    let has_ctrl = (flags & (1 << 18)) != 0;

                    if event_type_raw == 12 {
                        // kCGEventFlagsChanged — modifier pressed or released.
                        // Always emit so the frontend can track modifier state
                        // and detect when all keys are released.
                        let _ = app.emit("key-modifier", serde_json::json!({
                            "metaKey": has_cmd,
                            "shiftKey": has_shift,
                            "altKey": has_alt,
                            "ctrlKey": has_ctrl,
                        }));
                    } else if event_type_raw == 10 {
                        // kCGEventKeyDown — only emit for non-modifier keys
                        let key_name = vk_to_name(keycode);
                        if key_name.is_empty() {
                            log::info!("Unknown macOS keycode: 0x{:02X}", keycode);
                            return event;
                        }
                        let is_modifier = matches!(key_name, "Meta" | "Shift" | "ShiftRight" |
                            "Alt" | "AltRight" | "Control" | "ControlRight" | "Fn");
                        if !is_modifier {
                            log::info!("Captured key: {} (vk 0x{:02X})", key_name, keycode);
                            let _ = app.emit("key-captured", serde_json::json!({
                                "code": key_name,
                                "key": key_name,
                                "source": "keyboard",
                                "metaKey": has_cmd,
                                "shiftKey": has_shift,
                                "altKey": has_alt,
                                "ctrlKey": has_ctrl,
                            }));
                        }
                    }
                    // Type 11 (kCGEventKeyUp) — no action needed
                }

                event
            }
        }

        let app_ptr = Box::into_raw(Box::new(app.clone()));

        extern "C" {
            fn CGEventTapCreate(
                tap: u32, place: u32, options: u32,
                events_of_interest: u64,
                callback: extern "C" fn(*const std::ffi::c_void, u32, *const std::ffi::c_void, *mut std::ffi::c_void) -> *const std::ffi::c_void,
                user_info: *mut std::ffi::c_void,
            ) -> *const std::ffi::c_void;
            fn CFMachPortCreateRunLoopSource(alloc: *const std::ffi::c_void, port: *const std::ffi::c_void, order: i64) -> *const std::ffi::c_void;
            fn CFRunLoopGetCurrent() -> *const std::ffi::c_void;
            fn CFRunLoopAddSource(rl: *const std::ffi::c_void, source: *const std::ffi::c_void, mode: *const std::ffi::c_void);
            fn CGEventTapEnable(tap: *const std::ffi::c_void, enable: bool);
            static kCFRunLoopCommonModes: *const std::ffi::c_void;
        }

        // Active tap requires Input Monitoring permission on macOS Sequoia
        let tap = CGEventTapCreate(1, 0, 0, event_mask, callback, app_ptr as *mut std::ffi::c_void);

        if tap.is_null() {
            let _ = Box::from_raw(app_ptr);
            return false;
        }

        CGEventTapEnable(tap, true);
        let source = CFMachPortCreateRunLoopSource(std::ptr::null(), tap, 0);
        let run_loop = CFRunLoopGetCurrent();
        CFRunLoopAddSource(run_loop, source, kCFRunLoopCommonModes);

        log::info!("CGEventTap created and enabled, entering run loop");

        extern "C" {
            fn CFRunLoopRun();
        }

        // Block in the run loop. CFRunLoopRun only returns if all sources
        // are removed, which means the tap was invalidated by the system.
        CFRunLoopRun();

        // If we get here, the run loop exited — tap was invalidated
        true
    }
}

/// Map macOS virtual key code to a name.
#[cfg(target_os = "macos")]
fn vk_to_name(keycode: u16) -> &'static str {
    match keycode {
        0x00 => "KeyA", 0x01 => "KeyS", 0x02 => "KeyD", 0x03 => "KeyF",
        0x04 => "KeyH", 0x05 => "KeyG", 0x06 => "KeyZ", 0x07 => "KeyX",
        0x08 => "KeyC", 0x09 => "KeyV", 0x0B => "KeyB", 0x0C => "KeyQ",
        0x0D => "KeyW", 0x0E => "KeyE", 0x0F => "KeyR", 0x10 => "KeyY",
        0x11 => "KeyT", 0x12 => "Digit1", 0x13 => "Digit2", 0x14 => "Digit3",
        0x15 => "Digit4", 0x16 => "Digit6", 0x17 => "Digit5", 0x18 => "Equal",
        0x19 => "Digit9", 0x1A => "Digit7", 0x1B => "Minus", 0x1C => "Digit8",
        0x1D => "Digit0", 0x1E => "BracketRight", 0x1F => "KeyO",
        0x20 => "KeyU", 0x21 => "BracketLeft", 0x22 => "KeyI", 0x23 => "KeyP",
        0x24 => "Enter", 0x25 => "KeyL", 0x26 => "KeyJ", 0x27 => "Quote",
        0x28 => "KeyK", 0x29 => "Semicolon", 0x2A => "Backslash",
        0x2B => "Comma", 0x2C => "Slash", 0x2D => "KeyN", 0x2E => "KeyM",
        0x2F => "Period", 0x30 => "Tab", 0x31 => "Space", 0x32 => "Backquote",
        0x33 => "Backspace", 0x35 => "Escape",
        0x37 => "Meta", 0x38 => "Shift", 0x39 => "CapsLock", 0x3A => "Alt",
        0x3B => "Control", 0x3C => "ShiftRight", 0x3D => "AltRight",
        0x3E => "ControlRight", 0x3F => "Fn",
        0x40 => "F17", 0x41 => "NumpadDecimal", 0x43 => "NumpadMultiply",
        0x45 => "NumpadAdd", 0x47 => "NumLock", 0x4B => "NumpadDivide",
        0x4C => "NumpadEnter", 0x4E => "NumpadSubtract", 0x4F => "F18",
        0x50 => "F19", 0x51 => "NumpadEqual",
        0x52 => "Numpad0", 0x53 => "Numpad1", 0x54 => "Numpad2",
        0x55 => "Numpad3", 0x56 => "Numpad4", 0x57 => "Numpad5",
        0x58 => "Numpad6", 0x59 => "Numpad7", 0x5A => "F20",
        0x5B => "Numpad8", 0x5C => "Numpad9",
        0x60 => "F5", 0x61 => "F6", 0x62 => "F7", 0x63 => "F3",
        0x64 => "F8", 0x65 => "F9", 0x67 => "F11", 0x69 => "F13",
        0x6A => "F16", 0x6B => "F14", 0x6D => "F10", 0x6F => "F12",
        0x71 => "F15", 0x72 => "Insert", 0x73 => "Home", 0x74 => "PageUp",
        0x75 => "Delete", 0x76 => "F4", 0x77 => "End", 0x78 => "F2",
        0x79 => "PageDown", 0x7A => "F1",
        0x7B => "ArrowLeft", 0x7C => "ArrowRight",
        0x7D => "ArrowDown", 0x7E => "ArrowUp",
        _ => "",
    }
}

/// Map a Tauri key name back to a macOS virtual key code (for push-to-talk polling).
#[cfg(target_os = "macos")]
pub fn tauri_key_to_vk(key: &str) -> Option<u16> {
    Some(match key {
        "KeyA" => 0x00, "KeyS" => 0x01, "KeyD" => 0x02, "KeyF" => 0x03,
        "KeyH" => 0x04, "KeyG" => 0x05, "KeyZ" => 0x06, "KeyX" => 0x07,
        "KeyC" => 0x08, "KeyV" => 0x09, "KeyB" => 0x0B, "KeyQ" => 0x0C,
        "KeyW" => 0x0D, "KeyE" => 0x0E, "KeyR" => 0x0F, "KeyY" => 0x10,
        "KeyT" => 0x11, "Space" => 0x31, "Enter" => 0x24, "Tab" => 0x30,
        "Escape" => 0x35, "Backspace" => 0x33, "Delete" => 0x75,
        "ArrowUp" => 0x7E, "ArrowDown" => 0x7D, "ArrowLeft" => 0x7B, "ArrowRight" => 0x7C,
        "F1" => 0x7A, "F2" => 0x78, "F3" => 0x63, "F4" => 0x76,
        "F5" => 0x60, "F6" => 0x61, "F7" => 0x62, "F8" => 0x64,
        "F9" => 0x65, "F10" => 0x6D, "F11" => 0x67, "F12" => 0x6F,
        "F13" => 0x69, "F14" => 0x6B, "F15" => 0x71, "F16" => 0x6A,
        "F17" => 0x40, "F18" => 0x4F, "F19" => 0x50, "F20" => 0x5A,
        "Digit1" => 0x12, "Digit2" => 0x13, "Digit3" => 0x14, "Digit4" => 0x15,
        "Digit5" => 0x17, "Digit6" => 0x16, "Digit7" => 0x1A, "Digit8" => 0x1C,
        "Digit9" => 0x19, "Digit0" => 0x1D,
        "Minus" => 0x1B, "Equal" => 0x18, "BracketLeft" => 0x21, "BracketRight" => 0x1E,
        "Backslash" => 0x2A, "Semicolon" => 0x29, "Quote" => 0x27,
        "Backquote" => 0x32, "Comma" => 0x2B, "Period" => 0x2F, "Slash" => 0x2C,
        "KeyI" => 0x22, "KeyJ" => 0x26, "KeyK" => 0x28, "KeyL" => 0x25,
        "KeyM" => 0x2E, "KeyN" => 0x2D, "KeyO" => 0x1F, "KeyP" => 0x23,
        "KeyU" => 0x20,
        _ => return None,
    })
}
