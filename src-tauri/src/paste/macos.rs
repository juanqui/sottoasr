use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

/// Save the current clipboard contents, write new text, paste via Cmd+V,
/// and optionally restore the original clipboard after a delay.
pub fn paste_text(text: &str) -> Result<(), String> {
    paste_text_inner(text, false)
}

/// Same as paste_text but restores the original clipboard contents afterwards.
pub fn paste_text_and_restore(text: &str) -> Result<(), String> {
    paste_text_inner(text, true)
}

fn paste_text_inner(text: &str, restore: bool) -> Result<(), String> {
    // Check accessibility first
    if !is_accessibility_trusted() {
        return Err(
            "Accessibility permission not granted. \
             Go to System Settings > Privacy & Security > Accessibility, \
             remove Sotto, then re-add the .app bundle and toggle it ON."
            .into()
        );
    }

    // Functional verification: check if accessibility is actually working,
    // not just reported as trusted by the TCC database.
    if !test_accessibility_functional() {
        return Err(
            "Accessibility permission is granted but not yet active. \
             Please restart Sotto for the permission to take effect."
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

    // Simulate Cmd+V with retry on failure
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
        std::thread::spawn(move || {
            // Wait long enough for the target app to consume the paste
            std::thread::sleep(std::time::Duration::from_millis(500));
            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                let _ = clipboard.set_text(&original);
                log::info!("Clipboard restored to previous contents");
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
        result == 0 || result == -25205
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

/// Write text to the system clipboard (for copy operations, not paste).
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    let mut clipboard = arboard::Clipboard::new()
        .map_err(|e| format!("Failed to open clipboard: {}", e))?;
    clipboard.set_text(text)
        .map_err(|e| format!("Failed to write to clipboard: {}", e))?;
    Ok(())
}
