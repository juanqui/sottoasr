/// Trait for paste/clipboard operations.
/// Production: CGEvent Cmd+V + arboard clipboard on macOS.
/// Tests: records pasted text for assertion.
pub trait PasteBackend: Send + Sync {
    /// Paste text at the cursor position in the target app.
    fn paste_text(&self, text: &str, target_pid: i32) -> Result<(), String>;

    /// Paste text and restore the original clipboard contents.
    fn paste_text_and_restore(&self, text: &str, target_pid: i32) -> Result<(), String>;

    /// Copy text to the clipboard (without pasting).
    fn copy_to_clipboard(&self, text: &str) -> Result<(), String>;

    /// Get the PID of the frontmost application. Returns 0 if unknown.
    fn get_frontmost_pid(&self) -> i32;

    /// Check if accessibility permission is granted.
    fn is_accessibility_trusted(&self) -> bool;
}

/// Production paste backend using macOS CGEvent + arboard.
#[cfg(target_os = "macos")]
pub struct MacOsPasteBackend;

#[cfg(target_os = "macos")]
impl PasteBackend for MacOsPasteBackend {
    fn paste_text(&self, text: &str, target_pid: i32) -> Result<(), String> {
        super::macos::paste_text(text, target_pid)
    }

    fn paste_text_and_restore(&self, text: &str, target_pid: i32) -> Result<(), String> {
        super::macos::paste_text_and_restore(text, target_pid)
    }

    fn copy_to_clipboard(&self, text: &str) -> Result<(), String> {
        super::macos::copy_to_clipboard(text)
    }

    fn get_frontmost_pid(&self) -> i32 {
        super::macos::get_frontmost_pid()
    }

    fn is_accessibility_trusted(&self) -> bool {
        super::macos::is_accessibility_trusted()
    }
}

/// Stub paste backend for non-macOS platforms. All operations return errors.
#[cfg(not(target_os = "macos"))]
pub struct StubPasteBackend;

#[cfg(not(target_os = "macos"))]
impl PasteBackend for StubPasteBackend {
    fn paste_text(&self, _text: &str, _target_pid: i32) -> Result<(), String> {
        Err("Paste not supported on this platform".into())
    }

    fn paste_text_and_restore(&self, _text: &str, _target_pid: i32) -> Result<(), String> {
        Err("Paste not supported on this platform".into())
    }

    fn copy_to_clipboard(&self, _text: &str) -> Result<(), String> {
        Err("Clipboard not supported on this platform".into())
    }

    fn get_frontmost_pid(&self) -> i32 {
        0
    }

    fn is_accessibility_trusted(&self) -> bool {
        true
    }
}
