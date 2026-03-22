use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager, WebviewUrl,
};

use std::io::{BufRead, BufReader};

pub fn setup_tray_menu(app: &AppHandle) -> Result<(), String> {
    let copy_last = MenuItem::with_id(app, "copy_last", "Copy Last Transcription", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let view_history = MenuItem::with_id(app, "view_history", "View Transcription History", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let settings = MenuItem::with_id(app, "settings", "Settings...", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let separator = PredefinedMenuItem::separator(app)
        .map_err(|e| e.to_string())?;
    let separator2 = PredefinedMenuItem::separator(app)
        .map_err(|e| e.to_string())?;
    let copy_diagnostics = MenuItem::with_id(app, "copy_diagnostics", "Copy Diagnostics", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let about = MenuItem::with_id(app, "about", "About SottoASR", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let quit = MenuItem::with_id(app, "quit", "Quit SottoASR", true, None::<&str>)
        .map_err(|e| e.to_string())?;

    let menu = Menu::with_items(app, &[
        &copy_last,
        &view_history,
        &separator,
        &settings,
        &separator2,
        &copy_diagnostics,
        &about,
        &quit,
    ])
    .map_err(|e| e.to_string())?;

    // Get the tray icon defined in tauri.conf.json
    let tray = match app.tray_by_id("main-tray") {
        Some(tray) => {
            log::info!("Found existing tray icon 'main-tray'");
            tray
        }
        None => {
            log::info!("Creating tray icon programmatically");
            TrayIconBuilder::with_id("main-tray")
                .tooltip("SottoASR — Speech to Text")
                .icon(app.default_window_icon().cloned().unwrap_or_else(|| {
                    tauri::image::Image::new(&[], 1, 1)
                }))
                .icon_as_template(false)
                .show_menu_on_left_click(true)
                .build(app)
                .map_err(|e| format!("Failed to create tray icon: {}", e))?
        }
    };

    tray.set_menu(Some(menu)).map_err(|e| e.to_string())?;
    tray.set_show_menu_on_left_click(true).map_err(|e| e.to_string())?;

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
                                Ok(()) => log::info!("Copied to clipboard: \"{}\"", &t.text[..t.text.len().min(50)]),
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
                open_or_focus_window(app, "history", "history.html", "SottoASR — History", 520.0, 640.0);
            }
            "settings" => {
                log::info!("Tray: Opening settings window");
                open_or_focus_window(app, "settings", "settings.html", "SottoASR — Settings", 520.0, 600.0);
            }
            "copy_diagnostics" => {
                log::info!("Tray: Copy diagnostics");
                let diagnostics = collect_diagnostics(app);
                match crate::paste::copy_to_clipboard(&diagnostics) {
                    Ok(()) => log::info!("Diagnostics copied to clipboard ({} bytes)", diagnostics.len()),
                    Err(e) => log::error!("Failed to copy diagnostics to clipboard: {}", e),
                }
            }
            "about" => {
                log::info!("Tray: Opening about window");
                open_or_focus_window(app, "about", "about.html", "About SottoASR", 480.0, 960.0);
            }
            "quit" => {
                log::info!("Quitting SottoASR");
                app.exit(0);
            }
            _ => {}
        }
    });

    log::info!("Tray menu configured");
    Ok(())
}

/// Collect diagnostic information: app version, macOS version, timestamp, and recent log lines.
fn collect_diagnostics(app: &AppHandle) -> String {
    let version = app.package_info().version.to_string();

    let macos_version = get_macos_version();

    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S %z").to_string();

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

/// Open a window by label, or focus it if already open.
/// Switches to Regular activation policy so macOS shows the window.
/// Reverts to Accessory when the window is closed (handled in lib.rs on_window_event).
fn open_or_focus_window(app: &AppHandle, label: &str, url: &str, title: &str, width: f64, height: f64) {
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
