use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager, WebviewUrl,
};

use std::io::{BufRead, BufReader};

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
pub fn refresh_tray_from_state(app: &AppHandle) {
    let state = match app.try_state::<crate::updater::UpdateState>() {
        Some(s) => s,
        None => return,
    };

    let app_update = state.update_available.load(std::sync::atomic::Ordering::SeqCst);
    let model_update = state.model_update_available.load(std::sync::atomic::Ordering::SeqCst);
    let restart = state.restart_pending.load(std::sync::atomic::Ordering::SeqCst);
    let version = state.available_version.blocking_lock().clone();

    // Icon: any update → show indicator.
    let has_any_update = app_update || model_update || restart;
    set_tray_icon(app, has_any_update);

    // Menu priority: restart > app update > model update > normal.
    let tray_state = if restart {
        TrayState::RestartPending
    } else if let Some(v) = version {
        let label = if model_update {
            format!("{} (+ model)", v)
        } else {
            v
        };
        TrayState::UpdateAvailable(label)
    } else if model_update {
        TrayState::ModelUpdateAvailable
    } else {
        TrayState::Normal
    };
    if let Err(e) = build_tray_menu(app, tray_state) {
        log::error!("Failed to rebuild tray menu: {}", e);
    }
}

// ---------------------------------------------------------------------------
// Internal: tray state enum
// ---------------------------------------------------------------------------

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
                        let last = state.last_transcription.lock().await;
                        if let Some(t) = last.as_ref() {
                            match crate::paste::copy_to_clipboard(&t.text) {
                                Ok(()) => log::info!(
                                    "Copied to clipboard: \"{}\"",
                                    &t.text[..t.text.len().min(50)]
                                ),
                                Err(e) => log::error!("Failed to copy to clipboard: {}", e),
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
                let diagnostics = collect_diagnostics(app);
                match crate::paste::copy_to_clipboard(&diagnostics) {
                    Ok(()) => log::info!(
                        "Diagnostics copied to clipboard ({} bytes)",
                        diagnostics.len()
                    ),
                    Err(e) => log::error!("Failed to copy diagnostics to clipboard: {}", e),
                }
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
    match std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
    {
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
        Ok(file) => {
            let reader = BufReader::new(file);
            let lines: Vec<String> = reader.lines().map_while(Result::ok).collect();
            let start = lines.len().saturating_sub(n);
            lines[start..].join("\n")
        }
        Err(e) => {
            format!("[Could not read log file {}: {}]", log_path.display(), e)
        }
    }
}

// ---------------------------------------------------------------------------
// Window management
// ---------------------------------------------------------------------------

/// Open a window by label, or focus it if already open.
/// Switches to Regular activation policy so macOS shows the window.
/// Reverts to Accessory when the window is closed (handled in lib.rs on_window_event).
pub fn open_or_focus_window(
    app: &AppHandle,
    label: &str,
    url: &str,
    title: &str,
    width: f64,
    height: f64,
) {
    // Switch to Regular so macOS allows us to show windows and the window appears in front
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);

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
