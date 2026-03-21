use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Manager};
use crate::models::AppStateEnum;
use crate::state::AppState;

pub fn setup_hotkeys(app: &AppHandle) -> Result<(), String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    let app_handle = app.clone();

    // Push-to-talk: Cmd+Shift+Space (hold to record, release to transcribe)
    app.global_shortcut().on_shortcut("CommandOrControl+Shift+Space", move |_app, _shortcut, event| {
        let app = app_handle.clone();
        match event.state {
            ShortcutState::Pressed => {
                tauri::async_runtime::spawn(async move {
                    handle_start_recording(&app);
                });
            }
            ShortcutState::Released => {
                tauri::async_runtime::spawn(async move {
                    handle_stop_recording(&app).await;
                });
            }
        }
    }).map_err(|e| format!("Failed to register push-to-talk shortcut: {}", e))?;

    let app_handle2 = app.clone();

    // Toggle mode: Cmd+Shift+D (press once to start, again to stop)
    app.global_shortcut().on_shortcut("CommandOrControl+Shift+D", move |_app, _shortcut, event| {
        if event.state == ShortcutState::Pressed {
            let app = app_handle2.clone();
            tauri::async_runtime::spawn(async move {
                let state: tauri::State<'_, AppState> = app.state();
                let current = state.get_state();
                match current {
                    AppStateEnum::Idle => {
                        handle_start_recording(&app);
                    }
                    AppStateEnum::Recording => {
                        handle_stop_recording(&app).await;
                    }
                    _ => {
                        log::info!("Toggle: ignoring (state={:?})", current);
                    }
                }
            });
        }
    }).map_err(|e| format!("Failed to register toggle shortcut: {}", e))?;

    log::info!("Hotkeys registered: Cmd+Shift+Space (push-to-talk), Cmd+Shift+D (toggle)");
    Ok(())
}

/// Start recording: open microphone, set state, emit events.
fn handle_start_recording(app: &AppHandle) {
    let state: tauri::State<'_, AppState> = app.state();
    let current = state.get_state();

    if current != AppStateEnum::Idle {
        log::warn!("Cannot start recording: currently in {:?} state", current);
        return;
    }

    // Drain any leftover samples from previous recording
    if let Ok(rx) = state.audio_receiver.lock() {
        while rx.try_recv().is_ok() {}
    }

    // Start audio capture
    let sender = {
        let s = state.audio_sender.lock().unwrap();
        s.clone()
    };
    let is_recording = Arc::new(AtomicBool::new(true));
    state.is_recording.store(true, Ordering::SeqCst);

    let is_recording_clone = is_recording.clone();
    let app_clone = app.clone();

    let start_result = {
        let mut capture = state.audio_capture.lock().unwrap();
        capture.start(
            sender,
            is_recording_clone,
            Box::new(move |level| {
                let _ = app_clone.emit("audio-level", serde_json::json!({ "level": level }));
            }),
        )
    };

    match start_result {
        Ok(()) => {
            state.set_state(AppStateEnum::Recording);
            let _ = app.emit("recording-started", ());
            let _ = app.emit("state-changed", &AppStateEnum::Recording);

            // Show the overlay window
            show_overlay(app);

            log::info!("Recording started — microphone active");
        }
        Err(e) => {
            log::error!("Failed to start audio capture: {}", e);
            state.is_recording.store(false, Ordering::SeqCst);
            let _ = app.emit("recording-error", serde_json::json!({ "error": e }));
        }
    }
}

/// Stop recording: close microphone, collect samples, transcribe, paste.
async fn handle_stop_recording(app: &AppHandle) {
    let state: tauri::State<'_, AppState> = app.state();
    let current = state.get_state();

    if current != AppStateEnum::Recording {
        log::warn!("Cannot stop recording: currently in {:?} state", current);
        return;
    }

    // Stop audio capture
    state.is_recording.store(false, Ordering::SeqCst);
    {
        let mut capture = state.audio_capture.lock().unwrap();
        capture.stop();
    }

    // Hide the overlay
    hide_overlay(app);

    state.set_state(AppStateEnum::Transcribing);
    let _ = app.emit("recording-stopped", ());
    let _ = app.emit("state-changed", &AppStateEnum::Transcribing);

    // Collect all audio samples from the channel
    let samples = {
        let rx = state.audio_receiver.lock().unwrap();
        let mut all = Vec::new();
        while let Ok(chunk) = rx.try_recv() {
            all.extend(chunk);
        }
        all
    };

    log::info!("Collected {} audio samples ({:.1}s at estimated rate)",
        samples.len(),
        samples.len() as f64 / 48000.0 // approximate — actual rate may vary
    );

    if samples.len() < 4000 {
        log::warn!("Recording too short ({} samples), discarding", samples.len());
        state.set_state(AppStateEnum::Idle);
        let _ = app.emit("state-changed", &AppStateEnum::Idle);
        return;
    }

    // Write samples to a temp WAV file for FluidAudio (which requires file-based input)
    let temp_path = std::env::temp_dir().join(format!("sotto_{}.wav", uuid::Uuid::new_v4()));

    // We captured at the device's native rate (likely 48kHz mono after downmix).
    // FluidAudio handles resampling internally, so just write at the capture rate.
    let sample_rate = {
        // Get the actual sample rate from cpal
        use cpal::traits::{HostTrait, DeviceTrait};
        let host = cpal::default_host();
        host.default_input_device()
            .and_then(|d| d.default_input_config().ok())
            .map(|c| c.sample_rate().0)
            .unwrap_or(48000)
    };

    let wav_spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };

    match hound::WavWriter::create(&temp_path, wav_spec) {
        Ok(mut writer) => {
            for &sample in &samples {
                let _ = writer.write_sample(sample);
            }
            let _ = writer.finalize();
            log::info!("Wrote temp WAV: {:?} ({} samples, {} Hz)", temp_path, samples.len(), sample_rate);
        }
        Err(e) => {
            log::error!("Failed to write temp WAV: {}", e);
            state.set_state(AppStateEnum::Idle);
            let _ = app.emit("state-changed", &AppStateEnum::Idle);
            let _ = app.emit("transcription-error", serde_json::json!({ "error": format!("WAV write failed: {}", e) }));
            return;
        }
    }

    // Transcribe
    let app_clone = app.clone();
    let temp_path_str = temp_path.to_string_lossy().to_string();

    tokio::spawn(async move {
        let state: tauri::State<'_, AppState> = app_clone.state();
        let mut engine = state.asr_engine.lock().await;

        log::info!("Starting transcription...");

        let result = engine.transcribe_file(&temp_path_str);

        // Clean up temp file
        let _ = std::fs::remove_file(&temp_path);

        match result {
            Ok(asr_result) => {
                log::info!("Transcription result: \"{}\" (RTF: {:.1}x)", &asr_result.text, asr_result.rtfx);

                let transcription = crate::models::Transcription {
                    id: uuid::Uuid::new_v4().to_string(),
                    text: asr_result.text.clone(),
                    duration_ms: (asr_result.duration_secs * 1000.0) as u64,
                    created_at: chrono::Utc::now(),
                    word_count: asr_result.text.split_whitespace().count(),
                };

                // Save
                {
                    let mut last = state.last_transcription.lock().await;
                    *last = Some(transcription.clone());
                }
                crate::commands::transcription::add_transcription(transcription.clone()).await;

                let _ = app_clone.emit("transcription-complete", &transcription);

                // Paste at cursor
                if !asr_result.text.trim().is_empty() {
                    match crate::paste::paste_text(&asr_result.text) {
                        Ok(()) => {
                            log::info!("Text pasted at cursor");
                            let _ = app_clone.emit("paste-complete", serde_json::json!({ "id": &transcription.id }));
                        }
                        Err(e) => {
                            log::error!("Paste failed: {}", e);
                            let _ = app_clone.emit("paste-error", serde_json::json!({ "error": e }));
                        }
                    }
                }
            }
            Err(e) => {
                log::error!("Transcription failed: {}", e);
                let _ = app_clone.emit("transcription-error", serde_json::json!({ "error": e }));
            }
        }

        state.set_state(AppStateEnum::Idle);
        let _ = app_clone.emit("state-changed", &AppStateEnum::Idle);
    });

    log::info!("Recording stopped, transcription queued");
}

/// Show the floating overlay pill window. Creates it if it doesn't exist.
fn show_overlay(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("overlay") {
        let _ = window.show();
        let _ = window.set_always_on_top(true);
        position_overlay_bottom_center(&window);
        log::info!("Overlay shown (existing window)");
    } else {
        // Create the overlay window — use non-transparent for reliability,
        // with a dark background set in the HTML/CSS instead.
        match tauri::webview::WebviewWindowBuilder::new(
            app,
            "overlay",
            tauri::WebviewUrl::App("overlay.html".into()),
        )
        .title("")
        .inner_size(280.0, 52.0)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .visible(true)
        .focused(false)
        .build()
        {
            Ok(window) => {
                let _ = window.set_always_on_top(true);
                position_overlay_bottom_center(&window);
                let _ = window.show();
                log::info!("Overlay window created and shown");
            }
            Err(e) => {
                log::error!("Failed to create overlay window: {}", e);
            }
        }
    }
}

/// Hide the overlay window and reset its state so it's clean for next use.
fn hide_overlay(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("overlay") {
        let _ = window.hide();
        // Reset frontend state while hidden so the next show() has a clean canvas
        let _ = window.eval("window.__resetOverlay && window.__resetOverlay()");
        log::info!("Overlay hidden");
    }
}

/// Position overlay above the Dock, centered horizontally.
/// Uses 200px from bottom to clear the Dock (~70-90px) with comfortable margin.
/// On Retina displays, all values are in physical pixels (2x logical).
fn position_overlay_bottom_center(window: &tauri::WebviewWindow) {
    if let Ok(Some(monitor)) = window.primary_monitor().or_else(|_| window.current_monitor()) {
        let screen = monitor.size();
        let scale = monitor.scale_factor();
        let pos = monitor.position();

        if let Ok(win_size) = window.outer_size() {
            let x = pos.x + (screen.width as i32 - win_size.width as i32) / 2;
            // 100 logical pixels from bottom — just above the Dock
            let margin_bottom = (100.0 * scale) as i32;
            let y = pos.y + screen.height as i32 - win_size.height as i32 - margin_bottom;
            let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
            log::info!(
                "Overlay at ({}, {}) — screen {}x{} scale={} margin_bottom={}px",
                x, y, screen.width, screen.height, scale, margin_bottom
            );
        }
    } else {
        log::warn!("Could not get monitor info for overlay positioning");
    }
}
