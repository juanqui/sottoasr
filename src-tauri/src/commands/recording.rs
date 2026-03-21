use tauri::{AppHandle, Emitter, State};
use crate::state::AppState;
use crate::models::AppStateEnum;

#[tauri::command]
pub async fn start_recording(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    // Recording is primarily triggered by hotkeys (see hotkeys/manager.rs).
    // This command exists for frontend-initiated recording if needed.
    let current = state.get_state();
    if current != AppStateEnum::Idle {
        return Err(format!("Cannot start recording: currently in {:?} state", current));
    }

    state.set_state(AppStateEnum::Recording);
    state.is_recording.store(true, std::sync::atomic::Ordering::SeqCst);

    app.emit("recording-started", ()).map_err(|e| e.to_string())?;
    app.emit("state-changed", &AppStateEnum::Recording).map_err(|e| e.to_string())?;

    log::info!("Recording started via command");
    Ok(())
}

#[tauri::command]
pub async fn stop_recording(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let current = state.get_state();
    if current != AppStateEnum::Recording {
        return Err(format!("Cannot stop recording: currently in {:?} state", current));
    }

    state.is_recording.store(false, std::sync::atomic::Ordering::SeqCst);
    state.set_state(AppStateEnum::Idle);

    app.emit("recording-stopped", ()).map_err(|e| e.to_string())?;
    app.emit("state-changed", &AppStateEnum::Idle).map_err(|e| e.to_string())?;

    log::info!("Recording stopped via command");
    Ok(())
}

#[tauri::command]
pub async fn cancel_recording(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.is_recording.store(false, std::sync::atomic::Ordering::SeqCst);
    {
        let mut capture = state.audio_capture.lock().unwrap();
        capture.stop();
    }
    state.set_state(AppStateEnum::Idle);

    // Drain any buffered audio
    if let Ok(rx) = state.audio_receiver.lock() {
        while rx.try_recv().is_ok() {}
    }

    app.emit("recording-cancelled", ()).map_err(|e| e.to_string())?;
    app.emit("state-changed", &AppStateEnum::Idle).map_err(|e| e.to_string())?;

    log::info!("Recording cancelled");
    Ok(())
}
