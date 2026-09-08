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

pub enum ClipboardAction {
    Copy,
    Paste { target_pid: i32, restore: bool },
}

/// AppKit activation and clipboard delivery can wait on other applications.
/// Keep those waits off the executor serving recording, settings and events.
pub async fn write_text(
    backend: &std::sync::Arc<dyn PasteBackend>,
    text: &str,
    action: ClipboardAction,
) -> Result<(), String> {
    let backend = backend.clone();
    let text = text.to_owned();
    tokio::task::spawn_blocking(move || match action {
        ClipboardAction::Copy => backend.copy_to_clipboard(&text),
        ClipboardAction::Paste { target_pid, restore: true } => backend.paste_text_and_restore(&text, target_pid),
        ClipboardAction::Paste { target_pid, restore: false } => backend.paste_text(&text, target_pid),
    }).await.map_err(|error| format!("Clipboard worker failed: {error}"))?
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    struct WaitingPaste {
        entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
    }

    impl PasteBackend for WaitingPaste {
        fn paste_text(&self, text: &str, target_pid: i32) -> Result<(), String> {
            assert_eq!(text, "Preserve the complete final sentence.");
            assert_eq!(target_pid, 42);
            self.entered.lock().unwrap().take().unwrap().send(()).unwrap();
            self.release.lock().unwrap().recv_timeout(Duration::from_secs(1))
                .map_err(|_| "Paste blocked the async executor".to_string())
        }
        fn paste_text_and_restore(&self, text: &str, target_pid: i32) -> Result<(), String> {
            self.paste_text(text, target_pid)
        }
        fn copy_to_clipboard(&self, _text: &str) -> Result<(), String> { Ok(()) }
        fn get_frontmost_pid(&self) -> i32 { 42 }
        fn is_accessibility_trusted(&self) -> bool { true }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn waiting_for_another_app_does_not_block_recording_and_settings_tasks() {
        let (entered, observed) = tokio::sync::oneshot::channel();
        let (release, waiting) = std::sync::mpsc::channel();
        let backend: Arc<dyn PasteBackend> = Arc::new(WaitingPaste {
            entered: Mutex::new(Some(entered)), release: Mutex::new(waiting),
        });
        let work = tokio::spawn(async move {
            write_text(&backend, "Preserve the complete final sentence.",
                ClipboardAction::Paste { target_pid: 42, restore: true }).await
        });
        observed.await.unwrap();
        // The single executor thread must remain available while paste waits.
        assert!(!work.is_finished());
        release.send(()).unwrap();
        work.await.unwrap().unwrap();
    }
}
