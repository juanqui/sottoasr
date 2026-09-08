//! Overlay panel commands invoked from the overlay webview.
//!
//! The overlay is an `NSPanel` converted via `tauri_nspanel::to_panel`
//! with `can_become_key_window: false`. wry's built-in
//! `-webkit-app-region: drag` heuristic hooks the original NSWindow's
//! event chain, which is lost after the to_panel conversion, so CSS
//! dragging does not work on this panel.
//!
//! Instead, the frontend calls `overlay_start_drag` on mousedown and we
//! dispatch `performWindowDragWithEvent:` to the NSPanel directly,
//! using the current NSEvent held by the shared NSApplication. This
//! bypasses wry's drag heuristic entirely and works for non-key panels.

use tauri::{AppHandle, Manager};
#[cfg(target_os = "macos")]
use tauri_nspanel::ManagerExt;

/// One authoritative view of the current recording, retained before the webview
/// starts. Revisions make an initial IPC snapshot safe alongside live events.
#[derive(Clone, serde::Serialize)]
pub struct OverlaySnapshot {
    pub revision: u64,
    pub generation: u64,
    pub state: crate::models::AppStateEnum,
    pub started_at_ms: Option<u64>,
    pub error: Option<OverlayFailure>,
}

#[derive(Clone, serde::Serialize)]
pub struct OverlayFailure {
    pub event: String,
    pub payload: serde_json::Value,
}

impl Default for OverlaySnapshot {
    fn default() -> Self {
        Self { revision: 0, generation: 0, state: crate::models::AppStateEnum::Idle,
            started_at_ms: None, error: None }
    }
}

impl OverlaySnapshot {
    fn clear(&mut self, current: &crate::models::AppStateEnum, revision: Option<u64>) -> bool {
        match revision {
            Some(revision) if revision != self.revision || *current != crate::models::AppStateEnum::Idle => return false,
            // A queued automatic hide may predate an error in this generation.
            // Only deliberate dismissal or the next recording clears recovery.
            None if self.error.is_some() => return false,
            _ => {}
        }
        self.error = None;
        self.state = crate::models::AppStateEnum::Idle;
        self.started_at_ms = None;
        self.revision += 1;
        true
    }

    fn transition(&mut self, generation: u64, state: crate::models::AppStateEnum) {
        if self.generation != generation || state == crate::models::AppStateEnum::Recording {
            self.error = None;
        }
        if state == crate::models::AppStateEnum::Recording
            && (self.generation != generation || self.state != state)
        {
            self.started_at_ms = Some(chrono::Utc::now().timestamp_millis().max(0) as u64);
        }
        self.generation = generation;
        self.state = state;
        self.revision += 1;
    }
}

fn emit_snapshot(app: &AppHandle, snapshot: &OverlaySnapshot) {
    use tauri::Emitter;
    let _ = app.emit("overlay-state", snapshot);
}

/// Keep the existing application event while giving the overlay a replayable,
/// revisioned snapshot. Ignore an old phase if another handler already advanced it.
pub fn publish_state(app: &AppHandle, phase: crate::models::AppStateEnum) {
    use tauri::Emitter;
    use std::sync::atomic::Ordering;
    let state = app.state::<crate::state::AppState>();
    let current = state.current_state.lock().unwrap_or_else(|error| error.into_inner());
    if *current != phase { return; }
    let snapshot = {
        let mut snapshot = state.overlay_snapshot.lock().unwrap_or_else(|error| error.into_inner());
        snapshot.transition(state.recording_generation.load(Ordering::SeqCst), phase.clone());
        snapshot.clone()
    };
    drop(current);
    emit_snapshot(app, &snapshot);
    let _ = app.emit("state-changed", &phase);
}

pub fn publish_error(app: &AppHandle, event: &str, generation: u64, mut payload: serde_json::Value) -> bool {
    use tauri::Emitter;
    use std::sync::atomic::Ordering;
    let state = app.state::<crate::state::AppState>();
    let current = state.current_state.lock().unwrap_or_else(|error| error.into_inner());
    if state.recording_generation.load(Ordering::SeqCst) != generation { return false; }
    payload["generation"] = serde_json::json!(generation);
    let snapshot = {
        let mut snapshot = state.overlay_snapshot.lock().unwrap_or_else(|error| error.into_inner());
        snapshot.generation = generation;
        snapshot.state = current.clone();
        snapshot.error = Some(OverlayFailure { event: event.into(), payload: payload.clone() });
        snapshot.revision += 1;
        snapshot.clone()
    };
    drop(current);
    emit_snapshot(app, &snapshot);
    let _ = app.emit(event, payload);
    true
}

pub fn clear_overlay(app: &AppHandle, generation: u64, revision: Option<u64>) -> bool {
    let state = app.state::<crate::state::AppState>();
    // Capture acquisition and error publication use this same lock. Checking
    // before dispatch, or before acquiring it, permits an old hide to clear a
    // new recording/error between the check and snapshot mutation.
    let current = state.current_state.lock().unwrap_or_else(|error| error.into_inner());
    if state.recording_generation.load(std::sync::atomic::Ordering::SeqCst) != generation {
        return false;
    }
    let snapshot = {
        let mut snapshot = state.overlay_snapshot.lock().unwrap_or_else(|error| error.into_inner());
        if !snapshot.clear(&current, revision) { return false; }
        snapshot.clone()
    };
    drop(current);
    emit_snapshot(app, &snapshot);
    true
}

/// Check before requesting a restart: Tauri records restart intent before its
/// ExitRequested event, so rejecting that event alone would leave stale intent.
#[tauri::command]
pub fn restart_app(app: AppHandle, state: tauri::State<'_, crate::state::AppState>) -> Result<(), String> {
    state.begin_exit()?;
    app.request_restart();
    Ok(())
}

pub fn show_busy_exit_warning(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let _ = app.run_on_main_thread(|| {
            use tauri_nspanel::{objc2::MainThreadMarker, objc2_app_kit::NSAlert, objc2_foundation::NSString};
            if let Some(main_thread) = MainThreadMarker::new() {
                let alert = NSAlert::new(main_thread);
                alert.setMessageText(&NSString::from_str("Finish dictation before quitting"));
                alert.setInformativeText(&NSString::from_str("Stop or cancel the recording, then wait for transcription to finish. SottoASR will stay open to finish your dictation."));
                alert.addButtonWithTitle(&NSString::from_str("Keep SottoASR Open"));
                alert.runModal();
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    { let _ = app; }
}

#[tauri::command]
pub fn get_overlay_snapshot(state: tauri::State<'_, crate::state::AppState>) -> OverlaySnapshot {
    state.overlay_snapshot.lock().unwrap_or_else(|error| error.into_inner()).clone()
}

#[tauri::command]
pub async fn dismiss_overlay_error(app: AppHandle, revision: u64) -> Result<(), String> {
    crate::hotkeys::manager::hide_overlay_if_revision(&app, Some(revision));
    Ok(())
}

#[tauri::command]
pub async fn open_transcription_history(app: AppHandle) -> Result<(), String> {
    let handle = app.clone();
    app.run_on_main_thread(move || {
        crate::tray::menu::open_or_focus_window(
            &handle, "history", "history.html", "SottoASR — History", 520.0, 640.0,
        );
    }).map_err(|error| error.to_string())
}

/// Only an existing recording made by this app in the system temp directory
/// can be revealed. A webview cannot use this command as a general path opener.
fn recovery_audio_path(path: &std::path::Path, temporary: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let name = path.file_name().and_then(|name| name.to_str()).ok_or("Invalid recording filename")?;
    let id = name.strip_prefix("sotto_").and_then(|name| name.strip_suffix(".wav"))
        .ok_or("Invalid recording filename")?;
    uuid::Uuid::parse_str(id).map_err(|_| "Invalid recording filename")?;
    let metadata = std::fs::symlink_metadata(path).map_err(|error| format!("Recording is unavailable: {error}"))?;
    if !metadata.file_type().is_file() { return Err("Recording must be a regular file".into()); }
    let canonical = path.canonicalize().map_err(|error| error.to_string())?;
    let temporary = temporary.canonicalize().map_err(|error| error.to_string())?;
    if canonical.parent() != Some(temporary.as_path()) {
        return Err("Recording is outside SottoASR's temporary audio directory".into());
    }
    Ok(canonical)
}

#[tauri::command]
pub async fn reveal_recording_audio(path: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let path = recovery_audio_path(std::path::Path::new(&path), &std::env::temp_dir())?;
        #[cfg(target_os = "macos")]
        {
            let status = std::process::Command::new("open").arg("-R").arg(path).status()
                .map_err(|error| format!("Could not reveal recording: {error}"))?;
            if status.success() { Ok(()) } else { Err("Finder could not reveal the recording".into()) }
        }
        #[cfg(not(target_os = "macos"))]
        { let _ = path; Err("Revealing recordings is supported on macOS".into()) }
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command]
pub fn overlay_start_drag(app: AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let app_clone = app.clone();
        app.run_on_main_thread(move || {
            let panel = match app_clone.get_webview_panel("overlay") {
                Ok(p) => p,
                Err(e) => {
                    log::warn!("overlay_start_drag: no overlay panel ({:?})", e);
                    return;
                }
            };
            unsafe {
                // [NSApp currentEvent] — the in-flight NSEvent the
                // window server dispatched. When this command is called
                // from a mousedown JS handler, that event is the
                // leftMouseDown that performWindowDragWithEvent: wants.
                let ns_app: *mut tauri_nspanel::objc2_foundation::NSObject =
                    tauri_nspanel::objc2::msg_send![
                        tauri_nspanel::objc2::class!(NSApplication),
                        sharedApplication
                    ];
                if ns_app.is_null() {
                    log::warn!("overlay_start_drag: NSApp sharedApplication is null");
                    return;
                }
                let event: *mut tauri_nspanel::objc2_foundation::NSObject =
                    tauri_nspanel::objc2::msg_send![ns_app, currentEvent];
                if event.is_null() {
                    log::warn!("overlay_start_drag: no current NSEvent");
                    return;
                }
                let _: () = tauri_nspanel::objc2::msg_send![
                    panel.as_panel(), performWindowDragWithEvent: event
                ];
            }
        })
        .map_err(|e| e.to_string())?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_keeps_an_early_error_until_dismissal_or_a_new_recording() {
        use crate::models::AppStateEnum;
        let mut snapshot = OverlaySnapshot::default();
        snapshot.transition(7, AppStateEnum::Recording);
        assert!(snapshot.started_at_ms.is_some());
        snapshot.error = Some(OverlayFailure { event: "transcription-error".into(),
            payload: serde_json::json!({"error": "Model unavailable", "generation": 7}) });
        snapshot.transition(7, AppStateEnum::Idle);
        let initial_webview = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(initial_webview["error"]["payload"]["error"], "Model unavailable");
        let previous_revision = snapshot.revision;
        snapshot.transition(8, AppStateEnum::Recording);
        assert!(snapshot.revision > previous_revision);
        assert!(snapshot.error.is_none());
        assert_eq!(snapshot.generation, 8);
    }

    #[test]
    fn queued_automatic_hide_cannot_erase_same_generation_recovery() {
        use crate::models::AppStateEnum;
        let mut snapshot = OverlaySnapshot::default();
        snapshot.transition(1, AppStateEnum::Recording);
        let old_revision = snapshot.revision;
        snapshot.error = Some(OverlayFailure { event: "recording-error".into(),
            payload: serde_json::json!({"error": "Microphone disconnected"}) });
        snapshot.revision += 1;
        let error_revision = snapshot.revision;
        assert!(!snapshot.clear(&AppStateEnum::Transcribing, None));
        assert!(!snapshot.clear(&AppStateEnum::Idle, Some(old_revision)));
        assert!(!snapshot.clear(&AppStateEnum::Transcribing, Some(error_revision)));
        assert!(snapshot.error.is_some());
        assert!(snapshot.clear(&AppStateEnum::Idle, Some(error_revision)));
        assert!(snapshot.error.is_none());
    }

    #[test]
    fn audio_reveal_rejects_non_recordings_outside_paths_and_symlinks() {
        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let name = format!("sotto_{}.wav", uuid::Uuid::new_v4());
        let audio = directory.path().join(&name);
        std::fs::write(&audio, b"recording").unwrap();
        assert_eq!(recovery_audio_path(&audio, directory.path()).unwrap(), audio.canonicalize().unwrap());
        let private = directory.path().join("private.json");
        std::fs::write(&private, b"private").unwrap();
        assert!(recovery_audio_path(&private, directory.path()).is_err());
        let other_audio = outside.path().join(&name);
        std::fs::write(&other_audio, b"other").unwrap();
        assert!(recovery_audio_path(&other_audio, directory.path()).is_err());
        #[cfg(unix)]
        {
            let link = directory.path().join(format!("sotto_{}.wav", uuid::Uuid::new_v4()));
            std::os::unix::fs::symlink(other_audio, &link).unwrap();
            assert!(recovery_audio_path(&link, directory.path()).is_err());
        }
    }
}
