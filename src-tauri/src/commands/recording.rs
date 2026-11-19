use tauri::{AppHandle, State};
use crate::state::AppState;
use crate::models::AppStateEnum;

#[tauri::command]
pub async fn start_recording(app: AppHandle) -> Result<(), String> {
    crate::hotkeys::manager::handle_start_recording(&app).map(|_| ())
}

/// Returns once the recording is claimed and its audio is finalized;
/// transcription and delivery continue in the background.
#[tauri::command]
pub async fn stop_recording(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let current = state.get_state();
    if current != AppStateEnum::Recording {
        return Err(format!("Cannot stop recording: currently in {:?} state", current));
    }
    crate::hotkeys::manager::handle_stop_recording(&app, None).await;
    Ok(())
}

#[tauri::command]
pub async fn cancel_recording(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let current = state.get_state();
    if current != AppStateEnum::Recording {
        return Err(format!("Cannot cancel recording: currently in {:?} state", current));
    }
    crate::hotkeys::manager::handle_cancel_recording(&app).await;
    Ok(())
}
