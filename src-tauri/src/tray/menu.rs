use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager, WebviewUrl,
};

use std::io::{Read, Seek, SeekFrom};

// Compile-time embedded tray icons (PNG template images).
const TRAY_ICON_NORMAL: &[u8] = include_bytes!("../../icons/tray-iconTemplate.png");
const TRAY_ICON_UPDATE: &[u8] = include_bytes!("../../icons/tray-icon-updateTemplate.png");

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initial tray setup.  Called once during app launch.
pub fn setup_tray_menu(app: &AppHandle) -> Result<(), String> {
    build_tray_menu(app, TrayState::Normal)
}

/// Single canonical tray refresh. Reads UpdateState directly.
/// Call from anywhere — periodic loop, manual check, download complete.
pub async fn refresh_tray_from_state(app: &AppHandle) {
    let state = match app.try_state::<crate::updater::UpdateState>() {
        Some(state) => state,
        None => return,
    };
    let (has_update, tray_state) = read_tray_state(&state).await;
    let handle = app.clone();
    if let Err(error) = app.run_on_main_thread(move || {
        set_tray_icon(&handle, has_update);
        if let Err(error) = build_tray_menu(&handle, tray_state) {
            log::error!("Failed to rebuild tray menu: {error}");
        }
    }) {
        log::error!("Failed to dispatch tray refresh: {error}");
    }
}

/// The updater calls this from async tasks. A blocking_lock here panics even
/// when the mutex is uncontended, terminating the periodic update checker.
async fn read_tray_state(state: &crate::updater::UpdateState) -> (bool, TrayState) {
    let version = state.available_version.lock().await.clone();
    let app_update = state.update_available.load(std::sync::atomic::Ordering::SeqCst);
    let model_update = state.model_update_available.load(std::sync::atomic::Ordering::SeqCst);
    let restart = state.restart_pending.load(std::sync::atomic::Ordering::SeqCst);
    let tray_state = if restart {
        TrayState::RestartPending
    } else if let Some(version) = version {
        TrayState::UpdateAvailable(if model_update { format!("{version} (+ model)") } else { version })
    } else if model_update {
        TrayState::ModelUpdateAvailable
    } else {
        TrayState::Normal
    };
    (app_update || model_update || restart, tray_state)
}

// ---------------------------------------------------------------------------
// Internal: tray state enum
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
enum TrayState {
    Normal,
    UpdateAvailable(String), // version string, may include " (+ model)" suffix
    RestartPending,
    ModelUpdateAvailable,
}

// ---------------------------------------------------------------------------
// Internal: build/rebuild the menu
// ---------------------------------------------------------------------------

fn build_tray_menu(app: &AppHandle, state: TrayState) -> Result<(), String> {
    // -- Build menu items --
    let copy_last =
        MenuItem::with_id(app, "copy_last", "Copy Last Transcription", true, None::<&str>)
            .map_err(|e| e.to_string())?;
    let view_history =
        MenuItem::with_id(app, "view_history", "View Transcription History", true, None::<&str>)
            .map_err(|e| e.to_string())?;
    let settings =
        MenuItem::with_id(app, "settings", "Settings...", true, None::<&str>)
            .map_err(|e| e.to_string())?;

    // Single update item — label varies by state, always opens the update window.
    let update_label = match &state {
        TrayState::Normal => "Check for Updates...".to_string(),
        TrayState::UpdateAvailable(version) => {
            format!("Update Available \u{2014} {}", version)
        }
        TrayState::RestartPending => "Restart to Update".to_string(),
        TrayState::ModelUpdateAvailable => "AI Model Update Available...".to_string(),
    };
    let check_updates =
        MenuItem::with_id(app, "check_updates", &update_label, true, None::<&str>)
            .map_err(|e| e.to_string())?;

    let copy_diagnostics =
        MenuItem::with_id(app, "copy_diagnostics", "Copy Diagnostics", true, None::<&str>)
            .map_err(|e| e.to_string())?;
    let about =
        MenuItem::with_id(app, "about", "About SottoASR", true, None::<&str>)
            .map_err(|e| e.to_string())?;
    let quit =
        MenuItem::with_id(app, "quit", "Quit SottoASR", true, None::<&str>)
            .map_err(|e| e.to_string())?;

    let sep1 = PredefinedMenuItem::separator(app).map_err(|e| e.to_string())?;
    let sep2 = PredefinedMenuItem::separator(app).map_err(|e| e.to_string())?;
    let sep3 = PredefinedMenuItem::separator(app).map_err(|e| e.to_string())?;

    let items: Vec<Box<dyn tauri::menu::IsMenuItem<tauri::Wry>>> = vec![
        Box::new(copy_last),
        Box::new(view_history),
        Box::new(sep1),
        Box::new(settings),
        Box::new(check_updates),
        Box::new(copy_diagnostics),
        Box::new(sep2),
        Box::new(about),
        Box::new(sep3),
        Box::new(quit),
    ];

    // Build the Menu from the item refs.
    let item_refs: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> =
        items.iter().map(|b| b.as_ref()).collect();
    let menu = Menu::with_items(app, &item_refs).map_err(|e| e.to_string())?;

    // Get or create the tray icon.
    // The tray is NOT defined in tauri.conf.json — it is created here so that
    // creation happens after the event loop is initialized (RunEvent::Ready),
    // avoiding the ghost/duplicate icon timing bug on macOS (tauri#9480).
    let tray = match app.tray_by_id("main-tray") {
        Some(tray) => tray,
        None => {
            log::info!("Creating tray icon programmatically");
            let icon = Image::from_bytes(TRAY_ICON_NORMAL)
                .map_err(|e| format!("Failed to load tray icon: {}", e))?;
            TrayIconBuilder::with_id("main-tray")
                .tooltip("SottoASR \u{2014} Speech to Text")
                .icon(icon)
                .icon_as_template(true)
                .show_menu_on_left_click(true)
                .build(app)
                .map_err(|e| format!("Failed to create tray icon: {}", e))?
        }
    };

    // Register event handler BEFORE setting the menu to avoid a race where
    // the first click arrives before the handler is wired up (fixes
    // first-right-click-ignored bug on macOS).
    tray.on_menu_event(move |app, event| {
        match event.id().as_ref() {
            "copy_last" => {
                log::info!("Tray: Copy last transcription");
                tauri::async_runtime::spawn({
                    let app = app.clone();
                    async move {
                        let state: tauri::State<'_, crate::state::AppState> = app.state();
                        let text = state.last_transcription.lock().await.as_ref().map(|item| item.text.clone());
                        if let Some(text) = text {
                            let length = text.chars().count();
                            match tokio::task::spawn_blocking(move || crate::paste::copy_to_clipboard(&text)).await {
                                Ok(Ok(())) => log::info!("Copied last transcription ({length} characters)"),
                                Ok(Err(error)) => log::error!("Failed to copy to clipboard: {error}"),
                                Err(error) => log::error!("Clipboard task failed: {error}"),
                            }
                        } else {
                            log::info!("No transcription to copy");
                        }
                    }
                });
            }
            "view_history" => {
                log::info!("Tray: Opening history window");
                open_or_focus_window(
                    app, "history", "history.html", "SottoASR \u{2014} History", 520.0, 640.0,
                );
            }
            "settings" => {
                log::info!("Tray: Opening settings window");
                open_or_focus_window(
                    app, "settings", "settings.html", "SottoASR \u{2014} Settings", 520.0, 600.0,
                );
            }
            "check_updates" => {
                log::info!("Tray: Opening update window");
                // Use context-appropriate title based on current tray state.
                // We can't access TrayState here directly (it's dropped after build_tray_menu),
                // so we read UpdateState to determine the title.
                let title = if let Some(us) = app.try_state::<crate::updater::UpdateState>() {
                    let app_update = us.update_available.load(std::sync::atomic::Ordering::SeqCst);
                    let model_update = us.model_update_available.load(std::sync::atomic::Ordering::SeqCst);
                    let restart = us.restart_pending.load(std::sync::atomic::Ordering::SeqCst);
                    if model_update && !app_update && !restart {
                        "SottoASR \u{2014} Model Update".to_string()
                    } else {
                        "SottoASR \u{2014} Software Update".to_string()
                    }
                } else {
                    "SottoASR \u{2014} Update".to_string()
                };
                open_or_focus_window(
                    app,
                    "update",
                    "update.html",
                    &title,
                    420.0,
                    480.0,
                );
            }
            "copy_diagnostics" => {
                log::info!("Tray: Copy diagnostics");
                let app = app.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    let diagnostics = collect_diagnostics(&app);
                    match crate::paste::copy_to_clipboard(&diagnostics) {
                        Ok(()) => log::info!(
                            "Diagnostics copied to clipboard ({} bytes)",
                            diagnostics.len()
                        ),
                        Err(e) => log::error!("Failed to copy diagnostics to clipboard: {}", e),
                    }
                });
            }
            "about" => {
                log::info!("Tray: Opening about window");
                open_or_focus_window(
                    app, "about", "about.html", "About SottoASR", 480.0, 960.0,
                );
            }
            "quit" => {
                log::info!("Quitting SottoASR");
                app.exit(0);
            }
            _ => {}
        }
    });

    tray.set_menu(Some(menu)).map_err(|e| e.to_string())?;

    log::info!("Tray menu configured");
    Ok(())
}

// ---------------------------------------------------------------------------
// Icon switching
// ---------------------------------------------------------------------------

fn set_tray_icon(app: &AppHandle, has_update: bool) {
    let icon_bytes = if has_update {
        TRAY_ICON_UPDATE
    } else {
        TRAY_ICON_NORMAL
    };
    if let Some(tray) = app.tray_by_id("main-tray") {
        match Image::from_bytes(icon_bytes) {
            Ok(icon) => {
                let _ = tray.set_icon(Some(icon));
                let _ = tray.set_icon_as_template(true);
            }
            Err(e) => log::error!("Failed to load tray icon: {}", e),
        }
    }
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

/// Collect diagnostic information: app version, macOS version, timestamp, and recent log lines.
fn collect_diagnostics(app: &AppHandle) -> String {
    let version = app.package_info().version.to_string();

    let macos_version = get_macos_version();

    let timestamp = chrono::Local::now()
        .format("%Y-%m-%d %H:%M:%S %z")
        .to_string();

    let log_tail = read_log_tail(app, 100);

    format!(
        "SottoASR Diagnostics\nVersion: {}\nmacOS: {}\nDate: {}\n---\n{}",
        version, macos_version, timestamp, log_tail
    )
}

/// Get macOS version string via `sw_vers`.
fn get_macos_version() -> String {
    match crate::process::bounded_command(
        std::process::Command::new("/usr/bin/sw_vers").arg("-productVersion"),
        std::time::Duration::from_secs(2),
    ) {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        _ => "unknown".to_string(),
    }
}

/// Read the last `n` lines from the app log file.
/// Uses the Tauri log directory to find the log file.
fn read_log_tail(app: &AppHandle, n: usize) -> String {
    let log_dir = match app.path().app_log_dir() {
        Ok(dir) => dir,
        Err(e) => {
            return format!("[Could not determine log directory: {}]", e);
        }
    };

    // tauri-plugin-log names the file based on the productName in tauri.conf.json
    // with a .log extension. Try the known filename first, then fall back to scanning.
    let log_path = log_dir.join("SottoASR.log");
    let log_path = if log_path.exists() {
        log_path
    } else {
        // Fallback: try the configured name from the plugin
        let alt = log_dir.join("sottoasr.log");
        if alt.exists() {
            alt
        } else {
            return format!("[Log file not found in {}]", log_dir.display());
        }
    };

    match std::fs::File::open(&log_path) {
        Ok(mut file) => bounded_log_tail(&mut file, n)
            .unwrap_or_else(|error| format!("[Could not read log tail: {error}]")),
        Err(e) => {
            format!("[Could not read log file {}: {}]", log_path.display(), e)
        }
    }
}

/// Bound disk I/O and memory even when a log has grown for months. Skip the
/// first partial line when seeking into the file; lossy decoding tolerates a
/// trailing write in progress without exposing an incomplete leading line.
fn bounded_log_tail(reader: &mut (impl Read + Seek), n: usize) -> std::io::Result<String> {
    const MAX_BYTES: u64 = 64 * 1024;
    let length = reader.seek(SeekFrom::End(0))?;
    let offset = length.saturating_sub(MAX_BYTES);
    reader.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::with_capacity(length.min(MAX_BYTES) as usize);
    reader.take(MAX_BYTES).read_to_end(&mut bytes)?;
    let bytes = if offset > 0 {
        bytes.iter().position(|byte| *byte == b'\n')
            .map_or(&[][..], |newline| &bytes[newline + 1..])
    } else {
        &bytes[..]
    };
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<_> = text.lines().collect();
    Ok(lines[lines.len().saturating_sub(n)..].join("\n"))
}

#[cfg(test)]
mod diagnostics_tests {
    use super::bounded_log_tail;
    use std::io::Cursor;

    #[test]
    fn large_log_tail_keeps_recent_unicode_lines_with_bounded_reads() {
        let mut bytes = vec![b'x'; 1024 * 1024];
        bytes.extend_from_slice("\nold\nready café\nlast line\n".as_bytes());
        let mut reader = Cursor::new(bytes);
        assert_eq!(bounded_log_tail(&mut reader, 2).unwrap(), "ready café\nlast line");
        assert_eq!(bounded_log_tail(&mut reader, 0).unwrap(), "");
        assert_eq!(bounded_log_tail(&mut Cursor::new(vec![b'x'; 100_000]), 100).unwrap(), "");
    }

    #[test]
    fn short_and_empty_logs_do_not_lose_the_first_line() {
        assert_eq!(bounded_log_tail(&mut Cursor::new(b"first\nsecond"), 100).unwrap(), "first\nsecond");
        assert_eq!(bounded_log_tail(&mut Cursor::new(b""), 100).unwrap(), "");
    }
}

// ---------------------------------------------------------------------------
// Window management
// ---------------------------------------------------------------------------

/// Open a window by label, or focus it if already open.
/// Accessory apps can show and focus windows without adding a Dock icon.
pub fn open_or_focus_window(
    app: &AppHandle,
    label: &str,
    url: &str,
    title: &str,
    width: f64,
    height: f64,
) {
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        log::info!("Focused existing {} window", label);
    } else {
        match tauri::webview::WebviewWindowBuilder::new(
            app,
            label,
            WebviewUrl::App(url.into()),
        )
        .title(title)
        .inner_size(width, height)
        .resizable(true)
        .center()
        .focused(true)
        .build()
        {
            Ok(window) => {
                let _ = window.set_focus();
                log::info!("Created and focused {} window", label);
            }
            Err(e) => log::error!("Failed to open {} window: {}", label, e),
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn tray_snapshot_waits_without_blocking_the_async_updater() {
        let state = std::sync::Arc::new(crate::updater::UpdateState::new());
        let mut version = state.available_version.lock().await;
        let reader_state = state.clone();
        let reader = tokio::spawn(async move { read_tray_state(&reader_state).await });
        tokio::task::yield_now().await;
        assert!(!reader.is_finished());
        *version = Some("0.9.0".into());
        state.update_available.store(true, std::sync::atomic::Ordering::SeqCst);
        drop(version);
        assert_eq!(reader.await.unwrap(), (true, TrayState::UpdateAvailable("0.9.0".into())));
    }
}
