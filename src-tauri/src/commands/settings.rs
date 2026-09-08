use std::path::{Path, PathBuf};
use tauri::{AppHandle, State};
use crate::state::AppState;
use crate::models::{AppStateEnum, Settings};

/// Get the persistent settings file path.
/// Stored alongside transcriptions in ~/Library/Application Support/com.sottoasr.app/
fn settings_path() -> Result<PathBuf, String> {
    let data_dir = dirs::data_dir().ok_or("Could not determine data directory")?;
    let app_dir = data_dir.join("com.sottoasr.app");
    std::fs::create_dir_all(&app_dir)
        .map_err(|e| format!("Failed to create app data dir: {}", e))?;
    Ok(app_dir.join("settings.json"))
}

/// Read the saved file without hiding a permission error or invalid JSON.
fn read_settings_from(path: &Path) -> Result<Settings, String> {
    let data = match std::fs::read_to_string(path) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Settings::default()),
        Err(error) => return Err(format!("Could not read settings: {error}")),
    };
    serde_json::from_str(&data).map_err(|error| format!("Could not parse settings: {error}"))
}

pub fn load_persisted_settings_checked() -> Result<Settings, String> {
    read_settings_from(&settings_path()?)
}

fn ensure_settings_loaded(state: &AppState) -> Result<(), String> {
    match &state.settings_load_error {
        Some(error) => Err(format!("Your saved settings could not be loaded and have been preserved. Repair settings.json in SottoASR's Application Support folder, then restart SottoASR. {error}")),
        None => Ok(()),
    }
}

/// Save settings to disk.
fn persist_settings(settings: &Settings) -> Result<(), String> {
    let path = settings_path()?;
    let data = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("Failed to serialize settings: {}", e))?;
    crate::persistence::write_atomic(&path, data.as_bytes())
        .map_err(|e| format!("Failed to write settings file: {}", e))?;
    log::info!("Settings persisted to {:?}", path);
    Ok(())
}

#[tauri::command]
pub async fn get_settings(
    state: State<'_, AppState>,
) -> Result<Settings, String> {
    ensure_settings_loaded(&state)?;
    let settings = state.settings.lock().await;
    Ok(settings.clone())
}

#[derive(serde::Serialize)]
pub struct UpdateSettingsResult {
    pub settings: Settings,
    pub warnings: Vec<String>,
}

fn shortcuts_changed(previous: &Settings, next: &Settings) -> bool {
    previous.push_to_talk_shortcut != next.push_to_talk_shortcut
        || previous.push_to_talk_shortcut_alt != next.push_to_talk_shortcut_alt
        || previous.toggle_shortcut != next.toggle_shortcut
        || previous.toggle_shortcut_alt != next.toggle_shortcut_alt
        || previous.cancel_shortcut != next.cancel_shortcut
        || previous.cancel_shortcut_alt != next.cancel_shortcut_alt
        || previous.open_settings_shortcut != next.open_settings_shortcut
}

/// Return the actual registration result, not merely main-thread dispatch success.
async fn register_settings_shortcuts(
    app: &AppHandle,
    next: Settings,
    rollback: Settings,
) -> Result<(), String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        use tauri::Manager;
        let state: tauri::State<'_, AppState> = handle.state();
        let result = if state.get_state() != AppStateEnum::Idle {
            Err("Finish recording before changing keyboard shortcuts".into())
        } else {
            crate::hotkeys::manager::register_shortcuts(&handle, &next).map_err(|error| {
                match crate::hotkeys::manager::register_shortcuts(&handle, &rollback) {
                    Ok(()) => format!("Could not activate shortcuts; previous shortcuts restored: {error}"),
                    Err(restore) => format!("Could not activate shortcuts ({error}) or restore previous shortcuts ({restore})"),
                }
            })
        };
        let _ = sender.send(result);
    }).map_err(|e| format!("Failed to dispatch shortcuts to main thread: {e}"))?;
    receiver.await.map_err(|_| "Shortcut registration did not complete".to_string())?
}

#[tauri::command]
pub async fn update_settings(
    app: AppHandle,
    mut new_settings: Settings,
    state: State<'_, AppState>,
) -> Result<UpdateSettingsResult, String> {
    let _update = state.settings_update.lock().await;
    ensure_settings_loaded(&state)?;
    new_settings.normalize()?;
    let previous = state.settings.lock().await.clone();
    let changed_shortcuts = shortcuts_changed(&previous, &new_settings);
    let _shortcut_update = if changed_shortcuts {
        Some(state.begin_shortcut_update()?)
    } else { None };
    if changed_shortcuts {
        register_settings_shortcuts(&app, new_settings.clone(), previous.clone()).await?;
    }

    let submitted = new_settings.clone();
    let saved = tokio::task::spawn_blocking(move || persist_settings(&submitted))
        .await.map_err(|e| format!("Settings save task failed: {e}"))
        .and_then(|result| result);
    if let Err(error) = saved {
        if changed_shortcuts {
            if let Err(restore) = register_settings_shortcuts(&app, previous.clone(), new_settings).await {
                return Err(format!("{error}. Restoring shortcuts also failed: {restore}"));
            }
        }
        return Err(error);
    }
    *state.settings.lock().await = new_settings.clone();
    drop(_shortcut_update);

    let mut warnings = Vec::new();
    if previous.launch_at_login != new_settings.launch_at_login {
        let handle = app.clone();
        let enabled = new_settings.launch_at_login;
        let result = tokio::task::spawn_blocking(move || {
            use tauri_plugin_autostart::ManagerExt;
            let manager = handle.autolaunch();
            if enabled { manager.enable() } else { manager.disable() }
        }).await;
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => warnings.push(format!("Settings saved, but launch at login could not be updated: {error}")),
            Err(error) => warnings.push(format!("Settings saved, but the login-item task failed: {error}")),
        }
    }
    if previous.llm_cleanup_enabled != new_settings.llm_cleanup_enabled {
        crate::commands::llm::notify_on_cleanup_preference_saved(&app, new_settings.llm_cleanup_enabled);
    }
    crate::commands::vocabulary::settings_saved(app.clone(), &previous, &new_settings);
    log::info!("Settings updated and persisted");
    Ok(UpdateSettingsResult { settings: new_settings, warnings })
}

/// Re-register global shortcuts from the current settings.
#[tauri::command]
pub async fn apply_shortcuts(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let _update = state.settings_update.lock().await;
    let _shortcut_update = state.begin_shortcut_update()?;
    let settings = state.settings.lock().await.clone();
    register_settings_shortcuts(&app, settings.clone(), settings).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn load_from_nonexistent_returns_defaults() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("does_not_exist.json");
        let settings = read_settings_from(&path).unwrap();
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn load_from_valid_json() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        let s = Settings { max_history: 42, language: "en".into(), ..Default::default() };
        let json = serde_json::to_string_pretty(&s).unwrap();
        std::fs::write(&path, json).unwrap();

        let loaded = read_settings_from(&path).unwrap();
        assert_eq!(loaded.max_history, 42);
        assert_eq!(loaded.language, "en");
    }

    #[test]
    fn load_from_invalid_json_returns_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "this is not json").unwrap();
        assert!(read_settings_from(&path).is_err());
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

        let loaded = read_settings_from(&path).unwrap();
        assert!(!loaded.show_overlay);
        assert_eq!(loaded.max_history, 100);
        // Fields with serde defaults should be populated
        assert!(loaded.restore_focus_before_paste); // default_true
        assert!(!loaded.llm_cleanup_enabled); // default false
        assert!(loaded.dictionary.is_empty());
        assert!(loaded.auto_check_updates); // default_true
        assert_eq!(loaded.open_settings_shortcut, "CommandOrControl+Shift+Comma");
    }

    #[test]
    fn load_from_empty_file_returns_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "").unwrap();
        assert!(read_settings_from(&path).is_err());
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
            dictionary: vec![crate::models::DictionaryEntry {
                heard: "Quen".into(), replacement: "Qwen".into(),
            }],
            auto_check_updates: false,
            ..Default::default()
        };

        let json = serde_json::to_string_pretty(&original).unwrap();
        std::fs::write(&path, json).unwrap();

        let loaded = read_settings_from(&path).unwrap();
        assert_eq!(loaded, original);
    }

    #[test]
    fn checked_load_rejects_corrupt_file_without_changing_it() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, b"{saved-but-incomplete").unwrap();
        assert!(read_settings_from(&path).is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"{saved-but-incomplete");
    }

    #[test]
    fn unrelated_settings_do_not_rebind_shortcuts() {
        let original = Settings::default();
        let mut changed = original.clone();
        changed.llm_cleanup_enabled = true;
        changed.launch_at_login = !original.launch_at_login;
        changed.vocabulary = vec!["Qwen".into()];
        assert!(!shortcuts_changed(&original, &changed));
        changed.toggle_shortcut_alt = Some("F18".into());
        assert!(shortcuts_changed(&original, &changed));
    }
}
