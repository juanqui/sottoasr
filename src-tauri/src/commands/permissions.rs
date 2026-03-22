use serde::Serialize;

// ---- Permission status types ----

#[derive(Debug, Clone, Serialize)]
pub struct PermissionStatus {
    /// "authorized", "denied", "not_determined", or "restricted"
    pub microphone: String,
    /// AXIsProcessTrusted() result
    pub accessibility_api: bool,
    /// Functional test (AXUIElement query) result
    pub accessibility_functional: bool,
    /// True if API says trusted but functional test fails (needs app restart)
    pub needs_restart: bool,
    /// "granted", "denied", or "undetermined"
    pub input_monitoring: String,
}

// ---- Individual permission checks ----

#[tauri::command]
pub async fn check_microphone_permission() -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        tokio::task::spawn_blocking(|| {
            Ok(check_mic_status() == "authorized")
        })
        .await
        .map_err(|e| format!("Task join error: {}", e))?
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(true)
    }
}

#[tauri::command]
pub async fn check_accessibility_permission() -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        tokio::task::spawn_blocking(|| {
            let trusted = crate::paste::is_accessibility_trusted();
            Ok(trusted)
        })
        .await
        .map_err(|e| format!("Task join error: {}", e))?
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(true)
    }
}

#[tauri::command]
pub async fn request_accessibility_permission() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        tokio::task::spawn_blocking(|| {
            prompt_accessibility();
            Ok::<(), String>(())
        })
        .await
        .map_err(|e| format!("Task join error: {}", e))??;
    }
    Ok(())
}

#[tauri::command]
pub async fn request_microphone_permission() -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        tokio::task::spawn_blocking(|| {
            request_mic_access()
        })
        .await
        .map_err(|e| format!("Task join error: {}", e))?
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(true)
    }
}

// ---- Structured check ----

#[tauri::command]
pub async fn check_all_permissions() -> Result<PermissionStatus, String> {
    #[cfg(target_os = "macos")]
    {
        tokio::task::spawn_blocking(|| {
            let microphone = check_mic_status();
            log::info!("Permission check: microphone={}", microphone);

            let accessibility_api = crate::paste::is_accessibility_trusted();
            let accessibility_functional = if accessibility_api {
                crate::paste::test_accessibility_functional()
            } else {
                false
            };
            let needs_restart = accessibility_api && !accessibility_functional;
            log::info!("Permission check: accessibility_api={}, functional={}", accessibility_api, accessibility_functional);

            // Check Input Monitoring via IOHIDCheckAccess
            let input_monitoring = unsafe {
                extern "C" {
                    fn IOHIDCheckAccess(request_type: u32) -> u32;
                }
                match IOHIDCheckAccess(1) { // kIOHIDRequestTypeListenEvent = 1
                    0 => "granted".to_string(),
                    1 => "denied".to_string(),
                    _ => "undetermined".to_string(),
                }
            };
            log::info!("Permission check: input_monitoring={}", input_monitoring);

            Ok(PermissionStatus {
                microphone,
                accessibility_api,
                accessibility_functional,
                needs_restart,
                input_monitoring,
            })
        })
        .await
        .map_err(|e| format!("Task join error: {}", e))?
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(PermissionStatus {
            microphone: "authorized".into(),
            accessibility_api: true,
            accessibility_functional: true,
            needs_restart: false,
            input_monitoring: "granted".into(),
        })
    }
}

// ---- Open System Settings deep links ----

#[tauri::command]
pub async fn open_accessibility_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
            .spawn()
            .map_err(|e| format!("Failed to open System Settings: {}", e))?;
    }
    Ok(())
}

/// Reset and re-request Accessibility permission.
/// Needed after each rebuild with ad-hoc signing because the code signature changes.
/// Runs `tccutil reset Accessibility com.sottoasr.app` then re-triggers the system prompt.
#[tauri::command]
pub async fn fix_accessibility_permission() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        tokio::task::spawn_blocking(|| {
            // Reset the stale TCC entry
            log::info!("Resetting Accessibility TCC entry for com.sottoasr.app...");
            let _ = std::process::Command::new("tccutil")
                .args(["reset", "Accessibility", "com.sottoasr.app"])
                .output();

            // Small delay for TCC database to update
            std::thread::sleep(std::time::Duration::from_millis(500));

            // Re-trigger the system prompt — this adds the current binary with the correct csreq
            log::info!("Re-requesting Accessibility permission...");
            prompt_accessibility();

            Ok::<(), String>(())
        })
        .await
        .map_err(|e| format!("Task join error: {}", e))??;
    }
    Ok(())
}

#[tauri::command]
pub async fn open_input_monitoring_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent")
            .spawn()
            .map_err(|e| format!("Failed to open System Settings: {}", e))?;
    }
    Ok(())
}

#[tauri::command]
pub async fn open_microphone_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
            .spawn()
            .map_err(|e| format!("Failed to open System Settings: {}", e))?;
    }
    Ok(())
}

// ---- macOS implementation helpers ----

/// Trigger the macOS Accessibility permission prompt.
/// Shows a system dialog and pre-populates the app in System Settings > Accessibility.
#[cfg(target_os = "macos")]
pub fn prompt_accessibility() {
    unsafe {
        extern "C" {
            fn CFStringCreateWithCString(
                alloc: *const std::ffi::c_void,
                cstr: *const std::ffi::c_char,
                encoding: u32,
            ) -> *const std::ffi::c_void;
            fn CFDictionaryCreate(
                alloc: *const std::ffi::c_void,
                keys: *const *const std::ffi::c_void,
                values: *const *const std::ffi::c_void,
                count: isize,
                key_callbacks: *const std::ffi::c_void,
                value_callbacks: *const std::ffi::c_void,
            ) -> *const std::ffi::c_void;
            fn AXIsProcessTrustedWithOptions(
                options: *const std::ffi::c_void,
            ) -> bool;
            fn CFRelease(cf: *const std::ffi::c_void);

            static kCFBooleanTrue: *const std::ffi::c_void;
            static kCFTypeDictionaryKeyCallBacks: std::ffi::c_void;
            static kCFTypeDictionaryValueCallBacks: std::ffi::c_void;
        }

        let key_cstr = c"AXTrustedCheckOptionPrompt".as_ptr();
        let key = CFStringCreateWithCString(std::ptr::null(), key_cstr, 0x08000100);
        let keys = [key];
        let values = [kCFBooleanTrue];
        let options = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            &kCFTypeDictionaryKeyCallBacks as *const _,
            &kCFTypeDictionaryValueCallBacks as *const _,
        );

        let _ = AXIsProcessTrustedWithOptions(options);
        CFRelease(options);
        CFRelease(key);
    }
}

/// Check microphone permission status via AVFoundation.
/// Returns: "authorized", "denied", "not_determined", or "restricted"
///
/// On ARM64, objc_msgSend must be called via a typed function pointer cast,
/// NOT as a variadic function. ARM64's calling convention passes variadic args
/// on the stack, but objc_msgSend expects them in registers.
#[cfg(target_os = "macos")]
fn check_mic_status() -> String {
    unsafe {
        extern "C" {
            fn objc_getClass(name: *const std::ffi::c_char) -> *const std::ffi::c_void;
            fn sel_registerName(name: *const std::ffi::c_char) -> *const std::ffi::c_void;
            fn objc_msgSend(); // untyped — will be cast to proper signature
        }

        let class = objc_getClass(c"AVCaptureDevice".as_ptr());
        if class.is_null() {
            log::warn!("AVCaptureDevice class not found — assuming not_determined");
            return "not_determined".into();
        }

        #[link(name = "AVFoundation", kind = "framework")]
        extern "C" {
            static AVMediaTypeAudio: *const std::ffi::c_void;
        }

        let media_type = AVMediaTypeAudio;
        if media_type.is_null() {
            log::warn!("AVMediaTypeAudio is null");
            return "not_determined".into();
        }

        let sel = sel_registerName(c"authorizationStatusForMediaType:".as_ptr());

        // Cast objc_msgSend to the exact signature:
        // +(AVAuthorizationStatus)authorizationStatusForMediaType:(AVMediaType)
        // = (Class, SEL, NSString*) -> NSInteger
        type AuthStatusFn = unsafe extern "C" fn(
            *const std::ffi::c_void,
            *const std::ffi::c_void,
            *const std::ffi::c_void,
        ) -> isize;

        let msg_send: AuthStatusFn = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let status = msg_send(class, sel, media_type);

        match status {
            0 => "not_determined".into(),
            1 => "restricted".into(),
            2 => "denied".into(),
            3 => "authorized".into(),
            _ => {
                log::warn!("Unknown AVAuthorizationStatus: {}", status);
                "not_determined".into()
            }
        }
    }
}

/// Request microphone access. Triggers the native TCC prompt by attempting
/// to open an audio input device via cpal. Returns true if access was granted.
#[cfg(target_os = "macos")]
fn request_mic_access() -> Result<bool, String> {
    // First check current status
    let current = check_mic_status();
    if current == "authorized" {
        return Ok(true);
    }
    if current == "denied" || current == "restricted" {
        return Ok(false);
    }

    // Status is "not_determined" — trigger the native prompt by attempting
    // to access the default input device. cpal's device enumeration triggers
    // the macOS TCC microphone prompt automatically.
    use cpal::traits::{HostTrait, DeviceTrait};
    let host = cpal::default_host();
    if let Some(device) = host.default_input_device() {
        // Attempting to get the config triggers the TCC prompt
        let _ = device.default_input_config();
    }

    // Give the system a moment to process the prompt
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Re-check status
    Ok(check_mic_status() == "authorized")
}
