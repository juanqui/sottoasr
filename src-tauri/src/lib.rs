mod models;
mod state;
mod commands;
mod audio;
mod asr;
mod paste;
mod hotkeys;
mod tray;

use state::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_log::Builder::default()
            .level(log::LevelFilter::Info)
            .target(tauri_plugin_log::Target::new(
                tauri_plugin_log::TargetKind::LogDir { file_name: Some("sotto".into()) },
            ))
            .target(tauri_plugin_log::Target::new(
                tauri_plugin_log::TargetKind::Stdout,
            ))
            .build())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_positioner::init())
        .manage(AppState::new())
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
            // Settings
            commands::settings::get_settings,
            commands::settings::update_settings,
            // Permissions
            commands::permissions::check_microphone_permission,
            commands::permissions::check_accessibility_permission,
            commands::permissions::request_accessibility_permission,
            // Setup / onboarding
            commands::setup::get_asr_backend,
            commands::setup::get_model_status,
            commands::setup::needs_onboarding,
            commands::setup::init_asr,
            commands::setup::download_model,
            commands::setup::complete_setup,
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            // Hide from Dock — menu bar only app.
            // LSUIElement in Info.plist should handle this, but we also set it
            // explicitly in case windows cause macOS to show the Dock icon.
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
                log::info!("Set activation policy to Accessory (no Dock icon)");
            }

            // Prompt for Accessibility permission at launch.
            // Global shortcuts SILENTLY FAIL without it on macOS.
            #[cfg(target_os = "macos")]
            {
                let ax_trusted = unsafe {
                    extern "C" {
                        fn AXIsProcessTrusted() -> bool;
                    }
                    AXIsProcessTrusted()
                };
                if !ax_trusted {
                    log::warn!("Accessibility permission NOT granted — hotkeys will not work!");
                    log::info!("Requesting Accessibility permission via system prompt...");
                    // Trigger the macOS prompt that adds this app to the Accessibility list
                    unsafe {
                        extern "C" {
                            fn CFStringCreateWithCString(
                                alloc: *const std::ffi::c_void,
                                cstr: *const std::ffi::c_char,
                                encoding: u32,
                            ) -> *const std::ffi::c_void;
                            fn CFDictionaryCreate(
                                alloc: *const std::ffi::c_void,
                                keys: *const *const std::ffi::c_void,
                                values: *const *const std::ffi::c_void,
                                count: isize,
                                key_callbacks: *const std::ffi::c_void,
                                value_callbacks: *const std::ffi::c_void,
                            ) -> *const std::ffi::c_void;
                            fn AXIsProcessTrustedWithOptions(
                                options: *const std::ffi::c_void,
                            ) -> bool;
                            fn CFRelease(cf: *const std::ffi::c_void);
                            static kCFBooleanTrue: *const std::ffi::c_void;
                            static kCFTypeDictionaryKeyCallBacks: std::ffi::c_void;
                            static kCFTypeDictionaryValueCallBacks: std::ffi::c_void;
                        }
                        let key_cstr = b"AXTrustedCheckOptionPrompt\0".as_ptr() as *const std::ffi::c_char;
                        let key = CFStringCreateWithCString(std::ptr::null(), key_cstr, 0x08000100);
                        let keys = [key];
                        let values = [kCFBooleanTrue];
                        let options = CFDictionaryCreate(
                            std::ptr::null(), keys.as_ptr(), values.as_ptr(), 1,
                            &kCFTypeDictionaryKeyCallBacks as *const _ as *const std::ffi::c_void,
                            &kCFTypeDictionaryValueCallBacks as *const _ as *const std::ffi::c_void,
                        );
                        let _ = AXIsProcessTrustedWithOptions(options);
                        CFRelease(options);
                        CFRelease(key);
                    }
                } else {
                    log::info!("Accessibility permission granted");
                }
            }

            // Setup tray menu
            tray::menu::setup_tray_menu(&handle)
                .map_err(|e| Box::new(std::io::Error::new(std::io::ErrorKind::Other, e)))?;

            // Setup hotkeys
            hotkeys::manager::setup_hotkeys(&handle)
                .map_err(|e| Box::new(std::io::Error::new(std::io::ErrorKind::Other, e)))?;

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
                .title("Welcome to Sotto")
                .inner_size(520.0, 600.0)
                .resizable(false)
                .center()
                .build()
                .map_err(|e| Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
            } else {
                log::info!("Models available — ready to use");
                // Initialize ASR in background
                tauri::async_runtime::spawn(async move {
                    let state: tauri::State<'_, AppState> = handle.state();
                    let mut engine = state.asr_engine.lock().await;
                    if let Err(e) = engine.init() {
                        log::error!("Background ASR init failed: {}", e);
                    } else {
                        state.is_model_loaded.store(true, std::sync::atomic::Ordering::SeqCst);
                        log::info!("ASR engine ready");
                    }
                });
            }

            log::info!("Sotto initialized (ASR backend: {})", asr::model::backend_name());
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
                        app.set_activation_policy(tauri::ActivationPolicy::Accessory);
                        log::info!("All windows closed — switched back to Accessory (no Dock icon)");
                    }
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building Sotto")
        .run(|_app, event| {
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                api.prevent_exit();
            }
        });
}
