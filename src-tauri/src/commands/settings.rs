use std::path::{Path, PathBuf};
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

/// Load settings from a specific path, falling back to defaults if not found or invalid.
pub fn load_settings_from(path: &Path) -> Settings {
    if !path.exists() {
        return Settings::default();
    }
    match std::fs::read_to_string(path) {
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

/// Load settings from disk, falling back to defaults if not found.
pub fn load_persisted_settings() -> Settings {
    match settings_path() {
        Ok(path) => load_settings_from(&path),
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
    app: AppHandle,
    new_settings: Settings,
    state: State<'_, AppState>,
) -> Result<(), String> {
    new_settings.validate()?;

    // Persist to disk first — if this fails, don't update in-memory state
    persist_settings(&new_settings)?;

    // Sync launch-at-login with macOS login items
    {
        use tauri_plugin_autostart::ManagerExt;
        let manager = app.autolaunch();
        let result = if new_settings.launch_at_login {
            manager.enable()
        } else {
            manager.disable()
        };
        if let Err(e) = result {
            log::warn!("Failed to sync autostart state: {}", e);
        }
    }

    let mut settings = state.settings.lock().await;
    *settings = new_settings;

    log::info!("Settings updated and persisted");
    Ok(())
}

/// Re-register global shortcuts from the current settings.
#[tauri::command]
pub async fn apply_shortcuts(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let settings = state.settings.lock().await.clone();

    let app_clone = app.clone();
    app.run_on_main_thread(move || {
        match crate::hotkeys::manager::register_shortcuts(&app_clone, &settings) {
            Ok(()) => log::info!("Shortcuts re-applied from settings"),
            Err(e) => log::error!("Failed to re-apply shortcuts: {}", e),
        }
    }).map_err(|e| format!("Failed to dispatch to main thread: {}", e))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn load_from_nonexistent_returns_defaults() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("does_not_exist.json");
        let settings = load_settings_from(&path);
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn load_from_valid_json() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        let s = Settings { max_history: 42, language: "en".into(), ..Default::default() };
        let json = serde_json::to_string_pretty(&s).unwrap();
        std::fs::write(&path, json).unwrap();

        let loaded = load_settings_from(&path);
        assert_eq!(loaded.max_history, 42);
        assert_eq!(loaded.language, "en");
    }

    #[test]
    fn load_from_invalid_json_returns_defaults() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "this is not json").unwrap();

        let loaded = load_settings_from(&path);
        assert_eq!(loaded, Settings::default());
    }

    #[test]
    fn load_from_partial_json_uses_serde_defaults() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        let json = r#"{
            "push_to_talk_shortcut": "Ctrl+Space",
            "toggle_shortcut": "Ctrl+D",
            "cancel_shortcut": "Escape",
            "show_overlay": false,
            "auto_paste": true,
            "restore_clipboard": true,
            "model_path": "",
            "language": "auto",
            "max_history": 100,
            "launch_at_login": false
        }"#;
        std::fs::write(&path, json).unwrap();

        let loaded = load_settings_from(&path);
        assert!(!loaded.show_overlay);
        assert_eq!(loaded.max_history, 100);
        // Fields with serde defaults should be populated
        assert!(loaded.restore_focus_before_paste); // default_true
        assert!(!loaded.llm_cleanup_enabled); // default false
        assert!(loaded.auto_check_updates); // default_true
        assert_eq!(loaded.open_settings_shortcut, "CommandOrControl+Shift+Comma");
    }

    #[test]
    fn load_from_empty_file_returns_defaults() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "").unwrap();

        let loaded = load_settings_from(&path);
        assert_eq!(loaded, Settings::default());
    }

    #[test]
    fn load_round_trip_preserves_all_fields() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        let original = Settings {
            push_to_talk_shortcut: "Alt+Space".into(),
            push_to_talk_shortcut_alt: Some("Ctrl+Alt+Space".into()),
            toggle_shortcut: "Alt+D".into(),
            cancel_shortcut: "Alt+Escape".into(),
            show_overlay: false,
            auto_paste: false,
            max_history: 999,
            llm_cleanup_enabled: true,
            auto_check_updates: false,
            ..Default::default()
        };

        let json = serde_json::to_string_pretty(&original).unwrap();
        std::fs::write(&path, json).unwrap();

        let loaded = load_settings_from(&path);
        assert_eq!(loaded, original);
    }
}
