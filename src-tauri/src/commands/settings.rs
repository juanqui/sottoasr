use tauri::State;
use crate::state::AppState;
use crate::models::Settings;

#[tauri::command]
pub async fn get_settings(
    state: State<'_, AppState>,
) -> Result<Settings, String> {
    let settings = state.settings.lock().await;
    Ok(settings.clone())
}

#[tauri::command]
pub async fn update_settings(
    new_settings: Settings,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut settings = state.settings.lock().await;
    *settings = new_settings;
    log::info!("Settings updated");
    Ok(())
}
