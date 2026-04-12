#![deny(warnings)]

mod models;
mod state;
mod commands;
mod audio;
mod asr;
mod llm;
mod paste;
mod hotkeys;
mod tray;
mod updater;
pub mod pipeline;
#[cfg(test)]
mod test_support;

use state::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_log::Builder::default()
            .level(log::LevelFilter::Info)
            .targets([
                tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::LogDir { file_name: None },
                ),
                tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::Stdout,
                ),
            ])
            .build())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_positioner::init())
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .plugin(tauri_nspanel::init())
        .manage(AppState::new())
        .manage(updater::UpdateState::new())
        .invoke_handler(tauri::generate_handler![
            // Recording
            commands::recording::start_recording,
            commands::recording::stop_recording,
            commands::recording::cancel_recording,
            // Transcription history
            commands::transcription::get_transcriptions,
            commands::transcription::get_last_transcription,
            commands::transcription::delete_transcription,
            commands::transcription::clear_transcriptions,
            commands::transcription::export_transcriptions_csv,
            // Settings
            commands::settings::get_settings,
            commands::settings::update_settings,
            commands::settings::apply_shortcuts,
            // Permissions
            commands::permissions::check_microphone_permission,
            commands::permissions::check_accessibility_permission,
            commands::permissions::request_accessibility_permission,
            commands::permissions::request_microphone_permission,
            commands::permissions::check_all_permissions,
            commands::permissions::open_accessibility_settings,
            commands::permissions::fix_accessibility_permission,
            commands::permissions::open_input_monitoring_settings,
            commands::permissions::open_microphone_settings,
            commands::permissions::open_url,
            // Key capture (for shortcut recorder)
            commands::keycapture::start_key_capture,
            commands::keycapture::stop_key_capture,
            // Setup / onboarding
            commands::setup::get_asr_backend,
            commands::setup::get_model_status,
            commands::setup::needs_onboarding,
            commands::setup::init_asr,
            commands::setup::download_model,
            commands::setup::complete_setup,
            // LLM transcript cleanup
            commands::llm::get_llm_status,
            commands::llm::check_llm_update,
            commands::llm::download_llm_model,
            commands::llm::update_llm_model,
            commands::llm::cancel_llm_download,
            commands::llm::delete_llm_model,
            commands::llm::load_llm_model,
            commands::llm::unload_llm_model,
            // App updater
            updater::check_app_update,
            updater::perform_app_update,
            updater::get_update_status,
            // Overlay drag
            commands::overlay::overlay_start_drag,
        ])
        .setup(|app| {
            // Register the updater plugin (must use Builder pattern inside setup)
            #[cfg(desktop)]
            app.handle().plugin(tauri_plugin_updater::Builder::new().build())?;

            let handle = app.handle().clone();

            // Hide from Dock — menu bar only app.
            // LSUIElement in Info.plist should handle this, but we also set it
            // explicitly in case windows cause macOS to show the Dock icon.
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
                log::info!("Set activation policy to Accessory (no Dock icon)");
            }

            // Check Accessibility permission at launch (don't prompt — onboarding handles that).
            #[cfg(target_os = "macos")]
            {
                let ax_trusted = paste::is_accessibility_trusted();
                if !ax_trusted {
                    log::warn!("Accessibility permission NOT granted — hotkeys and paste will not work!");
                } else {
                    log::info!("Accessibility permission granted (AXIsProcessTrusted)");
                    // One-time functional verification: check the AX API actually works.
                    // This catches the Sequoia bug where TCC says "granted" but the
                    // permission hasn't propagated yet. Log a warning but don't block —
                    // it may settle by the time the user records.
                    if paste::test_accessibility_functional() {
                        log::info!("Accessibility functional check passed");
                    } else {
                        log::warn!(
                            "Accessibility functional check FAILED — permission may not have \
                             propagated yet. Paste may not work until app restart."
                        );
                    }
                    paste::warmup_cgevent_pipeline();
                }
            }

            // Start CGEventTap thread for key capture (used by shortcut recorder)
            // Must start after accessibility check — needs the permission to create taps
            commands::keycapture::init_key_capture_thread(&handle);

            // NOTE: Tray icon creation is deferred to RunEvent::Ready (below)
            // so that the NSStatusItem is created after the event loop is fully
            // initialized, avoiding the ghost/duplicate icon timing bug (tauri#9480).

            // Setup hotkeys
            hotkeys::manager::setup_hotkeys(&handle)
                .map_err(|e| Box::new(std::io::Error::other(e)))?;

            // Pre-create the overlay panel (hidden) so that the first recording
            // doesn't steal focus. WebviewWindow creation activates the app on
            // macOS, but this is harmless at startup before the user is interacting.
            hotkeys::manager::precreate_overlay(&handle);

            // Check if onboarding is needed and open the setup window
            let needs_setup = !asr::model::is_model_available();
            if needs_setup {
                log::info!("First launch detected — opening onboarding window");
                // Create the onboarding window
                let _onboarding = tauri::WebviewWindowBuilder::new(
                    &handle,
                    "onboarding",
                    tauri::WebviewUrl::App("onboarding.html".into()),
                )
                .title("Welcome to SottoASR")
                .inner_size(520.0, 600.0)
                .resizable(false)
                .center()
                .build()
                .map_err(|e| Box::new(std::io::Error::other(e.to_string())))?;
            } else {
                log::info!("Models available — ready to use");
                // Initialize ASR in background
                let asr_handle = handle.clone();
                tauri::async_runtime::spawn(async move {
                    let state: tauri::State<'_, AppState> = asr_handle.state();
                    let mut engine = state.asr_engine.lock().await;
                    if let Err(e) = engine.init() {
                        log::error!("Background ASR init failed: {}", e);
                    } else {
                        state.is_model_loaded.store(true, std::sync::atomic::Ordering::SeqCst);
                        log::info!("ASR engine ready");
                    }
                });

                // Pre-load LLM sidecar in background (if enabled and model is downloaded)
                if llm::engine::is_feature_compiled() {
                    let llm_handle = handle.clone();
                    tauri::async_runtime::spawn(async move {
                        let state: tauri::State<'_, AppState> = llm_handle.state();
                        let enabled = state.settings.lock().await.llm_cleanup_enabled;

                        if !enabled
                            || !llm::engine::is_venv_ready()
                            || !llm::engine::is_model_downloaded()
                        {
                            return;
                        }

                        log::info!("Pre-loading LLM sidecar in background...");
                        match tokio::task::spawn_blocking(|| {
                            let mut e = llm::engine::LlmEngine::spawn()?;
                            e.load_model()?;
                            Ok::<_, String>(e)
                        })
                        .await
                        {
                            Ok(Ok(engine)) => {
                                let mut guard = state.llm_engine.lock().await;
                                *guard = Some(Box::new(engine) as Box<dyn llm::engine::LlmBackend>);
                                log::info!("LLM sidecar pre-loaded and ready");
                            }
                            Ok(Err(e)) => {
                                log::warn!("LLM pre-load failed: {}", e);
                            }
                            Err(e) => {
                                log::error!("LLM pre-load panicked: {}", e);
                            }
                        }
                    });
                }
            }

            // Start the auto-update checker (15s delay, then every 4 hours)
            updater::start_update_checker(&handle);

            // Sync launch-at-login state from persisted settings
            {
                use tauri_plugin_autostart::ManagerExt;
                let launch_at_login = handle.state::<AppState>().settings.blocking_lock().launch_at_login;
                let manager = handle.autolaunch();
                let result = if launch_at_login {
                    manager.enable()
                } else {
                    manager.disable()
                };
                match result {
                    Ok(_) => log::info!("Autostart synced (enabled={})", launch_at_login),
                    Err(e) => log::error!("Failed to sync autostart state: {}", e),
                }
            }

            log::info!("SottoASR initialized (ASR backend: {})", asr::model::backend_name());
            Ok(())
        })
        .on_window_event(|window, event| {
            // When a user-visible window (history, settings, onboarding) is closed,
            // check if any visible windows remain. If not, switch back to Accessory
            // so the Dock icon disappears.
            if let tauri::WindowEvent::Destroyed = event {
                let label = window.label();
                // Don't care about overlay closing
                if label == "overlay" {
                    return;
                }
                log::info!("Window '{}' closed", label);

                let app = window.app_handle();
                // Check if any non-overlay windows are still open
                let has_visible_windows = app.webview_windows()
                    .iter()
                    .any(|(l, w)| {
                        l.as_str() != "overlay" && w.is_visible().unwrap_or(false)
                    });

                if !has_visible_windows {
                    #[cfg(target_os = "macos")]
                    {
                        let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
                        log::info!("All windows closed — switched back to Accessory (no Dock icon)");
                    }
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building SottoASR")
        .run(|app, event| {
            match event {
                tauri::RunEvent::Ready => {
                    // Create the tray icon NOW — after the event loop is fully
                    // initialized. Creating it earlier (in setup() or via
                    // tauri.conf.json) can cause ghost/duplicate icons on macOS
                    // when other apps are fullscreen (tauri#9480).
                    if let Err(e) = tray::menu::setup_tray_menu(app) {
                        log::error!("Failed to setup tray menu: {}", e);
                    }

                    // Start tray icon occlusion monitor (detects when icon is
                    // hidden behind the notch or by menu bar overflow).
                    #[cfg(target_os = "macos")]
                    tray::occlusion::start_occlusion_monitor(app);
                }
                tauri::RunEvent::ExitRequested { api, code, .. } => {
                    // Only prevent exit when triggered by last window closing (code == None).
                    // Allow explicit exit via app.exit() (code == Some(0)).
                    if code.is_none() {
                        api.prevent_exit();
                    }
                }
                _ => {}
            }
        });
}
