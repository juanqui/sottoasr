use std::path::PathBuf;
use tauri::{AppHandle, State};
use crate::state::AppState;
use crate::models::Settings;

/// Get the persistent settings file path.
/// Stored alongside transcriptions in ~/Library/Application Support/com.sottoasr.app/
fn settings_path() -> Result<PathBuf, String> {
    let data_dir = dirs::data_dir().ok_or("Could not determine data directory")?;
    let app_dir = data_dir.join("com.sottoasr.app");
    std::fs::create_dir_all(&app_dir)
        .map_err(|e| format!("Failed to create app data dir: {}", e))?;
    Ok(app_dir.join("settings.json"))
}

/// Load settings from disk, falling back to defaults if not found.
pub fn load_persisted_settings() -> Settings {
    match settings_path() {
        Ok(path) if path.exists() => {
            match std::fs::read_to_string(&path) {
                Ok(data) => {
                    match serde_json::from_str::<Settings>(&data) {
                        Ok(settings) => {
                            log::info!("Loaded settings from {:?}", path);
                            settings
                        }
                        Err(e) => {
                            log::warn!("Failed to parse settings file, using defaults: {}", e);
                            Settings::default()
                        }
                    }
                }
                Err(e) => {
                    log::warn!("Failed to read settings file, using defaults: {}", e);
                    Settings::default()
                }
            }
        }
        _ => Settings::default(),
    }
}

/// Save settings to disk.
fn persist_settings(settings: &Settings) -> Result<(), String> {
    let path = settings_path()?;
    let data = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("Failed to serialize settings: {}", e))?;
    std::fs::write(&path, data)
        .map_err(|e| format!("Failed to write settings file: {}", e))?;
    log::info!("Settings persisted to {:?}", path);
    Ok(())
}

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
    *settings = new_settings.clone();
    drop(settings);

    // Persist to disk
    if let Err(e) = persist_settings(&new_settings) {
        log::error!("Failed to persist settings: {}", e);
    }

    log::info!("Settings updated");
    Ok(())
}

/// Re-register global shortcuts from the current settings.
#[tauri::command]
pub async fn apply_shortcuts(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let settings = state.settings.lock().await;
    let ptt = settings.push_to_talk_shortcut.clone();
    let ptt_alt = settings.push_to_talk_shortcut_alt.clone();
    let toggle = settings.toggle_shortcut.clone();
    let toggle_alt = settings.toggle_shortcut_alt.clone();
    let cancel = settings.cancel_shortcut.clone();
    let cancel_alt = settings.cancel_shortcut_alt.clone();
    drop(settings);

    let app_clone = app.clone();
    app.run_on_main_thread(move || {
        match crate::hotkeys::manager::register_shortcuts(
            &app_clone,
            &ptt,
            ptt_alt.as_deref(),
            &toggle,
            toggle_alt.as_deref(),
            &cancel,
            cancel_alt.as_deref(),
        ) {
            Ok(()) => log::info!("Shortcuts re-applied from settings"),
            Err(e) => log::error!("Failed to re-apply shortcuts: {}", e),
        }
    }).map_err(|e| format!("Failed to dispatch to main thread: {}", e))?;

    Ok(())
}
