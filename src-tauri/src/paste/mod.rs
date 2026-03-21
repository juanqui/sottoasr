#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::paste_text;

#[cfg(not(target_os = "macos"))]
pub fn paste_text(_text: &str) -> Result<(), String> {
    Err("Paste not supported on this platform yet".into())
}
