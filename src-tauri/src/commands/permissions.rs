#[tauri::command]
pub async fn check_microphone_permission() -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        Ok(true) // cpal triggers the TCC prompt on first recording
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
            let trusted = unsafe {
                extern "C" {
                    fn AXIsProcessTrusted() -> bool;
                }
                AXIsProcessTrusted()
            };
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
            unsafe {
                // AXIsProcessTrustedWithOptions with kAXTrustedCheckOptionPrompt = true
                // This makes macOS show the "Sotto wants to control this computer" prompt
                // AND pre-populates Sotto in the Accessibility list in System Settings.
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

                // kAXTrustedCheckOptionPrompt key
                let key_cstr = b"AXTrustedCheckOptionPrompt\0".as_ptr() as *const std::ffi::c_char;
                let key = CFStringCreateWithCString(
                    std::ptr::null(),
                    key_cstr,
                    0x08000100, // kCFStringEncodingUTF8
                );

                let keys = [key];
                let values = [kCFBooleanTrue];

                let options = CFDictionaryCreate(
                    std::ptr::null(),
                    keys.as_ptr(),
                    values.as_ptr(),
                    1,
                    &kCFTypeDictionaryKeyCallBacks as *const _ as *const std::ffi::c_void,
                    &kCFTypeDictionaryValueCallBacks as *const _ as *const std::ffi::c_void,
                );

                // This call both checks AND prompts — Sotto will appear in System Settings
                let _trusted = AXIsProcessTrustedWithOptions(options);

                CFRelease(options);
                CFRelease(key);
            }
            Ok::<(), String>(())
        })
        .await
        .map_err(|e| format!("Task join error: {}", e))??;
    }
    Ok(())
}
