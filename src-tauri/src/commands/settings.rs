use std::path::{Path, PathBuf};
use tauri::{AppHandle, State};
use crate::state::{AppState, ShortcutUpdate};
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
    match state.settings_load_error() {
        Some(error) => Err(format!("Your saved settings could not be read and were left unchanged. Choose Back up and reset to start from defaults. {error}")),
        None => Ok(()),
    }
}

/// Builds before this one require these keys. Without them a rollback cannot
/// parse the file and registers no hotkeys, so they are written with fixed
/// values that this build ignores on load.
fn settings_file_contents(settings: &Settings) -> Result<String, String> {
    let mut value = serde_json::to_value(settings)
        .map_err(|e| format!("Failed to serialize settings: {}", e))?;
    let object = value.as_object_mut().ok_or("Settings did not serialize to an object")?;
    object.insert("model_path".into(), serde_json::json!(""));
    object.insert("language".into(), serde_json::json!("auto"));
    object.insert("max_history".into(), serde_json::json!(500));
    serde_json::to_string_pretty(&value).map_err(|e| format!("Failed to serialize settings: {}", e))
}

fn write_settings_to(path: &Path, settings: &Settings) -> Result<(), String> {
    let data = settings_file_contents(settings)?;
    crate::persistence::write_atomic(path, data.as_bytes())
        .map_err(|e| format!("Failed to write settings file: {}", e))
}

/// Save settings to disk.
fn persist_settings(settings: &Settings) -> Result<(), String> {
    let path = settings_path()?;
    write_settings_to(&path, settings)?;
    log::info!("Settings persisted to {:?}", path);
    Ok(())
}

/// Normalize, then apply the same validation as startup, so a saved file can
/// never disable the hotkeys at the next launch.
fn checked_settings(mut settings: Settings) -> Result<Settings, String> {
    settings.normalize()?;
    settings.validate()?;
    Ok(settings)
}

#[tauri::command]
pub async fn get_settings(
    state: State<'_, AppState>,
) -> Result<Settings, String> {
    ensure_settings_loaded(&state)?;
    let settings = state.settings.lock().await;
    Ok(settings.clone())
}

/// Why the saved settings could not be read; `None` when they loaded normally.
#[tauri::command]
pub async fn get_settings_load_error(
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    Ok(state.settings_load_error())
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

/// Run `job` on the main thread, where shortcuts are registered, once the app
/// is Idle. Returns the job's actual result, not merely dispatch success.
async fn on_main_thread_when_idle<T: Send + 'static>(
    app: &AppHandle,
    job: impl FnOnce(&AppHandle) -> T + Send + 'static,
) -> Result<T, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        use tauri::Manager;
        let state: tauri::State<'_, AppState> = handle.state();
        let result = if state.get_state() != AppStateEnum::Idle {
            Err("Finish recording before changing keyboard shortcuts".to_string())
        } else {
            Ok(job(&handle))
        };
        let _ = sender.send(result);
    }).map_err(|e| format!("Failed to dispatch shortcuts to main thread: {e}"))?;
    receiver.await.map_err(|_| "Shortcut registration did not complete".to_string())?
}

/// Activate `next`; if any of it fails, register `rollback` again.
fn activate_shortcuts(
    next: &Settings,
    rollback: &Settings,
    mut register: impl FnMut(&Settings) -> Result<(), String>,
) -> Result<(), String> {
    register(next).map_err(|error| match register(rollback) {
        Ok(()) => format!("Could not activate the new shortcuts ({error}). The previous shortcuts are active again."),
        Err(restore) => format!("Could not activate the new shortcuts ({error}). Restoring the previous shortcuts: {restore}."),
    })
}

/// After a failed save, memory and disk still hold `previous`, so register
/// exactly that, once, and keep a partial result: falling back to the unsaved
/// shortcuts would leave them active while nothing records them. Returns the
/// save error, plus what the restore could not register.
fn restore_after_failed_save(
    save_error: String,
    previous: &Settings,
    register: impl FnOnce(&Settings) -> Result<(), String>,
) -> String {
    match register(previous) {
        Ok(()) => save_error,
        Err(restore) => format!("{save_error}. Restoring the previous shortcuts: {restore}."),
    }
}

async fn register_settings_shortcuts(
    app: &AppHandle,
    next: Settings,
    rollback: Settings,
) -> Result<(), String> {
    on_main_thread_when_idle(app, move |handle| {
        activate_shortcuts(&next, &rollback, |settings| crate::hotkeys::manager::register_shortcuts(handle, settings))
    }).await?
}

/// Re-register the in-memory shortcuts. Callers hold `settings_update` and the
/// shortcut-update guard. There is nothing to roll back to, so a partial
/// result stays and its error names the shortcuts that are not active.
pub(crate) async fn register_saved_shortcuts(app: &AppHandle, state: &AppState) -> Result<(), String> {
    let settings = state.settings.lock().await.clone();
    on_main_thread_when_idle(app, move |handle| crate::hotkeys::manager::register_shortcuts(handle, &settings)).await?
}

#[tauri::command]
pub async fn update_settings(
    app: AppHandle,
    new_settings: Settings,
    state: State<'_, AppState>,
) -> Result<UpdateSettingsResult, String> {
    let _update = state.settings_update.lock().await;
    ensure_settings_loaded(&state)?;
    let new_settings = checked_settings(new_settings)?;
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
        if !changed_shortcuts {
            return Err(error);
        }
        let saved_shortcuts = previous.clone();
        let save_error = error.clone();
        let restored = on_main_thread_when_idle(&app, move |handle| {
            restore_after_failed_save(save_error, &saved_shortcuts, |settings| {
                crate::hotkeys::manager::register_shortcuts(handle, settings)
            })
        }).await;
        return Err(restored.unwrap_or_else(|restore| format!("{error}. Restoring the previous shortcuts: {restore}.")));
    }
    *state.settings.lock().await = new_settings.clone();
    state.apply_runtime_preferences(&new_settings);
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
    crate::commands::speech::settings_saved(app.clone(), &previous, &new_settings);
    log::info!("Settings updated and persisted");
    let retention_changed = previous.history_retention_days != new_settings.history_retention_days
        || previous.audio_retention_days != new_settings.audio_retention_days;
    // Retention takes the settings lock and the Idle recovery claim itself.
    drop(_update);
    if retention_changed {
        crate::commands::recovery::enforce_retention(&app).await;
    }
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
    register_saved_shortcuts(&app, &state).await
}

/// Reset exists only to repair an unreadable file. It re-registers shortcuts,
/// so it needs the same Idle guard as a shortcut change.
fn begin_settings_reset(state: &AppState) -> Result<ShortcutUpdate<'_>, String> {
    if state.settings_load_error().is_none() {
        return Err("Your settings loaded normally, so there is nothing to reset.".into());
    }
    state.begin_shortcut_update()
}

/// Move the unreadable file aside, so nothing the user saved is lost, and
/// `write` the new file in its place. If the write fails the file is put back.
/// Returns where the old file now lives, or `None` when there was none.
fn replace_unreadable_settings(
    path: &Path,
    now: chrono::DateTime<chrono::Utc>,
    write: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<Option<PathBuf>, String> {
    let mut backup = path.as_os_str().to_owned();
    backup.push(format!(".unreadable-{}", now.format("%Y%m%dT%H%M%SZ")));
    let backup = PathBuf::from(backup);
    let moved = match std::fs::rename(path, &backup) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(format!("Could not back up the unreadable settings file: {error}")),
    };
    if let Err(error) = write(path) {
        if moved {
            if let Err(restore) = std::fs::rename(&backup, path) {
                return Err(format!("{error}. The unreadable file is kept at {}: {restore}", backup.display()));
            }
        }
        return Err(error);
    }
    Ok(moved.then_some(backup))
}

/// Back up the unreadable settings file, start again from defaults, and turn
/// on the default shortcuts.
#[tauri::command]
pub async fn reset_settings(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Settings, String> {
    let _update = state.settings_update.lock().await;
    let _shortcut_update = begin_settings_reset(&state)?;
    let defaults = Settings::default();
    let written = defaults.clone();
    let backup = tokio::task::spawn_blocking(move || {
        replace_unreadable_settings(&settings_path()?, chrono::Utc::now(), |path| write_settings_to(path, &written))
    })
    .await
    .map_err(|e| format!("Settings reset task failed: {e}"))??;
    match backup {
        Some(backup) => log::info!("Unreadable settings kept at {:?}; defaults saved", backup),
        None => log::info!("No settings file to back up; defaults saved"),
    }
    *state.settings.lock().await = defaults.clone();
    state.apply_runtime_preferences(&defaults);
    *state.settings_load_error.lock().unwrap_or_else(|error| error.into_inner()) = None;
    register_saved_shortcuts(&app, &state).await.map_err(|error| {
        format!("Settings were reset. Choose other keyboard shortcuts and save, because these could not be turned on: {error}")
    })?;
    Ok(defaults)
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
        let s = Settings { history_retention_days: 42, sound_cues: true, ..Default::default() };
        let json = serde_json::to_string_pretty(&s).unwrap();
        std::fs::write(&path, json).unwrap();

        let loaded = read_settings_from(&path).unwrap();
        assert_eq!(loaded.history_retention_days, 42);
        assert!(loaded.sound_cues);
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
            history_retention_days: 999,
            input_device: Some("External Microphone".into()),
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

    #[test]
    fn failed_save_restores_only_the_saved_shortcuts() {
        use crate::hotkeys::manager::plan_shortcuts;
        let active = std::cell::RefCell::new(None::<Settings>);
        let calls = std::cell::Cell::new(0);
        // Stands in for the lenient registrar: every readable entry becomes
        // active and the error names the rest.
        let register = |settings: &Settings| -> Result<(), String> {
            calls.set(calls.get() + 1);
            *active.borrow_mut() = Some(settings.clone());
            let skipped: Vec<_> = plan_shortcuts(settings).into_iter()
                .filter(|entry| entry.shortcut.is_err())
                .map(|entry| entry.label)
                .collect();
            if skipped.is_empty() {
                Ok(())
            } else {
                Err(format!("every other shortcut was registered; skipped: {}", skipped.join("; ")))
            }
        };
        // The saved settings hold one key the registrar cannot read.
        let previous = Settings { toggle_shortcut: "Ctrl+NotAKey".into(), ..Default::default() };
        let unsaved = Settings { toggle_shortcut: "Alt+D".into(), ..Default::default() };

        // The new shortcuts register, then the file write fails.
        activate_shortcuts(&unsaved, &previous, &register).unwrap();
        assert_eq!(active.borrow().as_ref(), Some(&unsaved));
        let message = restore_after_failed_save("Failed to write settings file: disk full".into(), &previous, register);

        assert_eq!(active.borrow().as_ref(), Some(&previous), "the unsaved shortcuts must not stay active");
        assert_eq!(calls.get(), 2);
        assert_eq!(
            message,
            "Failed to write settings file: disk full. Restoring the previous shortcuts: every other shortcut was registered; skipped: Toggle recording.",
        );
        // A complete restore leaves the save error as it is.
        assert_eq!(restore_after_failed_save("disk full".into(), &unsaved, register), "disk full");
    }

    #[test]
    fn save_rejects_a_duplicate_shortcut_and_an_incomplete_dictionary_row() {
        let duplicate = Settings {
            toggle_shortcut: Settings::default().push_to_talk_shortcut,
            ..Default::default()
        };
        assert!(checked_settings(duplicate).unwrap_err().contains("cannot be the same"));
        let incomplete = Settings {
            dictionary: vec![crate::models::DictionaryEntry { heard: " Quen ".into(), replacement: " ".into() }],
            ..Default::default()
        };
        assert!(checked_settings(incomplete).unwrap_err().contains("Dictionary row 1"));
        let untouched_row = Settings {
            dictionary: vec![crate::models::DictionaryEntry { heard: " ".into(), replacement: String::new() }],
            ..Default::default()
        };
        assert!(checked_settings(untouched_row).unwrap().dictionary.is_empty());
    }

    #[test]
    fn written_file_still_parses_in_builds_that_require_the_legacy_keys() {
        // The required fields of `Settings` in builds before this one.
        #[derive(serde::Deserialize)]
        struct PreviousBuildSettings {
            push_to_talk_shortcut: String,
            toggle_shortcut: String,
            cancel_shortcut: String,
            show_overlay: bool,
            auto_paste: bool,
            restore_clipboard: bool,
            model_path: String,
            language: String,
            max_history: usize,
            launch_at_login: bool,
        }
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        let settings = Settings {
            toggle_shortcut: "Alt+D".into(),
            show_overlay: false,
            sound_cues: true,
            audio_retention_days: 30,
            ..Default::default()
        };
        write_settings_to(&path, &settings).unwrap();

        let previous: PreviousBuildSettings = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(previous.model_path, "");
        assert_eq!(previous.language, "auto");
        assert_eq!(previous.max_history, 500);
        assert_eq!(previous.push_to_talk_shortcut, settings.push_to_talk_shortcut);
        assert_eq!(previous.toggle_shortcut, "Alt+D");
        assert_eq!(previous.cancel_shortcut, settings.cancel_shortcut);
        assert!(!previous.show_overlay);
        assert_eq!(previous.auto_paste, settings.auto_paste);
        assert_eq!(previous.restore_clipboard, settings.restore_clipboard);
        assert_eq!(previous.launch_at_login, settings.launch_at_login);
        // This build ignores the legacy keys and reads back what it saved.
        assert_eq!(read_settings_from(&path).unwrap(), settings);
    }

    fn reset_time() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-11-19T08:30:05Z").unwrap().with_timezone(&chrono::Utc)
    }

    fn write_defaults(path: &Path) -> Result<(), String> {
        write_settings_to(path, &Settings::default())
    }

    #[test]
    fn reset_backs_up_the_unreadable_file_and_writes_defaults() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, b"{saved-but-incomplete").unwrap();

        let backup = replace_unreadable_settings(&path, reset_time(), write_defaults).unwrap().unwrap();
        assert_eq!(backup, dir.path().join("settings.json.unreadable-20261119T083005Z"));
        assert_eq!(std::fs::read(&backup).unwrap(), b"{saved-but-incomplete");
        assert_eq!(read_settings_from(&path).unwrap(), Settings::default());

        std::fs::remove_file(&path).unwrap();
        assert_eq!(replace_unreadable_settings(&path, reset_time(), write_defaults).unwrap(), None);
        assert_eq!(read_settings_from(&path).unwrap(), Settings::default());
    }

    #[test]
    fn failed_reset_write_puts_the_unreadable_file_back() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, b"{saved-but-incomplete").unwrap();

        let error = replace_unreadable_settings(&path, reset_time(), |target| {
            // The original has already been moved aside when the write runs.
            assert!(!target.exists());
            Err("Failed to write settings file: disk full".into())
        }).unwrap_err();
        assert!(error.contains("disk full"));
        assert_eq!(std::fs::read(&path).unwrap(), b"{saved-but-incomplete");
        let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("settings.json")]);
    }

    #[test]
    fn reset_refuses_after_a_normal_load_and_while_not_idle() {
        use crate::test_support::{MockAsrEngine, MockAudioCapture, MockPasteBackend};
        let state = AppState::new_with_backends(
            Box::new(MockAudioCapture::new(vec![0.1; 4800], 48_000)),
            Box::new(MockAsrEngine::with_text("test")), None,
            Box::new(MockPasteBackend::new()), Settings::default(),
        );
        // `ShortcutUpdate` is a guard without `Debug`, so read the error with `err()`.
        assert!(begin_settings_reset(&state).err().unwrap().contains("nothing to reset"));

        *state.settings_load_error.lock().unwrap() = Some("Could not parse settings".into());
        state.set_state(AppStateEnum::Recording);
        assert!(begin_settings_reset(&state).err().unwrap().contains("Finish recording"));
        assert!(!state.shortcuts_updating.load(std::sync::atomic::Ordering::SeqCst));

        state.set_state(AppStateEnum::Idle);
        let reset = begin_settings_reset(&state).unwrap();
        assert!(state.shortcuts_updating.load(std::sync::atomic::Ordering::SeqCst));
        assert!(begin_settings_reset(&state).is_err());
        drop(reset);
        assert!(!state.shortcuts_updating.load(std::sync::atomic::Ordering::SeqCst));
    }
}
