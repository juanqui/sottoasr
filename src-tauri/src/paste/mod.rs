#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::{
    paste_text,
    paste_text_and_restore,
    copy_to_clipboard,
    is_accessibility_trusted,
    test_accessibility_functional,
    warmup_cgevent_pipeline,
};

#[cfg(not(target_os = "macos"))]
pub fn paste_text(_text: &str) -> Result<(), String> {
    Err("Paste not supported on this platform yet".into())
}

#[cfg(not(target_os = "macos"))]
pub fn paste_text_and_restore(_text: &str) -> Result<(), String> {
    Err("Paste not supported on this platform yet".into())
}

#[cfg(not(target_os = "macos"))]
pub fn copy_to_clipboard(_text: &str) -> Result<(), String> {
    Err("Clipboard not supported on this platform yet".into())
}

#[cfg(not(target_os = "macos"))]
pub fn is_accessibility_trusted() -> bool {
    true
}

#[cfg(not(target_os = "macos"))]
pub fn test_accessibility_functional() -> bool {
    true
}

#[cfg(not(target_os = "macos"))]
pub fn warmup_cgevent_pipeline() {}
