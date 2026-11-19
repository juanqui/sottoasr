//! Optional system sounds for dictation events.

/// A dictation event that can play a sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cue {
    Start,
    Stop,
    Cancel,
    Busy,
    Error,
}

impl Cue {
    /// One built-in macOS sound per event (`/System/Library/Sounds`).
    #[cfg(target_os = "macos")]
    fn sound_name(self) -> &'static str {
        match self {
            Cue::Start => "Tink",
            Cue::Stop => "Pop",
            Cue::Cancel => "Bottle",
            Cue::Busy => "Funk",
            Cue::Error => "Basso",
        }
    }
}

/// Play `cue` when sound cues are on. Playback is asynchronous: the caller
/// never waits for the sound.
pub fn play(app: &tauri::AppHandle, cue: Cue) {
    use tauri::Manager;
    let Some(state) = app.try_state::<crate::state::AppState>() else { return };
    if !state.sound_cues.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    #[cfg(target_os = "macos")]
    {
        let name = cue.sound_name();
        let _ = app.run_on_main_thread(move || {
            use tauri_nspanel::{objc2_app_kit::NSSound, objc2_foundation::NSString};
            if let Some(sound) = NSSound::soundNamed(&NSString::from_str(name)) {
                // Named sounds are shared: restart one that is still playing.
                let _ = sound.stop();
                let _ = sound.play();
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = cue;
    }
}
