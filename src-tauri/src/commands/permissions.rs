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

/// AVFoundation's microphone authorization for this app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) enum MicAuthorization {
    Authorized,
    Denied,
    Restricted,
    NotDetermined,
}

#[cfg(target_os = "macos")]
impl MicAuthorization {
    /// The `PermissionStatus.microphone` value the frontend reads.
    fn as_str(self) -> &'static str {
        match self {
            Self::Authorized => "authorized",
            Self::Denied => "denied",
            Self::Restricted => "restricted",
            Self::NotDetermined => "not_determined",
        }
    }
}

/// Read without prompting; safe to call from any thread.
pub(crate) fn microphone_authorization() -> MicAuthorization {
    #[cfg(target_os = "macos")]
    {
        check_mic_status()
    }
    #[cfg(not(target_os = "macos"))]
    {
        MicAuthorization::Authorized
    }
}

pub async fn check_microphone_permission() -> Result<bool, String> {
    tokio::task::spawn_blocking(|| Ok(microphone_authorization() == MicAuthorization::Authorized))
        .await
        .map_err(|e| format!("Task join error: {}", e))?
}

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
        tokio::task::spawn_blocking(request_mic_access)
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
            let microphone = check_mic_status().as_str().to_string();
            log::info!("Permission check: microphone={}", microphone);

            let accessibility_api = crate::paste::is_accessibility_trusted();
            let accessibility_functional = if accessibility_api {
                crate::paste::test_accessibility_functional()
            } else {
                false
            };
            let needs_restart = accessibility_api && !accessibility_functional;
            log::info!(
                "Permission check: accessibility_api={}, functional={}",
                accessibility_api,
                accessibility_functional
            );

            // Check Input Monitoring via IOHIDCheckAccess
            let input_monitoring = unsafe {
                extern "C" {
                    fn IOHIDCheckAccess(request_type: u32) -> u32;
                }
                match IOHIDCheckAccess(1) {
                    // kIOHIDRequestTypeListenEvent = 1
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
        std::process::Command::new("/usr/bin/open")
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
            reset_accessibility_with(
                || {
                    crate::process::bounded_command(
                        std::process::Command::new("/usr/bin/tccutil").args([
                            "reset",
                            "Accessibility",
                            "com.sottoasr.app",
                        ]),
                        std::time::Duration::from_secs(3),
                    )
                },
                || {
                    // Let TCC settle before requesting the current signature.
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    log::info!("Re-requesting Accessibility permission...");
                    prompt_accessibility();
                },
            )
        })
        .await
        .map_err(|e| format!("Task join error: {}", e))??;
    }
    Ok(())
}

#[cfg(any(target_os = "macos", all(test, unix)))]
fn reset_accessibility_with(
    reset: impl FnOnce() -> Result<std::process::Output, String>,
    prompt: impl FnOnce(),
) -> Result<(), String> {
    let output =
        reset().map_err(|error| format!("Could not reset Accessibility permission: {error}"))?;
    if !output.status.success() {
        return Err(format!("Could not reset Accessibility permission ({}). Open System Settings to remove and re-add SottoASR.", output.status));
    }
    prompt();
    Ok(())
}

#[tauri::command]
pub async fn open_input_monitoring_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/open")
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
        std::process::Command::new("/usr/bin/open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
            .spawn()
            .map_err(|e| format!("Failed to open System Settings: {}", e))?;
    }
    Ok(())
}

/// Hosts the app links to. Anything else could be a page a compromised
/// webview wants the user to trust. The two AWS hosts are for the data-use
/// and pricing pages of the optional cloud speech engine.
const OPEN_URL_HOSTS: [&str; 5] = ["huggingface.co", "github.com", "sottoasr.app", "docs.aws.amazon.com", "aws.amazon.com"];

fn checked_url(url: &str) -> Result<tauri::Url, String> {
    let parsed = tauri::Url::parse(url).map_err(|_| "This link is not a valid web address".to_string())?;
    let allowed = parsed.scheme() == "https"
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.host_str().is_some_and(|host| OPEN_URL_HOSTS.contains(&host));
    if !allowed {
        return Err(format!("SottoASR opens only HTTPS links to {}", OPEN_URL_HOSTS.join(", ")));
    }
    Ok(parsed)
}

/// Open an allowed HTTPS URL in the default browser.
#[tauri::command]
pub async fn open_url(url: String) -> Result<(), String> {
    let url = checked_url(&url)?;
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/open")
            .arg(url.as_str())
            .spawn()
            .map_err(|e| format!("Failed to open URL: {}", e))?;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = url;
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
            fn AXIsProcessTrustedWithOptions(options: *const std::ffi::c_void) -> bool;
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
///
/// On ARM64, objc_msgSend must be called via a typed function pointer cast,
/// NOT as a variadic function. ARM64's calling convention passes variadic args
/// on the stack, but objc_msgSend expects them in registers.
#[cfg(target_os = "macos")]
fn check_mic_status() -> MicAuthorization {
    unsafe {
        extern "C" {
            fn objc_getClass(name: *const std::ffi::c_char) -> *const std::ffi::c_void;
            fn sel_registerName(name: *const std::ffi::c_char) -> *const std::ffi::c_void;
            fn objc_msgSend(); // untyped — will be cast to proper signature
        }

        let class = objc_getClass(c"AVCaptureDevice".as_ptr());
        if class.is_null() {
            log::warn!("AVCaptureDevice class not found — assuming not_determined");
            return MicAuthorization::NotDetermined;
        }

        #[link(name = "AVFoundation", kind = "framework")]
        extern "C" {
            static AVMediaTypeAudio: *const std::ffi::c_void;
        }

        let media_type = AVMediaTypeAudio;
        if media_type.is_null() {
            log::warn!("AVMediaTypeAudio is null");
            return MicAuthorization::NotDetermined;
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
            0 => MicAuthorization::NotDetermined,
            1 => MicAuthorization::Restricted,
            2 => MicAuthorization::Denied,
            3 => MicAuthorization::Authorized,
            _ => {
                log::warn!("Unknown AVAuthorizationStatus: {}", status);
                MicAuthorization::NotDetermined
            }
        }
    }
}

/// Request microphone access. Triggers the native TCC prompt by attempting
/// to open an audio input device via cpal. Returns true if access was granted.
#[cfg(target_os = "macos")]
fn request_mic_access() -> Result<bool, String> {
    // First check current status
    match check_mic_status() {
        MicAuthorization::Authorized => return Ok(true),
        MicAuthorization::Denied | MicAuthorization::Restricted => return Ok(false),
        MicAuthorization::NotDetermined => {}
    }

    // Status is "not_determined" — trigger the native prompt by attempting
    // to access the default input device. cpal's device enumeration triggers
    // the macOS TCC microphone prompt automatically.
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    if let Some(device) = host.default_input_device() {
        // Attempting to get the config triggers the TCC prompt
        let _ = device.default_input_config();
    }

    // Give the system a moment to process the prompt
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Re-check status
    Ok(check_mic_status() == MicAuthorization::Authorized)
}

#[cfg(all(test, unix))]
mod tests {
    use super::{checked_url, reset_accessibility_with};
    use std::cell::Cell;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{ExitStatus, Output};

    fn output(code: i32) -> Output {
        Output {
            status: ExitStatus::from_raw(code << 8),
            stdout: Vec::new(),
            stderr: Vec::new(),
        }
    }

    #[test]
    fn reset_failure_never_prompts_for_accessibility() {
        let prompts = Cell::new(0);
        let timed_out = reset_accessibility_with(
            || Err("Command timed out".into()),
            || prompts.set(prompts.get() + 1),
        );
        assert!(timed_out.unwrap_err().contains("timed out"));
        let rejected =
            reset_accessibility_with(|| Ok(output(1)), || prompts.set(prompts.get() + 1));
        assert!(rejected.unwrap_err().contains("System Settings"));
        assert_eq!(prompts.get(), 0);
    }

    #[test]
    fn successful_reset_prompts_once() {
        let prompts = Cell::new(0);
        reset_accessibility_with(|| Ok(output(0)), || prompts.set(prompts.get() + 1)).unwrap();
        assert_eq!(prompts.get(), 1);
    }

    #[test]
    fn open_url_accepts_only_https_links_to_known_hosts() {
        for allowed in [
            "https://huggingface.co/mlx-community/model",
            "https://github.com/juanqui/sottoasr/releases",
            "https://SottoASR.app/help",
            "https://docs.aws.amazon.com/transcribe/latest/dg/opt-out.html",
            "https://aws.amazon.com/transcribe/pricing/",
        ] {
            assert!(checked_url(allowed).is_ok(), "{allowed}");
        }
        for rejected in [
            "http://github.com/juanqui/sottoasr",
            "https://example.com/",
            "https://github.com.example.com/",
            "https://evilgithub.com/",
            "https://api.github.com/",
            "https://console.aws.amazon.com/",
            "https://amazon.com/",
            "https://user@github.com/",
            "https://user:secret@huggingface.co/",
            "https://github.com@example.com/",
            "file:///etc/passwd",
            "not a url",
        ] {
            assert!(checked_url(rejected).is_err(), "{rejected}");
        }
    }
}
