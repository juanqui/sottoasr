use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Manager};
use tauri_nspanel::{tauri_panel, ManagerExt, WebviewWindowExt as _};
use crate::models::AppStateEnum;
use crate::state::AppState;

// Define a non-activating NSPanel class for the overlay.
// can_become_key_window: false ensures it never steals focus from the user's app.
tauri_panel! {
    panel!(OverlayPanel {
        config: {
            can_become_key_window: false,
            is_floating_panel: true
        }
    })
}

/// Maximum recording duration before auto-stop (12 minutes).
const MAX_RECORDING_SECS: u64 = 12 * 60;
/// Seconds before max duration to show a warning (1 minute before).
const WARNING_BEFORE_LIMIT_SECS: u64 = 60;
/// Maximum sample rate we expect to handle (96kHz for high-quality audio).
const MAX_EXPECTED_SAMPLE_RATE_HZ: usize = 96_000;
/// Maximum audio buffer size (MAX_RECORDING_SECS at max expected sample rate).
/// Prevents memory exhaustion from unbounded recordings.
/// At 96kHz: 12 minutes * 60 seconds * 96,000 samples = 69.1M samples ≈ 277MB
const MAX_AUDIO_BUFFER_SAMPLES: usize = MAX_EXPECTED_SAMPLE_RATE_HZ * MAX_RECORDING_SECS as usize;

pub fn setup_hotkeys(app: &AppHandle) -> Result<(), String> {
    // Load persisted settings to use saved shortcuts (not hardcoded defaults)
    let settings = crate::commands::settings::load_persisted_settings();
    register_shortcuts(
        app,
        &settings.push_to_talk_shortcut,
        settings.push_to_talk_shortcut_alt.as_deref(),
        &settings.toggle_shortcut,
        settings.toggle_shortcut_alt.as_deref(),
        &settings.cancel_shortcut,
        settings.cancel_shortcut_alt.as_deref(),
    )
}

/// Re-register all shortcuts. Called at startup and when settings change.
/// Note: the cancel shortcut is NOT registered globally here — it is only
/// registered while recording is active (see register_cancel_shortcut).
pub fn register_shortcuts(
    app: &AppHandle,
    ptt_shortcut: &str,
    ptt_shortcut_alt: Option<&str>,
    toggle_shortcut: &str,
    toggle_shortcut_alt: Option<&str>,
    cancel_shortcut: &str,
    cancel_shortcut_alt: Option<&str>,
) -> Result<(), String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    // Unregister all existing shortcuts first
    let _ = app.global_shortcut().unregister_all();

    // Store the cancel shortcuts for dynamic registration during recording
    {
        let state: tauri::State<'_, AppState> = app.state();
        let mut cs = state.cancel_shortcut.lock().unwrap();
        *cs = cancel_shortcut.to_string();
        let mut cs_alt = state.cancel_shortcut_alt.lock().unwrap();
        *cs_alt = cancel_shortcut_alt.filter(|s| !s.is_empty()).map(|s| s.to_string());
    }

    // Helper: register a push-to-talk shortcut with the given key string
    fn register_ptt(app: &AppHandle, shortcut: &str) -> Result<(), String> {
        use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

        let app_handle = app.clone();
        let ptt_main_key = shortcut.split('+').next_back().unwrap_or("Space").to_string();

        app.global_shortcut().on_shortcut(shortcut, move |_app, _shortcut, event| {
            if event.state != ShortcutState::Pressed {
                return;
            }
            let app = app_handle.clone();
            let main_key = ptt_main_key.clone();
            tauri::async_runtime::spawn(async move {
                let state: tauri::State<'_, AppState> = app.state();
                if state.get_state() != AppStateEnum::Idle {
                    return;
                }
                handle_start_recording(&app);

                // Poll for key release via CGEventSourceKeyState
                #[cfg(target_os = "macos")]
                {
                    if let Some(vk) = crate::commands::keycapture::tauri_key_to_vk(&main_key) {
                        let app_for_release = app.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(std::time::Duration::from_millis(100));
                            unsafe {
                                extern "C" {
                                    fn CGEventSourceKeyState(stateID: u32, key: u16) -> bool;
                                }
                                loop {
                                    std::thread::sleep(std::time::Duration::from_millis(33));
                                    let still_pressed = CGEventSourceKeyState(0, vk);
                                    if !still_pressed {
                                        break;
                                    }
                                }
                            }
                            let state: tauri::State<'_, AppState> = app_for_release.state();
                            if state.get_state() == AppStateEnum::Recording {
                                tauri::async_runtime::spawn(async move {
                                    handle_stop_recording(&app_for_release).await;
                                });
                            }
                        });
                    }
                }
            });
        }).map_err(|e| format!("Failed to register push-to-talk shortcut '{}': {}", shortcut, e))
    }

    // Helper: register a toggle shortcut with the given key string
    fn register_toggle(app: &AppHandle, shortcut: &str) -> Result<(), String> {
        use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

        let app_handle = app.clone();
        app.global_shortcut().on_shortcut(shortcut, move |_app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                let app = app_handle.clone();
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
        }).map_err(|e| format!("Failed to register toggle shortcut '{}': {}", shortcut, e))
    }

    // Register primary shortcuts
    register_ptt(app, ptt_shortcut)?;
    register_toggle(app, toggle_shortcut)?;

    // Register alt shortcuts (if set and non-empty)
    if let Some(alt) = ptt_shortcut_alt.filter(|s| !s.is_empty()) {
        if let Err(e) = register_ptt(app, alt) {
            log::warn!("Failed to register alt push-to-talk shortcut: {}", e);
        }
    }
    if let Some(alt) = toggle_shortcut_alt.filter(|s| !s.is_empty()) {
        if let Err(e) = register_toggle(app, alt) {
            log::warn!("Failed to register alt toggle shortcut: {}", e);
        }
    }

    log::info!(
        "Hotkeys registered: '{}' (push-to-talk), '{}' (toggle) | cancel '{}' (registered only while recording)",
        ptt_shortcut, toggle_shortcut, cancel_shortcut
    );
    Ok(())
}

/// Register the cancel shortcut(s) globally. Called when recording starts.
pub fn register_cancel_shortcut(app: &AppHandle) {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    let state: tauri::State<'_, AppState> = app.state();
    let cancel_shortcut = state.cancel_shortcut.lock()
        .map(|cs| cs.clone())
        .unwrap_or_else(|_| "Escape".to_string());
    let cancel_shortcut_alt = state.cancel_shortcut_alt.lock()
        .ok()
        .and_then(|cs| cs.clone());

    // Helper to register a single cancel shortcut string
    let register_one = |app: &AppHandle, shortcut: &str| {
        let app_handle = app.clone();
        let result = app.global_shortcut().on_shortcut(shortcut, move |_app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                let app = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    let state: tauri::State<'_, AppState> = app.state();
                    let current = state.get_state();
                    if current == AppStateEnum::Recording {
                        handle_cancel_recording(&app).await;
                    }
                });
            }
        });

        match result {
            Ok(()) => log::info!("Cancel shortcut '{}' registered (recording active)", shortcut),
            Err(e) => log::error!("Failed to register cancel shortcut '{}': {}", shortcut, e),
        }
    };

    register_one(app, &cancel_shortcut);
    if let Some(alt) = cancel_shortcut_alt.as_deref().filter(|s| !s.is_empty()) {
        register_one(app, alt);
    }
}

/// Unregister the cancel shortcut(s). Called when recording stops or is cancelled.
pub fn unregister_cancel_shortcut(app: &AppHandle) {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    let state: tauri::State<'_, AppState> = app.state();
    let cancel_shortcut = state.cancel_shortcut.lock()
        .map(|cs| cs.clone())
        .unwrap_or_else(|_| "Escape".to_string());
    let cancel_shortcut_alt = state.cancel_shortcut_alt.lock()
        .ok()
        .and_then(|cs| cs.clone());

    match app.global_shortcut().unregister(cancel_shortcut.as_str()) {
        Ok(()) => log::info!("Cancel shortcut '{}' unregistered (recording ended)", cancel_shortcut),
        Err(e) => log::warn!("Failed to unregister cancel shortcut '{}': {}", cancel_shortcut, e),
    }
    if let Some(alt) = cancel_shortcut_alt.as_deref().filter(|s| !s.is_empty()) {
        match app.global_shortcut().unregister(alt) {
            Ok(()) => log::info!("Cancel alt shortcut '{}' unregistered", alt),
            Err(e) => log::warn!("Failed to unregister cancel alt shortcut '{}': {}", alt, e),
        }
    }
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
            // Capture the frontmost app PID before showing the overlay.
            // This is the app that should receive the paste when transcription completes.
            let target_pid = crate::paste::get_frontmost_pid();
            state.target_pid.store(target_pid, Ordering::SeqCst);
            log::info!("Captured frontmost app PID: {}", target_pid);

            state.set_state(AppStateEnum::Recording);
            let _ = app.emit("recording-started", ());
            let _ = app.emit("state-changed", &AppStateEnum::Recording);

            // Register cancel shortcut (only active while recording)
            register_cancel_shortcut(app);

            // Show the overlay window
            show_overlay(app);

            // Spawn auto-stop timer: warn at (MAX - WARNING) seconds, stop at MAX seconds.
            // Capture the recording generation so this timer can detect if a new
            // recording session has started (making this timer stale).
            let generation = state.recording_generation.fetch_add(1, Ordering::SeqCst) + 1;
            let app_for_timer = app.clone();
            tokio::spawn(async move {
                let warning_at = MAX_RECORDING_SECS - WARNING_BEFORE_LIMIT_SECS;
                tokio::time::sleep(std::time::Duration::from_secs(warning_at)).await;
                let state: tauri::State<'_, AppState> = app_for_timer.state();
                if state.recording_generation.load(Ordering::SeqCst) != generation {
                    log::info!("Auto-stop timer (gen {}) is stale, exiting", generation);
                    return;
                }
                if state.get_state() != AppStateEnum::Recording {
                    return;
                }
                log::info!("Recording approaching limit — {}s warning", WARNING_BEFORE_LIMIT_SECS);
                let _ = app_for_timer.emit(
                    "recording-time-warning",
                    serde_json::json!({ "remaining_secs": WARNING_BEFORE_LIMIT_SECS }),
                );

                tokio::time::sleep(std::time::Duration::from_secs(WARNING_BEFORE_LIMIT_SECS)).await;
                let state: tauri::State<'_, AppState> = app_for_timer.state();
                if state.recording_generation.load(Ordering::SeqCst) != generation {
                    log::info!("Auto-stop timer (gen {}) is stale after warning, exiting", generation);
                    return;
                }
                if state.get_state() == AppStateEnum::Recording {
                    log::info!("Max recording duration ({}s) reached — auto-stopping", MAX_RECORDING_SECS);
                    handle_stop_recording(&app_for_timer).await;
                }
            });

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

    // Unregister cancel shortcut so it doesn't block the key globally
    unregister_cancel_shortcut(app);

    // Keep overlay visible through transcription and LLM cleanup.
    // It will be hidden after paste/clipboard write completes.

    state.set_state(AppStateEnum::Transcribing);
    let _ = app.emit("recording-stopped", ());
    let _ = app.emit("state-changed", &AppStateEnum::Transcribing);

    // Collect all audio samples from the channel
    // Check buffer limit to prevent memory exhaustion
    let samples = {
        let rx = state.audio_receiver.lock().unwrap();
        let mut all = Vec::new();
        while let Ok(chunk) = rx.try_recv() {
            // Check if adding this chunk would exceed the buffer limit
            if all.len().saturating_add(chunk.len()) > MAX_AUDIO_BUFFER_SAMPLES {
                log::error!(
                    "Audio buffer limit ({}) exceeded after {} samples - recording too long",
                    MAX_AUDIO_BUFFER_SAMPLES,
                    all.len()
                );
                // Hide overlay before returning
                hide_overlay(app);
                let _ = app.emit("recording-error", serde_json::json!({
                    "error": format!("Recording too long: maximum {} minutes of audio allowed", MAX_AUDIO_BUFFER_SAMPLES / 48_000 / 60)
                }));
                state.set_state(AppStateEnum::Idle);
                let _ = app.emit("state-changed", &AppStateEnum::Idle);
                return;
            }
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

    // Assign a job ID for stale-result prevention
    let job_id = state.new_job();

    tokio::spawn(async move {
        let state: tauri::State<'_, AppState> = app_clone.state();
        let mut engine = state.asr_engine.lock().await;

        log::info!("Starting transcription...");

        let result = engine.transcribe_file(&temp_path_str);
        drop(engine); // Release ASR engine lock

        // Clean up temp file
        let _ = std::fs::remove_file(&temp_path);

        match result {
            Ok(asr_result) => {
                log::info!("Transcription result: \"{}\" (RTF: {:.1}x)", &asr_result.text, asr_result.rtfx);

                // Check if this job is still current (user may have started a new recording)
                if !state.is_current_job(job_id) {
                    log::info!("Job {} is stale, discarding transcription", job_id);
                    return;
                }

                let raw_asr_text = asr_result.text.clone();
                let mut final_text = asr_result.text.clone();
                let mut llm_was_applied = false;

                // LLM cleanup (if enabled and input is long enough)
                let settings = state.settings.lock().await;
                let llm_enabled = settings.llm_cleanup_enabled;
                let markdown_mode = settings.llm_markdown_mode;
                let llm_model_size = settings.llm_model_size.clone();
                let auto_paste = settings.auto_paste;
                let restore_clipboard = settings.restore_clipboard;
                let restore_focus_before_paste = settings.restore_focus_before_paste;
                drop(settings);

                if llm_enabled && final_text.split_whitespace().count() >= 5 {
                    // Transition overlay to "Cleaning up..." state
                    state.set_state(AppStateEnum::CleaningUp);
                    let _ = app_clone.emit("state-changed", &AppStateEnum::CleaningUp);

                    let mode = if markdown_mode {
                        crate::llm::prompts::CleanupMode::Markdown
                    } else {
                        crate::llm::prompts::CleanupMode::Standard
                    };

                    let selected_model_id = crate::llm::engine::model_id_for_size(&llm_model_size).to_string();

                    {
                        let mut llm_guard = state.llm_engine.lock().await;

                        // Respawn sidecar if model changed or not running
                        let needs_spawn = match llm_guard.as_ref() {
                            None => true,
                            Some(engine) => engine.model_id != selected_model_id,
                        };

                        if needs_spawn {
                            // Shut down old sidecar if model changed
                            if let Some(mut old) = llm_guard.take() {
                                log::info!("Model changed, shutting down old sidecar");
                                old.quit();
                            }

                            log::info!("Spawning LLM sidecar for {}...", selected_model_id);
                            let model_id_for_spawn = selected_model_id.clone();
                            match tokio::task::spawn_blocking(move || {
                                crate::llm::engine::LlmEngine::spawn_with_model(&model_id_for_spawn)
                            }).await {
                                Ok(Ok(engine)) => {
                                    *llm_guard = Some(engine);
                                }
                                Ok(Err(e)) => {
                                    log::warn!("Failed to spawn LLM sidecar: {}", e);
                                }
                                Err(e) => {
                                    log::error!("LLM sidecar spawn panicked: {}", e);
                                }
                            }
                        }

                        // Run cleanup via sidecar (take/put pattern for spawn_blocking Send requirement)
                        if let Some(mut llm) = llm_guard.take() {
                            let text_for_cleanup = final_text.clone();

                            let cleanup_result = tokio::time::timeout(
                                std::time::Duration::from_secs(30),
                                tokio::task::spawn_blocking(move || {
                                    let result = llm.cleanup(&text_for_cleanup, mode);
                                    (llm, result)
                                }),
                            ).await;

                            match cleanup_result {
                                Ok(Ok((llm_back, Ok(cleaned)))) => {
                                    *llm_guard = Some(llm_back);
                                    log::info!("LLM cleanup: {} → {} chars", final_text.len(), cleaned.len());
                                    final_text = cleaned;
                                    llm_was_applied = true;
                                }
                                Ok(Ok((llm_back, Err(e)))) => {
                                    *llm_guard = Some(llm_back);
                                    log::warn!("LLM cleanup failed: {}, using raw text", e);
                                }
                                Ok(Err(e)) => {
                                    log::error!("LLM cleanup task panicked: {}, sidecar lost", e);
                                }
                                Err(_) => {
                                    log::warn!("LLM cleanup timed out after 30s, using raw text");
                                }
                            }
                        }
                    }
                }

                // Check again if this job is still current
                if !state.is_current_job(job_id) {
                    log::info!("Job {} is stale after cleanup, discarding", job_id);
                    hide_overlay(&app_clone);
                    return;
                }

                let transcription = crate::models::Transcription {
                    id: uuid::Uuid::new_v4().to_string(),
                    text: final_text.clone(),
                    duration_ms: (asr_result.duration_secs * 1000.0) as u64,
                    created_at: chrono::Utc::now(),
                    word_count: final_text.split_whitespace().count(),
                    cancelled: false,
                    raw_text: if llm_was_applied { Some(raw_asr_text.clone()) } else { None },
                    llm_applied: llm_was_applied,
                };

                // Save
                {
                    let mut last = state.last_transcription.lock().await;
                    *last = Some(transcription.clone());
                }
                crate::commands::transcription::add_transcription(transcription.clone()).await;

                let _ = app_clone.emit("transcription-complete", &transcription);

                // Paste at cursor (if auto_paste is enabled), then hide overlay
                if !final_text.trim().is_empty() {
                    if auto_paste {
                        let target_pid = if restore_focus_before_paste {
                            let start_pid = state.target_pid.load(Ordering::SeqCst);
                            let current_pid = crate::paste::get_frontmost_pid();
                            let our_pid = std::process::id() as i32;

                            if current_pid == start_pid || current_pid == our_pid || current_pid == 0 {
                                // Same app or SottoASR stole focus → use original target
                                start_pid
                            } else {
                                // User intentionally switched apps → paste where they are now
                                log::info!("User switched apps during recording: {} → {}, pasting at current", start_pid, current_pid);
                                current_pid
                            }
                        } else {
                            0
                        };
                        let paste_result = if restore_clipboard {
                            crate::paste::paste_text_and_restore(&final_text, target_pid)
                        } else {
                            crate::paste::paste_text(&final_text, target_pid)
                        };

                        match paste_result {
                            Ok(()) => {
                                log::info!("Text pasted at cursor");
                                let _ = app_clone.emit("paste-complete", serde_json::json!({ "id": &transcription.id }));
                            }
                            Err(e) => {
                                log::error!("Paste failed: {}", e);
                                let _ = app_clone.emit("paste-error", serde_json::json!({
                                    "error": &e,
                                    "text": &final_text,
                                    "needs_restart": e.contains("restart"),
                                    "needs_permission": e.contains("permission not granted"),
                                }));
                                let _ = crate::paste::copy_to_clipboard(&final_text);
                                log::info!("Text copied to clipboard as fallback");
                            }
                        }
                    } else {
                        match crate::paste::copy_to_clipboard(&final_text) {
                            Ok(()) => {
                                log::info!("Text copied to clipboard (auto_paste disabled)");
                                let _ = app_clone.emit("paste-complete", serde_json::json!({ "id": &transcription.id, "clipboard_only": true }));
                            }
                            Err(e) => {
                                log::error!("Clipboard copy failed: {}", e);
                                let _ = app_clone.emit("paste-error", serde_json::json!({ "error": e }));
                            }
                        }
                    }
                }
                // Hide overlay after paste/copy completes
                hide_overlay(&app_clone);
            }
            Err(e) => {
                log::error!("Transcription failed: {}", e);
                let _ = app_clone.emit("transcription-error", serde_json::json!({ "error": e }));
                hide_overlay(&app_clone);
            }
        }

        state.set_state(AppStateEnum::Idle);
        let _ = app_clone.emit("state-changed", &AppStateEnum::Idle);
    });

    log::info!("Recording stopped, transcription queued");
}

/// Cancel recording: stop mic, transcribe what we have, save as cancelled, don't paste.
async fn handle_cancel_recording(app: &AppHandle) {
    let state: tauri::State<'_, AppState> = app.state();
    let current = state.get_state();

    if current != AppStateEnum::Recording {
        log::warn!("Cannot cancel recording: currently in {:?} state", current);
        return;
    }

    // Stop audio capture
    state.is_recording.store(false, Ordering::SeqCst);
    {
        let mut capture = state.audio_capture.lock().unwrap();
        capture.stop();
    }

    // Unregister cancel shortcut so it doesn't block the key globally
    unregister_cancel_shortcut(app);

    // Hide the overlay
    hide_overlay(app);

    let _ = app.emit("recording-cancelled", ());
    let _ = app.emit("state-changed", &AppStateEnum::Idle);

    // Collect audio samples
    let samples = {
        let rx = state.audio_receiver.lock().unwrap();
        let mut all = Vec::new();
        while let Ok(chunk) = rx.try_recv() {
            all.extend(chunk);
        }
        all
    };

    let sample_count = samples.len();
    log::info!("Recording cancelled — {} samples collected", sample_count);

    // If we have enough audio, transcribe it and save as cancelled
    if sample_count >= 4000 {
        let app_clone = app.clone();
        let temp_path = std::env::temp_dir().join(format!("sotto_{}.wav", uuid::Uuid::new_v4()));

        let sample_rate = {
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

        if let Ok(mut writer) = hound::WavWriter::create(&temp_path, wav_spec) {
            for &sample in &samples {
                let _ = writer.write_sample(sample);
            }
            let _ = writer.finalize();

            let temp_path_str = temp_path.to_string_lossy().to_string();
            tokio::spawn(async move {
                let state: tauri::State<'_, AppState> = app_clone.state();
                let mut engine = state.asr_engine.lock().await;
                let result = engine.transcribe_file(&temp_path_str);
                let _ = std::fs::remove_file(&temp_path);

                if let Ok(asr_result) = result {
                    let transcription = crate::models::Transcription {
                        id: uuid::Uuid::new_v4().to_string(),
                        text: asr_result.text.clone(),
                        duration_ms: (asr_result.duration_secs * 1000.0) as u64,
                        created_at: chrono::Utc::now(),
                        word_count: asr_result.text.split_whitespace().count(),
                        cancelled: true,
                        raw_text: None,
                        llm_applied: false,
                    };
                    crate::commands::transcription::add_transcription(transcription.clone()).await;
                    let _ = app_clone.emit("transcription-complete", &transcription);
                    log::info!("Cancelled transcription saved: \"{}\"", &asr_result.text[..asr_result.text.len().min(50)]);
                }

                state.set_state(AppStateEnum::Idle);
            });
        } else {
            state.set_state(AppStateEnum::Idle);
        }
    } else {
        // Too short to transcribe — just save a placeholder
        let transcription = crate::models::Transcription {
            id: uuid::Uuid::new_v4().to_string(),
            text: String::new(),
            duration_ms: (sample_count as u64 * 1000) / 48000,
            created_at: chrono::Utc::now(),
            word_count: 0,
            cancelled: true,
            raw_text: None,
            llm_applied: false,
        };
        crate::commands::transcription::add_transcription(transcription).await;
        state.set_state(AppStateEnum::Idle);
    }

    log::info!("Recording cancelled");
}

/// Logical dimensions of the overlay pill window.
const OVERLAY_WIDTH: f64 = 300.0;
const OVERLAY_HEIGHT: f64 = 110.0;

/// Pre-create the overlay panel at startup so that the first recording
/// doesn't steal focus. WebviewWindowBuilder::build() activates the app
/// on macOS (Tauri bug #9065), but this only happens on initial creation.
/// By creating the panel early (hidden), the first recording can just
/// show the existing non-activating NSPanel.
pub fn precreate_overlay(app: &AppHandle) {
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        // Don't create if it already exists
        if app.get_webview_panel("overlay").is_ok() {
            return;
        }

        let window = match tauri::webview::WebviewWindowBuilder::new(
            &app,
            "overlay",
            tauri::WebviewUrl::App("overlay.html".into()),
        )
        .title("")
        .inner_size(OVERLAY_WIDTH, OVERLAY_HEIGHT)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .visible(false)
        .focused(false)
        .build()
        {
            Ok(w) => w,
            Err(e) => {
                log::error!("Failed to pre-create overlay window: {}", e);
                return;
            }
        };

        match window.to_panel::<OverlayPanel>() {
            Ok(panel) => {
                panel.set_transparent(true);
                panel.set_has_shadow(false);
                panel.set_hides_on_deactivate(false);
                // Visible on all macOS Spaces (like Cmd+Shift+5 screenshot toolbar)
                panel.set_collection_behavior(
                    tauri_nspanel::objc2_app_kit::NSWindowCollectionBehavior::CanJoinAllSpaces
                    | tauri_nspanel::objc2_app_kit::NSWindowCollectionBehavior::FullScreenAuxiliary,
                );
                clear_all_backgrounds(panel.as_panel());
                // Do NOT show — leave hidden until first recording
                log::info!("Overlay panel pre-created (hidden)");
            }
            Err(e) => {
                log::error!("Failed to convert overlay to panel during pre-creation: {}", e);
            }
        }
    });
}

/// Show the floating overlay pill panel. Creates it if it doesn't exist.
/// Panel operations (show/hide/create) must run on the main thread for NSPanel.
fn show_overlay(app: &AppHandle) {
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        // Try to show an existing panel first
        if let Ok(panel) = app.get_webview_panel("overlay") {
            panel.show();
            // Re-apply floating level after show — this is required to fix
            // Tauri issue #13530 where the setting is lost after hide/show.
            // The PanelLevel::Floating keeps the window above normal windows.
            use tauri_nspanel::PanelLevel;
            panel.set_level(PanelLevel::Floating.into());
            // Re-apply floating panel behavior to ensure consistent layering
            panel.set_floating_panel(true);
            // Bring to front without activating
            panel.order_front_regardless();
            if let Some(window) = app.get_webview_window("overlay") {
                if let Some(pos) = compute_overlay_position(&window) {
                    let _ = window.set_position(pos);
                }
            }
            log::info!("Overlay shown (existing panel, floating level reapplied)");
            return;
        }

        // Fallback: check for a regular webview window (handles case where panel
        // conversion failed in the past and we have a plain window instead).
        if let Some(window) = app.get_webview_window("overlay") {
            let _ = window.show();
            if let Some(pos) = compute_overlay_position(&window) {
                let _ = window.set_position(pos);
            }
            log::info!("Overlay shown (existing window fallback)");
            return;
        }

        // Create the overlay as a regular WebviewWindow first (proven to work),
        // then convert it to an NSPanel for proper transparency.
        let window = match tauri::webview::WebviewWindowBuilder::new(
            &app,
            "overlay",
            tauri::WebviewUrl::App("overlay.html".into()),
        )
        .title("")
        .inner_size(OVERLAY_WIDTH, OVERLAY_HEIGHT)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .visible(false)
        .focused(false)
        .build()
        {
            Ok(w) => w,
            Err(e) => {
                log::error!("Failed to create overlay window: {}", e);
                return;
            }
        };

        // Position the window
        if let Some(pos) = compute_overlay_position(&window) {
            let _ = window.set_position(pos);
        }

        // Convert to NSPanel for true transparency.
        match window.to_panel::<OverlayPanel>() {
            Ok(panel) => {
                panel.set_transparent(true);
                panel.set_has_shadow(false);
                panel.set_hides_on_deactivate(false);
                panel.set_collection_behavior(
                    tauri_nspanel::objc2_app_kit::NSWindowCollectionBehavior::CanJoinAllSpaces
                    | tauri_nspanel::objc2_app_kit::NSWindowCollectionBehavior::FullScreenAuxiliary,
                );

                // Synchronously clear backgrounds on ALL views in the hierarchy.
                clear_all_backgrounds(panel.as_panel());

                panel.show();
                log::info!("Overlay panel created and shown");
            }
            Err(e) => {
                log::error!("Failed to convert overlay to panel: {}, showing as window", e);
                let _ = window.show();
            }
        }
    });
}

/// Hide the overlay panel and reset its state for the next recording.
fn hide_overlay(app: &AppHandle) {
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        // Reset overlay state (timer, waveform, isRecording) so the next
        // recording starts fresh with a false→true transition.
        if let Some(window) = app.get_webview_window("overlay") {
            let _ = window.eval("window.__resetOverlay && window.__resetOverlay()");
        }

        if let Ok(panel) = app.get_webview_panel("overlay") {
            panel.hide();
            log::info!("Overlay hidden");
        } else if let Some(window) = app.get_webview_window("overlay") {
            let _ = window.hide();
            log::info!("Overlay hidden (window fallback)");
        }
    });
}

/// Synchronously clear background drawing on all views in the NSPanel hierarchy.
/// This walks the content view tree and sets every view + the WKWebView to transparent.
fn clear_all_backgrounds(panel: &tauri_nspanel::NSPanel) {
    use tauri_nspanel::{objc2, objc2_app_kit, objc2_foundation};

    unsafe {
        // 1. Clear the content view background
        let content_view: objc2::rc::Retained<objc2_app_kit::NSView> =
            objc2::msg_send![panel, contentView];
        let _: () = objc2::msg_send![&*content_view, setWantsLayer: true];
        if let Some(layer) = content_view.layer() {
            let clear: objc2::rc::Retained<objc2_foundation::NSObject> =
                objc2::msg_send![objc2::class!(NSColor), clearColor];
            let _: () = objc2::msg_send![&*layer, setBackgroundColor:
                { let cg: *const std::ffi::c_void = objc2::msg_send![&*clear, CGColor]; cg }];
        }

        // 2. Walk all subviews and clear backgrounds / find WKWebView
        let subviews: objc2::rc::Retained<objc2_foundation::NSArray<objc2_app_kit::NSView>> =
            objc2::msg_send![&*content_view, subviews];
        let count: usize = subviews.count();

        for i in 0..count {
            let view: objc2::rc::Retained<objc2_app_kit::NSView> =
                objc2::msg_send![&*subviews, objectAtIndex: i];

            // Check if this is a WKWebView by class name
            let cls: *const objc2::runtime::AnyClass = objc2::msg_send![&*view, class];
            let cls_name = std::ffi::CStr::from_ptr((*cls).name().to_bytes_with_nul().as_ptr() as *const _);
            let cls_str = cls_name.to_str().unwrap_or("");

            if cls_str.contains("WKWebView") || cls_str.contains("WebView") {
                // Set drawsBackground = false on the WKWebView
                let key = objc2_foundation::NSString::from_str("drawsBackground");
                let no: objc2::rc::Retained<objc2_foundation::NSNumber> =
                    objc2::msg_send![objc2::class!(NSNumber), numberWithBool: false];
                let _: () = objc2::msg_send![&*view, setValue: &*no, forKey: &*key];
                log::info!("Found {} — set drawsBackground=false", cls_str);
            }

            // Make every view non-opaque with clear layer background
            let _: () = objc2::msg_send![&*view, setWantsLayer: true];
            if let Some(layer) = view.layer() {
                let clear: objc2::rc::Retained<objc2_foundation::NSObject> =
                    objc2::msg_send![objc2::class!(NSColor), clearColor];
                let _: () = objc2::msg_send![&*layer, setBackgroundColor:
                    { let cg: *const std::ffi::c_void = objc2::msg_send![&*clear, CGColor]; cg }];
            }
        }
        log::info!("Cleared backgrounds on {} subviews", count);
    }
}

/// Compute overlay position: centered horizontally, 100 logical pixels above bottom.
/// Uses the monitor containing the mouse cursor (active screen), falling back to primary.
fn compute_overlay_position(window: &tauri::WebviewWindow) -> Option<tauri::PhysicalPosition<i32>> {
    // Prefer the monitor under the mouse cursor (the screen the user is actively on)
    let monitor = window.available_monitors().ok()
        .and_then(|monitors| {
            let mouse_pos = get_mouse_position();
            monitors.into_iter().find(|m| {
                let pos = m.position();
                let size = m.size();
                mouse_pos.0 >= pos.x && mouse_pos.0 < pos.x + size.width as i32
                    && mouse_pos.1 >= pos.y && mouse_pos.1 < pos.y + size.height as i32
            })
        })
        .or_else(|| window.current_monitor().ok().flatten())
        .or_else(|| window.primary_monitor().ok().flatten())?;

    let screen = monitor.size();
    let scale = monitor.scale_factor();
    let pos = monitor.position();

    let win_phys_w = (OVERLAY_WIDTH * scale) as i32;
    let win_phys_h = (OVERLAY_HEIGHT * scale) as i32;

    let x = pos.x + (screen.width as i32 - win_phys_w) / 2;
    let margin_bottom = (100.0 * scale) as i32;
    let y = pos.y + screen.height as i32 - win_phys_h - margin_bottom;

    log::info!(
        "Overlay at ({}, {}) — screen {}x{} scale={} margin_bottom={}px",
        x, y, screen.width, screen.height, scale, margin_bottom
    );

    Some(tauri::PhysicalPosition::new(x, y))
}

/// Get the current mouse cursor position in global (physical) coordinates.
fn get_mouse_position() -> (i32, i32) {
    unsafe {
        // NSEvent.mouseLocation returns the position in screen coordinates
        // with origin at bottom-left. Tauri uses top-left origin, so we need
        // to flip the Y coordinate using the main screen height.
        let mouse_loc: tauri_nspanel::objc2_foundation::NSPoint =
            tauri_nspanel::objc2::msg_send![
                tauri_nspanel::objc2::class!(NSEvent),
                mouseLocation
            ];
        // Get main screen height for Y-flip
        let screens: *const tauri_nspanel::objc2_foundation::NSArray<tauri_nspanel::objc2_app_kit::NSScreen> =
            tauri_nspanel::objc2::msg_send![
                tauri_nspanel::objc2::class!(NSScreen),
                screens
            ];
        let main_screen: *const tauri_nspanel::objc2_app_kit::NSScreen =
            tauri_nspanel::objc2::msg_send![&*screens, objectAtIndex: 0usize];
        let main_frame: tauri_nspanel::objc2_foundation::NSRect =
            tauri_nspanel::objc2::msg_send![main_screen, frame];

        let x = mouse_loc.x as i32;
        let y = (main_frame.size.height - mouse_loc.y) as i32;
        (x, y)
    }
}
