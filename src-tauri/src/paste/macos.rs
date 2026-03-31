use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

/// Save the current clipboard contents, write new text, paste via Cmd+V,
/// and optionally restore the original clipboard after a delay.
/// If `target_pid` is non-zero and differs from the current frontmost app,
/// re-activates the target app before pasting to avoid focus race conditions.
pub fn paste_text(text: &str, target_pid: i32) -> Result<(), String> {
    paste_text_inner(text, false, target_pid)
}

/// Same as paste_text but restores the original clipboard contents afterwards.
pub fn paste_text_and_restore(text: &str, target_pid: i32) -> Result<(), String> {
    paste_text_inner(text, true, target_pid)
}

fn paste_text_inner(text: &str, restore: bool, target_pid: i32) -> Result<(), String> {
    // Check accessibility — AXIsProcessTrusted() is the authoritative check.
    // We no longer run the functional AX query on every paste because it can
    // produce false negatives when called from background threads or during
    // focus transitions (e.g., AXFocusedApplication returns an error when no
    // app has focus). The functional check is done once at startup instead.
    if !is_accessibility_trusted() {
        return Err(
            "Accessibility permission not granted. \
             Go to System Settings > Privacy & Security > Accessibility, \
             remove SottoASR, then re-add the .app bundle and toggle it ON."
            .into()
        );
    }

    // Save current clipboard contents for restore
    let saved_clipboard = if restore {
        let mut clipboard = arboard::Clipboard::new()
            .map_err(|e| format!("Failed to open clipboard: {}", e))?;
        clipboard.get_text().ok()
    } else {
        None
    };

    // Write text to clipboard via arboard (uses NSPasteboard directly)
    {
        let mut clipboard = arboard::Clipboard::new()
            .map_err(|e| format!("Failed to open clipboard: {}", e))?;
        clipboard.set_text(text)
            .map_err(|e| format!("Failed to write to clipboard: {}", e))?;
    }

    // Brief pause for clipboard to settle (NSPasteboard change count propagation)
    std::thread::sleep(std::time::Duration::from_millis(30));

    // Always re-activate the target app before pasting. CGEventPost to HID sends
    // Cmd+V to whatever app is frontmost, so we must ensure the right app has focus.
    // We always activate (even if we think it's already frontmost) because
    // NSWorkspace.frontmostApplication is unreliable from background threads.
    if target_pid > 0 {
        log::info!("Re-activating target app PID {} before paste", target_pid);
        activate_pid(target_pid);
    }

    // Simulate Cmd+V via HID (goes to the frontmost app)
    let paste_result = simulate_cmd_v();
    if paste_result.is_err() {
        log::warn!("First Cmd+V attempt failed, retrying...");
        std::thread::sleep(std::time::Duration::from_millis(50));
        simulate_cmd_v()?;
    }

    // Small delay after posting to ensure event is delivered
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Restore clipboard contents after the paste has been consumed
    if let Some(original) = saved_clipboard {
        // Capture the current change count right after our paste
        let change_count_after_paste = get_pasteboard_change_count();

        std::thread::spawn(move || {
            // Wait long enough for the target app to consume the paste
            std::thread::sleep(std::time::Duration::from_millis(500));
            // Only restore if nobody else has changed the clipboard
            let current_count = get_pasteboard_change_count();
            if current_count == change_count_after_paste {
                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                    let _ = clipboard.set_text(&original);
                    log::info!("Clipboard restored to previous contents");
                }
            } else {
                log::info!(
                    "Clipboard changed by user, skipping restore (count {} \u{2192} {})",
                    change_count_after_paste, current_count
                );
            }
        });
    }

    log::info!("CGEvent Cmd+V posted");
    Ok(())
}

/// Check if the process is trusted for Accessibility.
pub fn is_accessibility_trusted() -> bool {
    unsafe {
        extern "C" { fn AXIsProcessTrusted() -> bool; }
        AXIsProcessTrusted()
    }
}

/// Perform a functional test of Accessibility — verifies that the permission
/// is actually working, not just reported as granted by the TCC database.
/// On macOS Sequoia, AXIsProcessTrusted() can return true while the permission
/// has not yet propagated to the system components that handle CGEvent posting.
pub fn test_accessibility_functional() -> bool {
    unsafe {
        extern "C" {
            fn AXUIElementCreateSystemWide() -> *const std::ffi::c_void;
            fn AXUIElementCopyAttributeValue(
                element: *const std::ffi::c_void,
                attribute: *const std::ffi::c_void,
                value: *mut *const std::ffi::c_void,
            ) -> i32;
            fn CFRelease(cf: *const std::ffi::c_void);
            fn CFStringCreateWithCString(
                alloc: *const std::ffi::c_void,
                cstr: *const std::ffi::c_char,
                encoding: u32,
            ) -> *const std::ffi::c_void;
        }

        let system_wide = AXUIElementCreateSystemWide();
        if system_wide.is_null() {
            return false;
        }

        // kAXFocusedApplicationAttribute = "AXFocusedApplication"
        let attr_cstr = c"AXFocusedApplication".as_ptr();
        let attr = CFStringCreateWithCString(std::ptr::null(), attr_cstr, 0x08000100);
        if attr.is_null() {
            CFRelease(system_wide);
            return false;
        }

        let mut value: *const std::ffi::c_void = std::ptr::null();
        let result = AXUIElementCopyAttributeValue(system_wide, attr, &mut value);

        // Clean up
        if !value.is_null() {
            CFRelease(value);
        }
        CFRelease(attr);
        CFRelease(system_wide);

        // errAXSuccess = 0, errAXAPIDisabled = -25211
        // On success or "attribute not settable" (-25205), accessibility is functional.
        // Only real failure codes (like -25211 API disabled) mean it's not working.
        let ok = result == 0 || result == -25205;
        if !ok {
            log::warn!("AX functional check returned error code: {}", result);
        }
        ok
    }
}

/// Initialize the CGEvent pipeline with a warm-up event.
/// On macOS 15 (Sequoia), the first CGEvent can be silently dropped if the
/// pipeline hasn't been initialized. Call this once at startup after
/// Accessibility permission is confirmed.
pub fn warmup_cgevent_pipeline() {
    match CGEventSource::new(CGEventSourceStateID::HIDSystemState) {
        Ok(source) => {
            // Create a flag-change event (no actual key press) to warm the pipeline
            if let Ok(event) = CGEvent::new_keyboard_event(source, 0xFF, false) {
                // Post to HID to initialize the pipeline, using an invalid key code
                // that won't produce any visible effect
                event.post(CGEventTapLocation::HID);
                log::info!("CGEvent pipeline warmed up");
            }
        }
        Err(_) => {
            log::warn!("Failed to create CGEventSource for warm-up (accessibility not granted?)");
        }
    }
}

/// Activate (bring to front) the application with the given PID.
/// Uses NSRunningApplication.activateWithOptions: and then AppleScript as fallback.
/// Waits for activation to settle before returning.
fn activate_pid(pid: i32) {
    // Try ObjC activation first
    let objc_ok = activate_pid_objc(pid);

    if !objc_ok {
        // Fallback: use AppleScript which is more reliable for cross-app activation
        log::info!("ObjC activation failed, trying AppleScript fallback for PID {}", pid);
        activate_pid_applescript(pid);
    }

    // Always wait for the activation to settle — window server needs time
    // to transfer focus, especially across apps.
    std::thread::sleep(std::time::Duration::from_millis(150));
}

/// Try to activate via NSRunningApplication. Returns true if the call succeeded.
fn activate_pid_objc(pid: i32) -> bool {
    unsafe {
        extern "C" {
            fn objc_getClass(name: *const std::ffi::c_char) -> *const std::ffi::c_void;
            fn sel_registerName(name: *const std::ffi::c_char) -> *const std::ffi::c_void;
            fn objc_msgSend(); // untyped — cast to proper signature per call site
        }

        type MsgSendObjI32 = unsafe extern "C" fn(
            *const std::ffi::c_void, *const std::ffi::c_void, i32,
        ) -> *const std::ffi::c_void;

        let send_obj_i32: MsgSendObjI32 = std::mem::transmute(objc_msgSend as *const std::ffi::c_void);

        let cls = objc_getClass(c"NSRunningApplication".as_ptr());
        if cls.is_null() {
            log::warn!("NSRunningApplication class not found");
            return false;
        }

        let sel_app = sel_registerName(c"runningApplicationWithProcessIdentifier:".as_ptr());
        let running_app = send_obj_i32(cls, sel_app, pid);
        if running_app.is_null() {
            log::warn!("No running application found for PID {}", pid);
            return false;
        }

        // activateWithOptions: NSApplicationActivateAllWindows | NSApplicationActivateIgnoringOtherApps = 3
        type MsgSendActivate = unsafe extern "C" fn(
            *const std::ffi::c_void, *const std::ffi::c_void, u64,
        ) -> bool;
        let send_activate: MsgSendActivate = std::mem::transmute(objc_msgSend as *const std::ffi::c_void);
        let sel_activate = sel_registerName(c"activateWithOptions:".as_ptr());
        let activated = send_activate(running_app, sel_activate, 3);

        log::info!("activateWithOptions: returned {} for PID {}", activated, pid);
        activated
    }
}

/// Fallback activation via osascript. More reliable across apps but slightly slower.
fn activate_pid_applescript(pid: i32) {
    let script = format!(
        "tell application \"System Events\" to set frontmost of (first process whose unix id is {}) to true",
        pid
    );
    match std::process::Command::new("osascript")
        .args(["-e", &script])
        .output()
    {
        Ok(output) => {
            if output.status.success() {
                log::info!("AppleScript activation succeeded for PID {}", pid);
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                log::warn!("AppleScript activation failed for PID {}: {}", pid, stderr.trim());
            }
        }
        Err(e) => {
            log::warn!("Failed to run osascript: {}", e);
        }
    }
}

fn simulate_cmd_v() -> Result<(), String> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| "Failed to create CGEventSource".to_string())?;

    // Key code 0x09 = kVK_ANSI_V
    let key_down = CGEvent::new_keyboard_event(source.clone(), 0x09, true)
        .map_err(|_| "Failed to create key down event".to_string())?;
    let key_up = CGEvent::new_keyboard_event(source, 0x09, false)
        .map_err(|_| "Failed to create key up event".to_string())?;

    // Set Command flag on BOTH events (not as separate key presses)
    key_down.set_flags(CGEventFlags::CGEventFlagCommand);
    key_up.set_flags(CGEventFlags::CGEventFlagCommand);

    key_down.post(CGEventTapLocation::HID);
    // Small gap between key down and key up for reliability
    std::thread::sleep(std::time::Duration::from_millis(10));
    key_up.post(CGEventTapLocation::HID);

    Ok(())
}

/// Get the NSPasteboard change count for the general pasteboard.
/// Returns -1 if the pasteboard cannot be accessed.
fn get_pasteboard_change_count() -> i64 {
    unsafe {
        extern "C" {
            fn objc_getClass(name: *const std::ffi::c_char) -> *const std::ffi::c_void;
            fn sel_registerName(name: *const std::ffi::c_char) -> *const std::ffi::c_void;
            fn objc_msgSend();
        }

        type MsgSendObj = unsafe extern "C" fn(
            *const std::ffi::c_void, *const std::ffi::c_void,
        ) -> *const std::ffi::c_void;
        type MsgSendI64 = unsafe extern "C" fn(
            *const std::ffi::c_void, *const std::ffi::c_void,
        ) -> i64;

        let send_obj: MsgSendObj = std::mem::transmute(objc_msgSend as *const std::ffi::c_void);
        let send_i64: MsgSendI64 = std::mem::transmute(objc_msgSend as *const std::ffi::c_void);

        let cls = objc_getClass(c"NSPasteboard".as_ptr());
        if cls.is_null() { return -1; }

        let sel_general = sel_registerName(c"generalPasteboard".as_ptr());
        let pasteboard = send_obj(cls, sel_general);
        if pasteboard.is_null() { return -1; }

        let sel_count = sel_registerName(c"changeCount".as_ptr());
        send_i64(pasteboard, sel_count)
    }
}

/// Get the PID of the frontmost (active) application.
/// Returns 0 if it cannot be determined.
///
/// Uses the ObjC runtime directly: [NSWorkspace.sharedWorkspace.frontmostApplication processIdentifier]
pub fn get_frontmost_pid() -> i32 {
    unsafe {
        extern "C" {
            fn objc_getClass(name: *const std::ffi::c_char) -> *const std::ffi::c_void;
            fn sel_registerName(name: *const std::ffi::c_char) -> *const std::ffi::c_void;
            fn objc_msgSend(); // untyped — cast to proper signature per call site
        }

        // Cast objc_msgSend to the correct calling convention for each return type.
        // ARM64 requires exact signatures (no variadics) for register-based dispatch.
        type MsgSendObj = unsafe extern "C" fn(*const std::ffi::c_void, *const std::ffi::c_void) -> *const std::ffi::c_void;
        type MsgSendI32 = unsafe extern "C" fn(*const std::ffi::c_void, *const std::ffi::c_void) -> i32;

        let send_obj: MsgSendObj = std::mem::transmute(objc_msgSend as *const std::ffi::c_void);
        let send_i32: MsgSendI32 = std::mem::transmute(objc_msgSend as *const std::ffi::c_void);

        let cls = objc_getClass(c"NSWorkspace".as_ptr());
        if cls.is_null() { return 0; }

        let sel_shared = sel_registerName(c"sharedWorkspace".as_ptr());
        let workspace = send_obj(cls, sel_shared);
        if workspace.is_null() { return 0; }

        let sel_front = sel_registerName(c"frontmostApplication".as_ptr());
        let app = send_obj(workspace, sel_front);
        if app.is_null() { return 0; }

        let sel_pid = sel_registerName(c"processIdentifier".as_ptr());
        send_i32(app, sel_pid)
    }
}

/// Write text to the system clipboard (for copy operations, not paste).
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    let mut clipboard = arboard::Clipboard::new()
        .map_err(|e| format!("Failed to open clipboard: {}", e))?;
    clipboard.set_text(text)
        .map_err(|e| format!("Failed to write to clipboard: {}", e))?;
    Ok(())
}
