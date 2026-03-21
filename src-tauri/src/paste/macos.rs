#[cfg(target_os = "macos")]
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
#[cfg(target_os = "macos")]
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

#[cfg(target_os = "macos")]
pub fn paste_text(text: &str) -> Result<(), String> {
    // Check accessibility first and log clearly
    let trusted = unsafe {
        extern "C" { fn AXIsProcessTrusted() -> bool; }
        AXIsProcessTrusted()
    };
    if !trusted {
        return Err(
            "Accessibility permission not granted. \
             Go to System Settings > Privacy & Security > Accessibility, \
             remove Sotto, then re-add the .app bundle and toggle it ON."
            .into()
        );
    }

    // Write text to clipboard using pbcopy
    let status = std::process::Command::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(stdin) = child.stdin.as_mut() {
                stdin.write_all(text.as_bytes())?;
            }
            child.wait()
        })
        .map_err(|e| format!("Failed to copy to clipboard: {}", e))?;

    if !status.success() {
        return Err("pbcopy failed".into());
    }

    // Wait for clipboard to settle — race condition otherwise
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Simulate Cmd+V
    simulate_cmd_v()?;

    // Small delay after posting to ensure event is delivered before we return
    // (compiled binaries can return before event is processed)
    std::thread::sleep(std::time::Duration::from_millis(50));

    Ok(())
}

#[cfg(target_os = "macos")]
fn simulate_cmd_v() -> Result<(), String> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| "Failed to create CGEventSource".to_string())?;

    // Key code 0x09 = kVK_ANSI_V
    let key_down = CGEvent::new_keyboard_event(source.clone(), 0x09, true)
        .map_err(|_| "Failed to create key down event".to_string())?;
    let key_up = CGEvent::new_keyboard_event(source, 0x09, false)
        .map_err(|_| "Failed to create key up event".to_string())?;

    // Set Command flag on BOTH events (not as separate key presses)
    // This avoids modifier interference from mouse movement
    key_down.set_flags(CGEventFlags::CGEventFlagCommand);
    key_up.set_flags(CGEventFlags::CGEventFlagCommand);

    // Note: macOS 15 may require valid timestamps on CGEvents.
    // The core-graphics crate doesn't expose set_timestamp, but
    // newly created events get timestamps automatically from the system.

    key_down.post(CGEventTapLocation::HID);
    // Small gap between key down and key up
    std::thread::sleep(std::time::Duration::from_millis(10));
    key_up.post(CGEventTapLocation::HID);

    log::info!("CGEvent Cmd+V posted");
    Ok(())
}
