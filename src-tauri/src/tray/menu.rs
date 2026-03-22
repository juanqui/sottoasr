use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager, WebviewUrl,
};

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
