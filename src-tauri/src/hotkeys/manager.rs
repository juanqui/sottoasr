use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Manager};
use tauri_nspanel::{tauri_panel, ManagerExt, WebviewWindowExt as _};
use crate::llm::cleanup::run_cleanup;
use crate::models::{AppStateEnum, LlmCleanupStatus};
use crate::state::{AppState, OverlaySession};

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

/// Maximum recording duration before auto-stop (20 minutes).
/// Raised from 12 min in the 2026-04-11 reliability spec to support long
/// dictations. At 1.24 tok/word and 150 WPM, 20 min ≈ 3000 words ≈ 3720
/// output tokens — still well inside the sidecar's 16384-token ceiling.
const MAX_RECORDING_SECS: u64 = 20 * 60;
/// Seconds before max duration to show a warning (1 minute before).
const WARNING_BEFORE_LIMIT_SECS: u64 = 60;
/// Maximum sample rate we expect to handle (96kHz for high-quality audio).
const MAX_EXPECTED_SAMPLE_RATE_HZ: usize = 96_000;
/// Maximum audio buffer size (MAX_RECORDING_SECS at max expected sample rate).
/// Prevents memory exhaustion from unbounded recordings.
/// At 96kHz: 20 minutes * 60 seconds * 96,000 samples = 115.2M samples ≈ 460MB
const MAX_AUDIO_BUFFER_SAMPLES: usize = MAX_EXPECTED_SAMPLE_RATE_HZ * MAX_RECORDING_SECS as usize;

pub fn setup_hotkeys(app: &AppHandle) -> Result<(), String> {
    let settings = crate::commands::settings::load_persisted_settings();
    register_shortcuts(app, &settings)
}

/// Re-register all shortcuts. Called at startup and when settings change.
/// Note: the cancel shortcut is NOT registered globally here — it is only
/// registered while recording is active (see register_cancel_shortcut).
pub fn register_shortcuts(
    app: &AppHandle,
    settings: &crate::models::Settings,
) -> Result<(), String> {
    let ptt_shortcut: &str = &settings.push_to_talk_shortcut;
    let ptt_shortcut_alt = settings.push_to_talk_shortcut_alt.as_deref();
    let toggle_shortcut: &str = &settings.toggle_shortcut;
    let toggle_shortcut_alt = settings.toggle_shortcut_alt.as_deref();
    let cancel_shortcut: &str = &settings.cancel_shortcut;
    let cancel_shortcut_alt = settings.cancel_shortcut_alt.as_deref();
    let open_settings_shortcut: &str = &settings.open_settings_shortcut;
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    // Unregister all existing shortcuts first
    let _ = app.global_shortcut().unregister_all();

    // Store the cancel shortcuts for dynamic registration during recording
    {
        let state: tauri::State<'_, AppState> = app.state();
        let mut cs = state.cancel_shortcut.lock().unwrap_or_else(|e| e.into_inner());
        *cs = cancel_shortcut.to_string();
        let mut cs_alt = state.cancel_shortcut_alt.lock().unwrap_or_else(|e| e.into_inner());
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
                                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(12 * 60 + 30);
                                loop {
                                    std::thread::sleep(std::time::Duration::from_millis(33));
                                    if std::time::Instant::now() > deadline {
                                        log::warn!("PTT key release polling timed out after {}s", 12 * 60 + 30);
                                        break;
                                    }
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

    // Register open-settings shortcut
    if !open_settings_shortcut.is_empty() {
        let app_handle = app.clone();
        app.global_shortcut().on_shortcut(open_settings_shortcut, move |_app, _shortcut, event| {
            use tauri_plugin_global_shortcut::ShortcutState;
            if event.state == ShortcutState::Pressed {
                log::info!("Open-settings hotkey pressed");
                crate::tray::menu::open_or_focus_window(
                    &app_handle,
                    "settings",
                    "settings.html",
                    "SottoASR \u{2014} Settings",
                    520.0,
                    600.0,
                );
            }
        }).map_err(|e| format!("Failed to register open-settings shortcut '{}': {}", open_settings_shortcut, e))?;
    }

    log::info!(
        "Hotkeys registered: '{}' (push-to-talk), '{}' (toggle), '{}' (open-settings) | cancel '{}' (registered only while recording)",
        ptt_shortcut, toggle_shortcut, open_settings_shortcut, cancel_shortcut
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
        let s = state.audio_sender.lock().unwrap_or_else(|e| e.into_inner());
        s.clone()
    };
    let is_recording = Arc::new(AtomicBool::new(true));
    state.is_recording.store(true, Ordering::SeqCst);

    let is_recording_clone = is_recording.clone();
    let app_clone = app.clone();

    let start_result = {
        let mut capture = state.audio_capture.lock().unwrap_or_else(|e| e.into_inner());
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
            let target_pid = state.paste_backend.get_frontmost_pid();
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
pub async fn handle_stop_recording(app: &AppHandle) {
    let state: tauri::State<'_, AppState> = app.state();
    let current = state.get_state();

    if current != AppStateEnum::Recording {
        log::warn!("Cannot stop recording: currently in {:?} state", current);
        return;
    }

    // Stop audio capture — drop the stream first so all in-flight callbacks
    // finish sending their samples before we clear the recording flag.
    {
        let mut capture = state.audio_capture.lock().unwrap_or_else(|e| e.into_inner());
        capture.stop();
    }
    state.is_recording.store(false, Ordering::SeqCst);

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
        let rx = state.audio_receiver.lock().unwrap_or_else(|e| e.into_inner());
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
        let capture = state.audio_capture.lock().unwrap_or_else(|e| e.into_inner());
        capture.sample_rate()
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
            // Append trailing silence so the ASR model fully processes the final
            // audio chunk. FluidAudio's chunked Parakeet TDT decoder may not emit
            // tokens for the last few words when speech extends to the very end of
            // the audio without a silence boundary.
            let silence_pad_ms: usize = 750;
            let silence_samples = sample_rate as usize * silence_pad_ms / 1000;
            for _ in 0..silence_samples {
                let _ = writer.write_sample(0.0f32);
            }
            let _ = writer.finalize();
            log::info!("Wrote temp WAV: {:?} ({} + {} pad samples, {} Hz)",
                temp_path, samples.len(), silence_samples, sample_rate);
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
                    state.set_state(AppStateEnum::Idle);
                    let _ = app_clone.emit("state-changed", &AppStateEnum::Idle);
                    hide_overlay(&app_clone);
                    return;
                }

                let raw_asr_text = asr_result.text.clone();
                let mut final_text = asr_result.text.clone();
                let mut llm_was_applied = false;
                let cleanup_status: LlmCleanupStatus;

                // LLM cleanup (if enabled).
                let settings = state.settings.lock().await;
                let llm_enabled = settings.llm_cleanup_enabled;
                let auto_paste = settings.auto_paste;
                let restore_clipboard = settings.restore_clipboard;
                let restore_focus_before_paste = settings.restore_focus_before_paste;
                drop(settings);

                let show_overlay_setting = {
                    let s = state.settings.lock().await;
                    s.show_overlay
                };

                if llm_enabled {
                    // Transition overlay to "Cleaning up..." state so the
                    // user sees the pipeline moved on from transcription.
                    state.set_state(AppStateEnum::CleaningUp);
                    let _ = app_clone.emit("state-changed", &AppStateEnum::CleaningUp);

                    let (cleaned, status) = run_cleanup(&state, &final_text).await;
                    cleanup_status = status;
                    if matches!(cleanup_status, LlmCleanupStatus::Applied { .. }) {
                        final_text = cleaned;
                        llm_was_applied = true;
                    }
                } else {
                    cleanup_status = LlmCleanupStatus::Disabled;
                }

                // Update cached last-status and emit to the UI so the overlay
                // can show a brief badge before hiding. Both places read the
                // same enum, so the overlay and the history view stay in sync.
                {
                    let mut last = state.llm_last_status.lock().await;
                    *last = cleanup_status.clone();
                }
                let _ = app_clone.emit("llm-cleanup-status", &cleanup_status);

                // Decide how long to keep the overlay open after cleanup so
                // the badge is visible to the user. Failure modes get a longer
                // dwell since the user needs time to read the message.
                // SkippedTooShort and Disabled get 0 because there's nothing
                // worth interrupting the flow for.
                let badge_dwell_ms: u64 = if !show_overlay_setting {
                    0
                } else {
                    match cleanup_status {
                        LlmCleanupStatus::Applied { .. } => 800,
                        LlmCleanupStatus::SkippedTooShort
                        | LlmCleanupStatus::Disabled
                        | LlmCleanupStatus::Idle => 0,
                        LlmCleanupStatus::Unavailable { .. }
                        | LlmCleanupStatus::Failed { .. }
                        | LlmCleanupStatus::TimedOut { .. } => 2000,
                    }
                };

                // Check again if this job is still current
                if !state.is_current_job(job_id) {
                    log::info!("Job {} is stale after cleanup, discarding", job_id);
                    state.set_state(AppStateEnum::Idle);
                    let _ = app_clone.emit("state-changed", &AppStateEnum::Idle);
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
                    llm_cleanup_status: cleanup_status.clone(),
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
                            let current_pid = state.paste_backend.get_frontmost_pid();
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
                            state.paste_backend.paste_text_and_restore(&final_text, target_pid)
                        } else {
                            state.paste_backend.paste_text(&final_text, target_pid)
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
                                let _ = state.paste_backend.copy_to_clipboard(&final_text);
                                log::info!("Text copied to clipboard as fallback");
                            }
                        }
                    } else {
                        match state.paste_backend.copy_to_clipboard(&final_text) {
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
                // Linger briefly so the cleanup-status badge is visible to the
                // user before the overlay hides. The paste already happened,
                // so this only delays the hide animation, not the user-visible
                // text appearing at the cursor.
                if badge_dwell_ms > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(badge_dwell_ms)).await;
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
pub async fn handle_cancel_recording(app: &AppHandle) {
    let state: tauri::State<'_, AppState> = app.state();
    let current = state.get_state();

    if current != AppStateEnum::Recording {
        log::warn!("Cannot cancel recording: currently in {:?} state", current);
        return;
    }

    // Stop audio capture — drop the stream first so all in-flight callbacks
    // finish sending their samples before we clear the recording flag.
    {
        let mut capture = state.audio_capture.lock().unwrap_or_else(|e| e.into_inner());
        capture.stop();
    }
    state.is_recording.store(false, Ordering::SeqCst);

    // Unregister cancel shortcut so it doesn't block the key globally
    unregister_cancel_shortcut(app);

    // Hide the overlay
    hide_overlay(app);

    let _ = app.emit("recording-cancelled", ());
    let _ = app.emit("state-changed", &AppStateEnum::Idle);

    // Collect audio samples
    let samples = {
        let rx = state.audio_receiver.lock().unwrap_or_else(|e| e.into_inner());
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
            let capture = state.audio_capture.lock().unwrap_or_else(|e| e.into_inner());
            capture.sample_rate()
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
                        llm_cleanup_status: crate::models::LlmCleanupStatus::Idle,
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
            llm_cleanup_status: crate::models::LlmCleanupStatus::Idle,
        };
        crate::commands::transcription::add_transcription(transcription).await;
        state.set_state(AppStateEnum::Idle);
    }

    log::info!("Recording cancelled");
}

/// Logical dimensions of the overlay pill window.
const OVERLAY_WIDTH: f64 = 300.0;
const OVERLAY_HEIGHT: f64 = 110.0;

/// Sub-point tolerance for "did the user drag the overlay?" detection.
/// Hide-time frame positions are compared against the value we set at
/// show-time; anything above this epsilon counts as a user drag and gets
/// persisted. See docs/specs/2026-04-11-overlay-positioning-multi-monitor-fix.md §5.7.
const DRAG_EPSILON: f64 = 0.5;

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

                // Park the hidden panel at a safe default (bottom-center
                // of the primary display). The first real show_overlay
                // will overwrite this, but parking the panel now
                // guarantees that if something skips positioning it
                // still appears on a sane screen. AppState.overlay_session
                // is intentionally left as None — precreation does not
                // open a user-visible session.
                let screens = get_native_screens();
                if let Some(primary) = screens.first() {
                    let _ = position_overlay_native(panel.as_panel(), primary);
                }

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
///
/// Positioning uses native macOS APIs (NSScreen, CGWindowList, setFrameOrigin:)
/// to bypass Tauri's buggy multi-monitor coordinate handling.
///
/// Ordering matters: we position the panel *while it is hidden* and only
/// then call `panel.show()`. Calling `setFrameOrigin:` on a visible
/// NSPanel whose frame lives on a different NSScreen is a known-flaky
/// pattern — see
/// docs/specs/2026-04-11-overlay-positioning-multi-monitor-fix.md §3
/// Defect C. If the panel is already visible (second and later
/// recordings) we `orderOut:` it first, so the transport is invisible.
fn show_overlay(app: &AppHandle) {
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        // Determine which screen to show the overlay on.
        // Uses the focused app's window first, then mouse cursor, then primary.
        let state = app.state::<AppState>();
        let target_pid = state.target_pid.load(Ordering::SeqCst);
        let target_screen = select_target_screen(target_pid);

        // Try to show an existing panel first
        if let Ok(panel) = app.get_webview_panel("overlay") {
            // 1. Hide first if currently visible so that setFrameOrigin
            //    does not have to perform a cross-display transport on a
            //    visible window.
            if panel_is_visible(panel.as_panel()) {
                unsafe {
                    let nil: Option<&tauri_nspanel::objc2_foundation::NSObject> = None;
                    let _: () = tauri_nspanel::objc2::msg_send![
                        panel.as_panel(), orderOut: nil
                    ];
                }
            }

            // 2. Position while hidden. Record the session state so that
            //    hide_overlay can tell user drags from auto-defaults.
            if let Some(ref screen) = target_screen {
                let session = position_overlay_native(panel.as_panel(), screen);
                *state.overlay_session.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(session);
            }

            // 3. Show.
            panel.show();

            // 4. Re-apply floating level — required to fix Tauri issue
            //    #13530 where the setting is lost after hide/show.
            use tauri_nspanel::PanelLevel;
            panel.set_level(PanelLevel::Floating.into());
            panel.set_floating_panel(true);
            panel.order_front_regardless();

            // 5. Verify the panel actually landed on the target screen;
            //    re-apply once if the window server moved it.
            if let Some(ref screen) = target_screen {
                if let Some(session) = *state
                    .overlay_session
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                {
                    verify_and_fix_overlay_frame(panel.as_panel(), screen, session);
                }
            }

            log::info!("Overlay shown (existing panel, floating level reapplied)");
            return;
        }

        // Fallback: check for a regular webview window (handles case where panel
        // conversion failed in the past and we have a plain window instead).
        if let Some(window) = app.get_webview_window("overlay") {
            let _ = window.show();
            // Best-effort positioning via Tauri for non-panel fallback
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

                // Position BEFORE showing; record session state.
                if let Some(ref screen) = target_screen {
                    let session = position_overlay_native(panel.as_panel(), screen);
                    *state.overlay_session.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some(session);
                }

                panel.show();

                // Verify post-show placement.
                if let Some(ref screen) = target_screen {
                    if let Some(session) = *state
                        .overlay_session
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                    {
                        verify_and_fix_overlay_frame(panel.as_panel(), screen, session);
                    }
                }

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
///
/// Persistence policy: we save the overlay's frame *only* if the user
/// actually dragged it away from the position we placed it at in
/// `show_overlay`. An auto-computed default that was never touched must
/// not be persisted, because the display arrangement might change before
/// the next show and the stored absolute coordinate would then correspond
/// to the wrong visual spot on the display it was keyed against.
/// See docs/specs/2026-04-11-overlay-positioning-multi-monitor-fix.md §5.7.
fn hide_overlay(app: &AppHandle) {
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        // Reset overlay state (timer, waveform, isRecording) so the next
        // recording starts fresh with a false→true transition.
        if let Some(window) = app.get_webview_window("overlay") {
            let _ = window.eval("window.__resetOverlay && window.__resetOverlay()");
        }

        if let Ok(panel) = app.get_webview_panel("overlay") {
            let panel_ref = panel.as_panel();
            let frame: tauri_nspanel::objc2_foundation::NSRect = unsafe {
                tauri_nspanel::objc2::msg_send![panel_ref, frame]
            };

            let state = app.state::<AppState>();
            let session = state
                .overlay_session
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();

            if let Some(sess) = session {
                let dx = (frame.origin.x - sess.applied_origin.0).abs();
                let dy = (frame.origin.y - sess.applied_origin.1).abs();
                let moved = dx > DRAG_EPSILON || dy > DRAG_EPSILON;

                if moved {
                    // User dragged. Persist against whichever display
                    // now contains the overlay's center — matches the
                    // macOS "majority geometry" rule for spanning windows.
                    let center_x = frame.origin.x + frame.size.width / 2.0;
                    let center_y = frame.origin.y + frame.size.height / 2.0;
                    let screens = get_native_screens();
                    if let Some(idx) =
                        screen_containing_point(&screens, center_x, center_y)
                    {
                        save_panel_position(panel_ref, screens[idx].display_id);
                    } else {
                        log::info!(
                            "Overlay was dragged off all screens; cannot persist \
                             ({:.0},{:.0})",
                            frame.origin.x, frame.origin.y
                        );
                    }
                } else {
                    log::info!(
                        "Overlay was not moved during session — not persisting \
                         (default {:.0},{:.0} for display {})",
                        sess.default_origin.0, sess.default_origin.1, sess.display_id
                    );
                }
            } else {
                log::info!("No overlay session — skipping persistence on hide");
            }

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

// ---------------------------------------------------------------------------
// Native multi-monitor overlay positioning
//
// Bypasses Tauri's buggy monitor/positioning APIs (see tauri#10980, #7890,
// #14825) by using macOS native APIs directly:
//   - NSScreen.screens        → screen enumeration
//   - CGWindowListCopyWindowInfo → focused-app window bounds
//   - NSEvent.mouseLocation   → cursor position
//   - NSPanel.setFrameOrigin: → window placement
//
// All coordinates stay in Cocoa's coordinate system (logical points, origin
// at bottom-left of primary display) to avoid cross-system conversion bugs.
//
// The overlay is draggable. User-chosen positions are remembered per monitor
// (keyed by CGDirectDisplayID) and restored on subsequent recordings.
// ---------------------------------------------------------------------------

/// Current persistence schema version. Bumped to 2 so that the loader can
/// reject any entry written by a pre-fix build — those entries were
/// auto-computed defaults that `hide_overlay` should never have persisted.
/// See docs/specs/2026-04-11-overlay-positioning-multi-monitor-fix.md §5.1.
const OVERLAY_POSITION_SCHEMA: u32 = 2;

/// Schema value inferred for entries that predate the schema field. Old
/// entries deserialize with this value and are rejected at load.
fn legacy_schema_version() -> u32 { 1 }

/// Saved overlay position for a specific monitor. Only user-dragged
/// positions are persisted; auto-computed defaults are not.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct SavedOverlayPosition {
    x: f64,
    y: f64,
    /// Schema version. Writers always set this to `OVERLAY_POSITION_SCHEMA`.
    #[serde(default = "legacy_schema_version")]
    schema: u32,
}

/// Map from display_id (as string) to saved position.
type OverlayPositions = std::collections::HashMap<String, SavedOverlayPosition>;

fn overlay_positions_path() -> Option<std::path::PathBuf> {
    let data_dir = dirs::data_dir()?;
    let app_dir = data_dir.join("com.sottoasr.app");
    std::fs::create_dir_all(&app_dir).ok()?;
    Some(app_dir.join("overlay_positions.json"))
}

fn load_overlay_positions() -> OverlayPositions {
    let path = match overlay_positions_path() {
        Some(p) if p.exists() => p,
        _ => return OverlayPositions::new(),
    };
    let mut positions: OverlayPositions = match std::fs::read_to_string(&path) {
        Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
        Err(_) => OverlayPositions::new(),
    };
    let before = positions.len();
    positions.retain(|_, v| v.schema >= OVERLAY_POSITION_SCHEMA);
    let dropped = before - positions.len();
    if dropped > 0 {
        log::info!(
            "Dropped {} legacy overlay-position entries (schema < {})",
            dropped, OVERLAY_POSITION_SCHEMA
        );
    }
    positions
}

fn save_overlay_positions(positions: &OverlayPositions) {
    if let Some(path) = overlay_positions_path() {
        if let Ok(data) = serde_json::to_string_pretty(positions) {
            let _ = std::fs::write(&path, data);
        }
    }
}

/// Save the current panel position for a specific display. This is the
/// *only* write site; every entry it creates carries
/// `OVERLAY_POSITION_SCHEMA` so that the loader accepts it.
fn save_panel_position(panel: &tauri_nspanel::objc2_app_kit::NSPanel, display_id: u32) {
    // Build-time guard: bumping the schema should force a loader update.
    debug_assert_eq!(
        OVERLAY_POSITION_SCHEMA, 2,
        "bump OVERLAY_POSITION_SCHEMA and extend the load filter together"
    );
    let frame: tauri_nspanel::objc2_foundation::NSRect = unsafe {
        tauri_nspanel::objc2::msg_send![panel, frame]
    };
    let mut positions = load_overlay_positions();
    positions.insert(display_id.to_string(), SavedOverlayPosition {
        x: frame.origin.x,
        y: frame.origin.y,
        schema: OVERLAY_POSITION_SCHEMA,
    });
    save_overlay_positions(&positions);
    log::info!(
        "Saved overlay position ({:.0}, {:.0}) for display {} (schema {})",
        frame.origin.x, frame.origin.y, display_id, OVERLAY_POSITION_SCHEMA
    );
}

/// Look up a saved position for this display. Returns `None` if the saved
/// point does not lie fully inside the current `visibleFrame` — i.e. the
/// display arrangement has changed since the position was persisted. A
/// saved entry that would need clamping is, by definition, stale; we
/// discard it rather than pin it to a visible edge.
fn get_saved_position(
    display_id: u32,
    visible: &tauri_nspanel::objc2_foundation::NSRect,
) -> Option<(f64, f64)> {
    let positions = load_overlay_positions();
    let saved = positions.get(&display_id.to_string())?;

    let fits_x = saved.x >= visible.origin.x
        && saved.x + OVERLAY_WIDTH <= visible.origin.x + visible.size.width;
    let fits_y = saved.y >= visible.origin.y
        && saved.y + OVERLAY_HEIGHT <= visible.origin.y + visible.size.height;

    if !fits_x || !fits_y {
        log::info!(
            "Discarding stale saved position ({:.0},{:.0}) for display {} — \
             does not fit current visibleFrame ({:.0},{:.0} {:.0}x{:.0})",
            saved.x, saved.y, display_id,
            visible.origin.x, visible.origin.y, visible.size.width, visible.size.height
        );
        return None;
    }

    log::info!(
        "Restored overlay position ({:.0},{:.0}) for display {}",
        saved.x, saved.y, display_id
    );
    Some((saved.x, saved.y))
}

/// Native screen info in Cocoa coordinates (logical points, origin bottom-left).
#[derive(Clone, Copy)]
struct NativeScreen {
    frame: tauri_nspanel::objc2_foundation::NSRect,
    visible_frame: tauri_nspanel::objc2_foundation::NSRect,
    scale_factor: f64,
    /// CGDirectDisplayID — unique, stable identifier for a physical monitor.
    display_id: u32,
}

/// Get all connected screens via [NSScreen screens].
/// Returns frames in Cocoa coordinates (points, origin at bottom-left of primary).
fn get_native_screens() -> Vec<NativeScreen> {
    unsafe {
        let screens: *const tauri_nspanel::objc2_foundation::NSArray<
            tauri_nspanel::objc2_app_kit::NSScreen,
        > = tauri_nspanel::objc2::msg_send![
            tauri_nspanel::objc2::class!(NSScreen),
            screens
        ];
        let count: usize = tauri_nspanel::objc2::msg_send![&*screens, count];
        let mut result = Vec::with_capacity(count);
        for i in 0..count {
            let screen: *const tauri_nspanel::objc2_app_kit::NSScreen =
                tauri_nspanel::objc2::msg_send![&*screens, objectAtIndex: i];
            let frame: tauri_nspanel::objc2_foundation::NSRect =
                tauri_nspanel::objc2::msg_send![screen, frame];
            let visible: tauri_nspanel::objc2_foundation::NSRect =
                tauri_nspanel::objc2::msg_send![screen, visibleFrame];
            let scale: f64 =
                tauri_nspanel::objc2::msg_send![screen, backingScaleFactor];
            // Extract CGDirectDisplayID from NSScreen.deviceDescription[@"NSScreenNumber"]
            let desc: *const tauri_nspanel::objc2_foundation::NSDictionary<
                tauri_nspanel::objc2_foundation::NSString,
                tauri_nspanel::objc2_foundation::NSObject,
            > = tauri_nspanel::objc2::msg_send![screen, deviceDescription];
            let key = tauri_nspanel::objc2_foundation::NSString::from_str("NSScreenNumber");
            let num_obj: *const tauri_nspanel::objc2_foundation::NSObject =
                tauri_nspanel::objc2::msg_send![&*desc, objectForKey: &*key];
            let display_id: u32 = if !num_obj.is_null() {
                tauri_nspanel::objc2::msg_send![num_obj, unsignedIntValue]
            } else {
                0
            };
            result.push(NativeScreen { frame, visible_frame: visible, scale_factor: scale, display_id });
        }
        result
    }
}

/// Get the mouse location in Cocoa coordinates (points, origin bottom-left).
fn get_mouse_location_cocoa() -> tauri_nspanel::objc2_foundation::NSPoint {
    unsafe {
        tauri_nspanel::objc2::msg_send![
            tauri_nspanel::objc2::class!(NSEvent),
            mouseLocation
        ]
    }
}

/// Get the bounds of the frontmost on-screen window belonging to `pid`.
/// Returns (x, y, width, height) in Quartz coordinates (origin top-left of primary).
fn get_frontmost_window_bounds(pid: i32) -> Option<(f64, f64, f64, f64)> {
    use core_graphics::window::*;
    use core_graphics::geometry::CGRect;
    use core_foundation::base::TCFType;

    extern "C" {
        fn CGRectMakeWithDictionaryRepresentation(
            dict: core_foundation::dictionary::CFDictionaryRef,
            rect: *mut CGRect,
        ) -> bool;
    }

    let windows = copy_window_info(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        kCGNullWindowID,
    )?;

    let arr_ref = windows.as_concrete_TypeRef();
    let len = unsafe { core_foundation::array::CFArrayGetCount(arr_ref) };

    for i in 0..len {
        unsafe {
            let dict_ptr = core_foundation::array::CFArrayGetValueAtIndex(arr_ref, i)
                as core_foundation::dictionary::CFDictionaryRef;
            if dict_ptr.is_null() { continue; }

            // Helper: read a numeric value from the window-info dictionary.
            let get_i32 = |key: core_foundation::string::CFStringRef| -> i32 {
                let mut value: *const std::ffi::c_void = std::ptr::null();
                if core_foundation::dictionary::CFDictionaryGetValueIfPresent(
                    dict_ptr, key as *const std::ffi::c_void, &mut value,
                ) != 0 && !value.is_null() {
                    let num = core_foundation::number::CFNumber::wrap_under_get_rule(
                        value as core_foundation::number::CFNumberRef,
                    );
                    num.to_i32().unwrap_or(0)
                } else {
                    0
                }
            };

            let owner_pid = get_i32(kCGWindowOwnerPID);
            let layer = get_i32(kCGWindowLayer);

            if owner_pid == pid && layer == 0 {
                let mut bounds_val: *const std::ffi::c_void = std::ptr::null();
                if core_foundation::dictionary::CFDictionaryGetValueIfPresent(
                    dict_ptr, kCGWindowBounds as *const std::ffi::c_void, &mut bounds_val,
                ) != 0 && !bounds_val.is_null() {
                    let mut rect = CGRect::new(
                        &core_graphics::geometry::CGPoint::new(0.0, 0.0),
                        &core_graphics::geometry::CGSize::new(0.0, 0.0),
                    );
                    if CGRectMakeWithDictionaryRepresentation(
                        bounds_val as core_foundation::dictionary::CFDictionaryRef,
                        &mut rect,
                    ) {
                        return Some((
                            rect.origin.x, rect.origin.y,
                            rect.size.width, rect.size.height,
                        ));
                    }
                }
            }
        }
    }
    None
}

/// Find which screen contains the given point (Cocoa coordinates).
fn screen_containing_point(screens: &[NativeScreen], x: f64, y: f64) -> Option<usize> {
    screens.iter().position(|s| {
        x >= s.frame.origin.x
            && x < s.frame.origin.x + s.frame.size.width
            && y >= s.frame.origin.y
            && y < s.frame.origin.y + s.frame.size.height
    })
}

/// Select the screen where the overlay should appear.
///
/// Priority: 1) screen with focused app's window  2) mouse cursor  3) primary
fn select_target_screen(target_pid: i32) -> Option<NativeScreen> {
    let screens = get_native_screens();
    if screens.is_empty() {
        log::error!("No screens detected — cannot position overlay");
        return None;
    }

    log_screen_configuration(&screens);

    // 1. Try the screen containing the focused app's frontmost window
    if target_pid > 0 {
        if let Some((bx, by, bw, bh)) = get_frontmost_window_bounds(target_pid) {
            let center_x = bx + bw / 2.0;
            let center_y_quartz = by + bh / 2.0;
            // Convert Quartz Y (top-left origin) → Cocoa Y (bottom-left origin)
            let primary_h = screens[0].frame.size.height;
            let center_y_cocoa = primary_h - center_y_quartz;
            if let Some(idx) = screen_containing_point(&screens, center_x, center_y_cocoa) {
                log::info!(
                    "Overlay target: screen {} (focused app PID {}, window center {:.0},{:.0})",
                    idx, target_pid, center_x, center_y_cocoa
                );
                return Some(screens[idx]);
            }
            log::warn!(
                "Focused app PID {} window center ({:.0},{:.0}) not on any screen",
                target_pid, center_x, center_y_cocoa
            );
        } else {
            log::info!("No on-screen windows found for PID {}", target_pid);
        }
    }

    // 2. Fall back to the screen under the mouse cursor
    let mouse = get_mouse_location_cocoa();
    if let Some(idx) = screen_containing_point(&screens, mouse.x, mouse.y) {
        log::info!("Overlay target: screen {} (mouse cursor fallback at {:.0},{:.0})", idx, mouse.x, mouse.y);
        return Some(screens[idx]);
    }

    // 3. Last resort: primary screen (screens[0] = menu-bar screen)
    log::info!("Overlay target: primary screen (final fallback)");
    Some(screens[0])
}

/// Position the overlay on the target screen using native Cocoa APIs.
///
/// Always computes the default bottom-center position; if the user has a
/// valid saved position for this display, uses that instead. Returns an
/// `OverlaySession` describing both origins so the caller can record it
/// on `AppState.overlay_session` and later detect whether the user
/// dragged the panel.
fn position_overlay_native(
    panel: &tauri_nspanel::objc2_app_kit::NSPanel,
    target: &NativeScreen,
) -> OverlaySession {
    let vis = &target.visible_frame;
    let margin_bottom: f64 = 8.0;

    // Default: centered horizontally, just above the Dock.
    let default_origin = (
        vis.origin.x + (vis.size.width - OVERLAY_WIDTH) / 2.0,
        vis.origin.y + margin_bottom,
    );

    let (x, y) = match get_saved_position(target.display_id, vis) {
        Some(saved) => saved,
        None        => default_origin,
    };

    unsafe {
        let origin = tauri_nspanel::objc2_foundation::NSPoint { x, y };
        let _: () = tauri_nspanel::objc2::msg_send![panel, setFrameOrigin: origin];
    }

    log::info!(
        "Overlay positioned at ({:.0},{:.0}) on display {} — default=({:.0},{:.0}) visible=({:.0},{:.0} {:.0}x{:.0})",
        x, y, target.display_id,
        default_origin.0, default_origin.1,
        vis.origin.x, vis.origin.y, vis.size.width, vis.size.height,
    );

    OverlaySession {
        display_id: target.display_id,
        default_origin,
        applied_origin: (x, y),
    }
}

/// True if the NSPanel's `isVisible` flag is set.
fn panel_is_visible(panel: &tauri_nspanel::objc2_app_kit::NSPanel) -> bool {
    unsafe {
        let visible: tauri_nspanel::objc2::runtime::Bool =
            tauri_nspanel::objc2::msg_send![panel, isVisible];
        visible.as_bool()
    }
}

/// Read the panel's actual frame after show and check it lies on the
/// target screen. If not (can happen when the window server decides to
/// re-assign the window to the "majority" display), re-apply once.
///
/// Checked against `target.frame` (not `visible_frame`) on purpose: the
/// goal is "did the panel land on the correct *screen*", not "is every
/// pixel inside the Dock-excluded area". A user who later drags the
/// panel into the Dock zone should still have their position honored.
fn verify_and_fix_overlay_frame(
    panel: &tauri_nspanel::objc2_app_kit::NSPanel,
    target: &NativeScreen,
    session: OverlaySession,
) {
    let frame: tauri_nspanel::objc2_foundation::NSRect = unsafe {
        tauri_nspanel::objc2::msg_send![panel, frame]
    };
    let center_x = frame.origin.x + frame.size.width / 2.0;
    let center_y = frame.origin.y + frame.size.height / 2.0;

    let tf = &target.frame;
    let inside = center_x >= tf.origin.x
        && center_x <  tf.origin.x + tf.size.width
        && center_y >= tf.origin.y
        && center_y <  tf.origin.y + tf.size.height;

    if inside {
        return;
    }

    log::warn!(
        "Overlay frame landed off-target after show (center {:.0},{:.0}, \
         target display {} frame {:.0},{:.0} {:.0}x{:.0}) — re-applying",
        center_x, center_y, target.display_id,
        tf.origin.x, tf.origin.y, tf.size.width, tf.size.height,
    );

    // Re-apply using the session's applied_origin rather than recomputing
    // so that we do not "demote" a valid user position to the default.
    unsafe {
        let origin = tauri_nspanel::objc2_foundation::NSPoint {
            x: session.applied_origin.0,
            y: session.applied_origin.1,
        };
        let _: () = tauri_nspanel::objc2::msg_send![panel, setFrameOrigin: origin];
    }
}

/// Log the current screen configuration for diagnostics.
fn log_screen_configuration(screens: &[NativeScreen]) {
    log::info!("=== Screen Configuration ({} screens) ===", screens.len());
    for (i, s) in screens.iter().enumerate() {
        log::info!(
            "  Screen {}: display={} frame=({:.0},{:.0} {:.0}x{:.0}) visible=({:.0},{:.0} {:.0}x{:.0}) scale={}",
            i, s.display_id,
            s.frame.origin.x, s.frame.origin.y,
            s.frame.size.width, s.frame.size.height,
            s.visible_frame.origin.x, s.visible_frame.origin.y,
            s.visible_frame.size.width, s.visible_frame.size.height,
            s.scale_factor,
        );
    }
}
