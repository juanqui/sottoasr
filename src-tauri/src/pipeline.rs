use crate::llm::cleanup::run_cleanup;
use crate::models::{AppStateEnum, LlmCleanupStatus, Transcription};
use crate::state::AppState;

/// Maximum sample rate we expect to handle (96kHz for high-quality audio).
const MAX_EXPECTED_SAMPLE_RATE_HZ: usize = 96_000;
/// Maximum recording duration before auto-stop (20 minutes).
/// Raised from 12 min in the 2026-04-11 reliability spec — see
/// docs/specs/2026-04-11-llm-cleanup-reliability.md §4.2.
const MAX_RECORDING_SECS: u64 = 20 * 60;
/// Maximum audio buffer size (MAX_RECORDING_SECS at max expected sample rate).
const MAX_AUDIO_BUFFER_SAMPLES: usize = MAX_EXPECTED_SAMPLE_RATE_HZ * MAX_RECORDING_SECS as usize;

/// Events emitted during pipeline execution.
/// In production these map to Tauri app.emit() calls.
/// In tests they are collected for assertion.
pub trait PipelineEvents: Send + Sync {
    fn emit_state_changed(&self, state: &AppStateEnum);
    fn emit_recording_started(&self);
    fn emit_recording_stopped(&self);
    fn emit_recording_cancelled(&self);
    fn emit_transcription_complete(&self, transcription: &Transcription);
    fn emit_transcription_error(&self, error: &str);
    fn emit_paste_complete(&self, id: &str);
    fn emit_paste_error(&self, error: &str, text: &str);
    fn emit_audio_level(&self, level: f32);
    fn emit_recording_error(&self, error: &str);
}

/// Start recording: acquire mic, set state.
/// Returns Ok(()) if recording started, Err if mic failed or wrong state.
///
/// The `level_callback` is invoked by the audio backend at ~30 Hz with the
/// current RMS level. In production, the caller creates a callback that
/// emits Tauri events. In tests, pass a no-op: `Box::new(|_| {})`.
///
/// Note: this does NOT handle overlay show/hide or cancel shortcut
/// registration -- those are Tauri-specific concerns handled by the caller.
pub fn pipeline_start_recording(
    state: &AppState,
    events: &dyn PipelineEvents,
    level_callback: Box<dyn Fn(f32) + Send + 'static>,
) -> Result<(), String> {
    let current = state.get_state();
    if current != AppStateEnum::Idle {
        return Err(format!("Cannot start recording: currently in {:?} state", current));
    }

    // Drain leftover samples
    if let Ok(rx) = state.audio_receiver.lock() {
        while rx.try_recv().is_ok() {}
    }

    // Start audio capture
    let sender = state.audio_sender.lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();

    let is_recording = std::sync::Arc::new(
        std::sync::atomic::AtomicBool::new(true)
    );
    state.is_recording.store(true, std::sync::atomic::Ordering::SeqCst);

    let is_recording_clone = is_recording.clone();

    let start_result = {
        let mut capture = state.audio_capture.lock()
            .unwrap_or_else(|e| e.into_inner());
        capture.start(sender, is_recording_clone, level_callback)
    };

    match start_result {
        Ok(()) => {
            let target_pid = state.paste_backend.get_frontmost_pid();
            state.target_pid.store(target_pid, std::sync::atomic::Ordering::SeqCst);

            state.set_state(AppStateEnum::Recording);
            events.emit_recording_started();
            events.emit_state_changed(&AppStateEnum::Recording);
            Ok(())
        }
        Err(e) => {
            state.is_recording.store(false, std::sync::atomic::Ordering::SeqCst);
            events.emit_recording_error(&e);
            Err(e)
        }
    }
}

/// Stop recording: collect samples, transcribe, optionally clean up, paste.
///
/// All intermediate results (transcription, paste status) are communicated
/// via the `events` trait and `state.last_transcription`. The return value
/// signals only whether the pipeline completed without a fatal error.
pub async fn pipeline_stop_recording(
    state: &AppState,
    events: &dyn PipelineEvents,
) -> Result<(), String> {
    // 1. Guard: check state is Recording
    let current = state.get_state();
    if current != AppStateEnum::Recording {
        return Err(format!("Cannot stop recording: currently in {:?} state", current));
    }

    // 2. Stop capture
    {
        let mut capture = state.audio_capture.lock().unwrap_or_else(|e| e.into_inner());
        capture.stop();
    }
    state.is_recording.store(false, std::sync::atomic::Ordering::SeqCst);

    // 3. Transition to Transcribing
    state.set_state(AppStateEnum::Transcribing);
    events.emit_recording_stopped();
    events.emit_state_changed(&AppStateEnum::Transcribing);

    // 4. Collect samples from channel
    let samples = {
        let rx = state.audio_receiver.lock().unwrap_or_else(|e| e.into_inner());
        let mut all = Vec::new();
        while let Ok(chunk) = rx.try_recv() {
            if all.len().saturating_add(chunk.len()) > MAX_AUDIO_BUFFER_SAMPLES {
                state.set_state(AppStateEnum::Idle);
                events.emit_state_changed(&AppStateEnum::Idle);
                return Err("Recording too long".into());
            }
            all.extend(chunk);
        }
        all
    };

    // 5. Short recording check
    if samples.len() < 4000 {
        log::warn!("Recording too short ({} samples), discarding", samples.len());
        state.set_state(AppStateEnum::Idle);
        events.emit_state_changed(&AppStateEnum::Idle);
        return Ok(());
    }

    // 6. Get sample rate from AudioCaptureBackend trait
    let sample_rate = {
        let capture = state.audio_capture.lock().unwrap_or_else(|e| e.into_inner());
        capture.sample_rate()
    };

    // 7. Write WAV to temp file
    let temp_path = std::env::temp_dir().join(format!("sotto_{}.wav", uuid::Uuid::new_v4()));

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
            // Append trailing silence so the ASR model fully processes the final chunk
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
            events.emit_state_changed(&AppStateEnum::Idle);
            events.emit_transcription_error(&format!("WAV write failed: {}", e));
            return Err(format!("WAV write failed: {}", e));
        }
    }

    let temp_path_str = temp_path.to_string_lossy().to_string();

    // 8. Assign job ID
    let job_id = state.new_job();

    // 9. Transcribe via ASR engine
    let mut engine = state.asr_engine.lock().await;
    log::info!("Starting transcription...");
    let result = engine.transcribe_file(&temp_path_str);
    drop(engine);

    // Clean up temp file
    let _ = std::fs::remove_file(&temp_path);

    // 10. Handle ASR result
    match result {
        Ok(asr_result) => {
            log::info!("Transcription result: \"{}\" (RTF: {:.1}x)", &asr_result.text, asr_result.rtfx);

            // 10a. Check job staleness
            if !state.is_current_job(job_id) {
                log::info!("Job {} is stale, discarding transcription", job_id);
                state.set_state(AppStateEnum::Idle);
                events.emit_state_changed(&AppStateEnum::Idle);
                return Ok(());
            }

            let raw_asr_text = asr_result.text.clone();
            let mut final_text = asr_result.text.clone();
            let mut llm_was_applied = false;
            let cleanup_status: LlmCleanupStatus;

            // 10b. Read settings
            let settings = state.settings.lock().await;
            let llm_enabled = settings.llm_cleanup_enabled;
            let auto_paste = settings.auto_paste;
            let restore_clipboard = settings.restore_clipboard;
            let restore_focus_before_paste = settings.restore_focus_before_paste;
            drop(settings);

            // 10c. LLM cleanup if enabled. The shared `run_cleanup()` helper
            // owns the spawn-or-reuse, timeout, kill-orphan, and zombie-handle
            // detection logic. See docs/specs/2026-04-11-llm-cleanup-reliability.md.
            if llm_enabled {
                state.set_state(AppStateEnum::CleaningUp);
                events.emit_state_changed(&AppStateEnum::CleaningUp);

                let (cleaned, status) = run_cleanup(state, &final_text).await;
                cleanup_status = status;
                if matches!(cleanup_status, LlmCleanupStatus::Applied { .. }) {
                    final_text = cleaned;
                    llm_was_applied = true;
                }
            } else {
                cleanup_status = LlmCleanupStatus::Disabled;
            }

            // Cache the latest cleanup status on AppState so the frontend
            // can read it via get_llm_status. The hotkey path also emits a
            // Tauri event in addition to caching — pipeline.rs is test-only
            // so the cache is the only surface here.
            {
                let mut last = state.llm_last_status.lock().await;
                *last = cleanup_status.clone();
            }

            // 10d. Second staleness check
            if !state.is_current_job(job_id) {
                log::info!("Job {} is stale after cleanup, discarding", job_id);
                state.set_state(AppStateEnum::Idle);
                events.emit_state_changed(&AppStateEnum::Idle);
                return Ok(());
            }

            // 10e. Build Transcription struct
            let transcription = Transcription {
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

            // 10f. Save to state.last_transcription + add_transcription
            {
                let mut last = state.last_transcription.lock().await;
                *last = Some(transcription.clone());
            }
            crate::commands::transcription::add_transcription(transcription.clone()).await;

            events.emit_transcription_complete(&transcription);

            // 10g. Paste or copy to clipboard
            if !final_text.trim().is_empty() {
                if auto_paste {
                    let target_pid = if restore_focus_before_paste {
                        let start_pid = state.target_pid.load(std::sync::atomic::Ordering::SeqCst);
                        let current_pid = state.paste_backend.get_frontmost_pid();
                        let our_pid = std::process::id() as i32;

                        if current_pid == start_pid || current_pid == our_pid || current_pid == 0 {
                            start_pid
                        } else {
                            log::info!("User switched apps during recording: {} -> {}, pasting at current", start_pid, current_pid);
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
                            events.emit_paste_complete(&transcription.id);
                        }
                        Err(e) => {
                            log::error!("Paste failed: {}", e);
                            events.emit_paste_error(&e, &final_text);
                            let _ = state.paste_backend.copy_to_clipboard(&final_text);
                            log::info!("Text copied to clipboard as fallback");
                        }
                    }
                } else {
                    match state.paste_backend.copy_to_clipboard(&final_text) {
                        Ok(()) => {
                            log::info!("Text copied to clipboard (auto_paste disabled)");
                            events.emit_paste_complete(&transcription.id);
                        }
                        Err(e) => {
                            log::error!("Clipboard copy failed: {}", e);
                            events.emit_paste_error(&e, &final_text);
                        }
                    }
                }
            }
        }
        Err(e) => {
            log::error!("Transcription failed: {}", e);
            events.emit_transcription_error(&e);
        }
    }

    state.set_state(AppStateEnum::Idle);
    events.emit_state_changed(&AppStateEnum::Idle);
    Ok(())
}

/// Cancel recording: stop mic, optionally transcribe, save as cancelled, don't paste.
pub async fn pipeline_cancel_recording(
    state: &AppState,
    events: &dyn PipelineEvents,
) -> Option<Transcription> {
    let current = state.get_state();
    if current != AppStateEnum::Recording {
        log::warn!("Cannot cancel recording: currently in {:?} state", current);
        return None;
    }

    // Stop audio capture
    {
        let mut capture = state.audio_capture.lock().unwrap_or_else(|e| e.into_inner());
        capture.stop();
    }
    state.is_recording.store(false, std::sync::atomic::Ordering::SeqCst);

    events.emit_recording_cancelled();
    events.emit_state_changed(&AppStateEnum::Idle);

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
    log::info!("Recording cancelled -- {} samples collected", sample_count);

    // If we have enough audio, transcribe it and save as cancelled
    if sample_count >= 4000 {
        let sample_rate = {
            let capture = state.audio_capture.lock().unwrap_or_else(|e| e.into_inner());
            capture.sample_rate()
        };

        let temp_path = std::env::temp_dir().join(format!("sotto_{}.wav", uuid::Uuid::new_v4()));

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

            let mut engine = state.asr_engine.lock().await;
            let result = engine.transcribe_file(&temp_path_str);
            drop(engine);
            let _ = std::fs::remove_file(&temp_path);

            if let Ok(asr_result) = result {
                let transcription = Transcription {
                    id: uuid::Uuid::new_v4().to_string(),
                    text: asr_result.text.clone(),
                    duration_ms: (asr_result.duration_secs * 1000.0) as u64,
                    created_at: chrono::Utc::now(),
                    word_count: asr_result.text.split_whitespace().count(),
                    cancelled: true,
                    raw_text: None,
                    llm_applied: false,
                    llm_cleanup_status: LlmCleanupStatus::Idle,
                };
                crate::commands::transcription::add_transcription(transcription.clone()).await;
                events.emit_transcription_complete(&transcription);
                log::info!("Cancelled transcription saved: \"{}\"",
                    &asr_result.text[..asr_result.text.len().min(50)]);

                state.set_state(AppStateEnum::Idle);
                return Some(transcription);
            }
        }

        state.set_state(AppStateEnum::Idle);
        None
    } else {
        // Too short to transcribe -- just save a placeholder
        let transcription = Transcription {
            id: uuid::Uuid::new_v4().to_string(),
            text: String::new(),
            duration_ms: (sample_count as u64 * 1000) / 48000,
            created_at: chrono::Utc::now(),
            word_count: 0,
            cancelled: true,
            raw_text: None,
            llm_applied: false,
            llm_cleanup_status: LlmCleanupStatus::Idle,
        };
        crate::commands::transcription::add_transcription(transcription.clone()).await;
        state.set_state(AppStateEnum::Idle);
        Some(transcription)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use crate::models::Settings;
    use std::sync::Arc;

    /// Helper: create an AppState with standard mock backends.
    fn test_state(
        audio: MockAudioCapture,
        asr: MockAsrEngine,
        llm: Option<MockLlmBackend>,
        paste: Box<dyn crate::paste::PasteBackend>,
    ) -> AppState {
        let llm_enabled = llm.is_some();
        let settings = Settings {
            auto_paste: true,
            restore_clipboard: true,
            restore_focus_before_paste: true,
            llm_cleanup_enabled: llm_enabled,
            ..Default::default()
        };

        AppState::new_with_backends(
            Box::new(audio),
            Box::new(asr),
            llm.map(|l| Box::new(l) as Box<dyn crate::llm::engine::LlmBackend>),
            paste,
            settings,
        )
    }

    // --- Test 1: Happy path with LLM cleanup ---
    #[tokio::test]
    #[serial_test::serial]
    async fn test_full_pipeline_happy_path() {
        let mock_paste = Arc::new(MockPasteBackend::new());
        let state = test_state(
            MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("hello world this is a test sentence"),
            Some(MockLlmBackend::fixed("Hello, world. This is a test sentence.")),
            Box::new(SharedMockPaste(mock_paste.clone())),
        );
        let events = CollectingEvents::new();

        // Start recording
        pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
        assert_eq!(state.get_state(), AppStateEnum::Recording);

        // Stop recording -- triggers transcription + LLM + paste
        pipeline_stop_recording(&state, &events).await.unwrap();

        // Assert final state
        assert_eq!(state.get_state(), AppStateEnum::Idle);

        // Assert transcription was saved
        let last = state.last_transcription.lock().await;
        let t = last.as_ref().expect("transcription should be saved");
        assert_eq!(t.text, "Hello, world. This is a test sentence.");
        assert_eq!(t.raw_text.as_deref(), Some("hello world this is a test sentence"));
        assert!(t.llm_applied);
        assert!(!t.cancelled);
        assert!(matches!(t.llm_cleanup_status, LlmCleanupStatus::Applied { .. }));
        // last_status should be cached on AppState too
        assert!(matches!(*state.llm_last_status.lock().await, LlmCleanupStatus::Applied { .. }));

        // Assert paste occurred with cleaned text
        let pasted = mock_paste.last_pasted().expect("should have pasted");
        assert_eq!(pasted.text, "Hello, world. This is a test sentence.");
        assert!(pasted.restore_clipboard);
        assert!(!events.paste_ids.lock().unwrap().is_empty());

        // Assert state transitions
        let states = events.state_changes.lock().unwrap();
        assert!(states.contains(&AppStateEnum::Recording));
        assert!(states.contains(&AppStateEnum::Transcribing));
        assert!(states.contains(&AppStateEnum::CleaningUp));
        assert!(states.contains(&AppStateEnum::Idle));
    }

    // --- Test 2: Pipeline without LLM ---
    #[tokio::test]
    #[serial_test::serial]
    async fn test_pipeline_without_llm() {
        let mock_paste = Arc::new(MockPasteBackend::new());
        let state = test_state(
            MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("hello world"),
            None, // No LLM
            Box::new(SharedMockPaste(mock_paste.clone())),
        );
        let events = CollectingEvents::new();

        pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
        pipeline_stop_recording(&state, &events).await.unwrap();

        // Assert raw ASR text was pasted (no LLM cleanup)
        let pasted = mock_paste.last_pasted().expect("should have pasted");
        assert_eq!(pasted.text, "hello world");

        // Assert transcription has no raw_text (LLM was not used)
        let last = state.last_transcription.lock().await;
        let t = last.as_ref().unwrap();
        assert!(!t.llm_applied);
        assert!(t.raw_text.is_none());
        // With llm_cleanup_enabled=false, status should be Disabled
        assert_eq!(t.llm_cleanup_status, LlmCleanupStatus::Disabled);
    }

    // --- Test 3: ASR error ---
    #[tokio::test]
    #[serial_test::serial]
    async fn test_pipeline_asr_error() {
        let mock_paste = Arc::new(MockPasteBackend::new());
        let state = test_state(
            MockAudioCapture::sine_wave(),
            MockAsrEngine::with_error("Model failed to load"),
            None,
            Box::new(SharedMockPaste(mock_paste.clone())),
        );
        let events = CollectingEvents::new();

        pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
        pipeline_stop_recording(&state, &events).await.unwrap();

        // Assert state returned to Idle
        assert_eq!(state.get_state(), AppStateEnum::Idle);

        // Assert no paste occurred
        assert!(mock_paste.last_pasted().is_none());
        assert!(mock_paste.copied_texts.lock().unwrap().is_empty());

        // Assert error was emitted
        let errors = events.errors.lock().unwrap();
        assert!(errors.iter().any(|e| e.contains("Model failed to load")));
    }

    // --- Test 4: Recording cancellation ---
    #[tokio::test]
    #[serial_test::serial]
    async fn test_pipeline_cancel() {
        let mock_paste = Arc::new(MockPasteBackend::new());
        let state = test_state(
            MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("hello world"),
            None,
            Box::new(SharedMockPaste(mock_paste.clone())),
        );
        let events = CollectingEvents::new();

        pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
        assert_eq!(state.get_state(), AppStateEnum::Recording);

        // Cancel instead of stop
        pipeline_cancel_recording(&state, &events).await;

        // Assert state returned to Idle
        assert_eq!(state.get_state(), AppStateEnum::Idle);

        // Assert no paste occurred
        assert!(mock_paste.last_pasted().is_none());

        // Assert cancellation event was emitted
        assert!(*events.recording_cancelled.lock().unwrap());
    }

    // --- Test 5: State machine guards ---
    #[tokio::test]
    async fn test_start_while_recording_fails() {
        let state = test_state(
            MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("hello"),
            None,
            Box::new(MockPasteBackend::new()),
        );
        let events = CollectingEvents::new();

        // Start recording
        pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
        assert_eq!(state.get_state(), AppStateEnum::Recording);

        // Attempt to start again -- should fail
        let result = pipeline_start_recording(&state, &events, Box::new(|_| {}));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Cannot start recording"));
    }

    #[tokio::test]
    async fn test_stop_while_idle_is_err() {
        let state = test_state(
            MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("hello"),
            None,
            Box::new(MockPasteBackend::new()),
        );
        let events = CollectingEvents::new();

        // State is Idle, stopping should return an error
        assert_eq!(state.get_state(), AppStateEnum::Idle);
        let result = pipeline_stop_recording(&state, &events).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Cannot stop recording"));
        assert_eq!(state.get_state(), AppStateEnum::Idle);
    }

    // --- Test 6: Job ID staleness ---
    #[tokio::test]
    #[serial_test::serial]
    async fn test_stale_job_discarded() {
        use crate::asr::engine::{AsrEngine, AsrResult};

        /// Mock ASR that bumps the job ID as a side effect.
        struct StaleJobAsrEngine {
            state_ref: Arc<AppState>,
        }
        impl AsrEngine for StaleJobAsrEngine {
            fn init(&mut self) -> Result<(), String> { Ok(()) }
            fn is_ready(&self) -> bool { true }
            fn transcribe_file(&mut self, _path: &str) -> Result<AsrResult, String> {
                self.state_ref.new_job();
                Ok(AsrResult {
                    text: "stale result".to_string(),
                    duration_secs: 1.0,
                    processing_time_secs: 0.01,
                    rtfx: 100.0,
                })
            }
            fn transcribe_samples(&mut self, _: &[f32], _: u32) -> Result<AsrResult, String> {
                self.transcribe_file("")
            }
            fn is_model_available(&self) -> bool { true }
            fn backend_name(&self) -> &'static str { "stale-job-mock" }
        }

        let mock_paste = Arc::new(MockPasteBackend::new());

        let settings = Settings {
            auto_paste: true,
            llm_cleanup_enabled: false,
            ..Default::default()
        };

        let state = Arc::new(AppState::new_with_backends(
            Box::new(MockAudioCapture::sine_wave()),
            Box::new(MockAsrEngine::with_text("placeholder")),
            None,
            Box::new(SharedMockPaste(mock_paste.clone())),
            settings,
        ));

        // Replace ASR engine with the side-effect mock
        {
            let mut engine = state.asr_engine.lock().await;
            *engine = Box::new(StaleJobAsrEngine { state_ref: state.clone() });
        }

        let events = CollectingEvents::new();

        pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
        pipeline_stop_recording(&state, &events).await.unwrap();

        // Assert no paste occurred (stale job discarded after ASR returned)
        assert!(mock_paste.last_pasted().is_none());
        assert_eq!(state.get_state(), AppStateEnum::Idle);
    }

    // --- Test 7: LLM cleanup failure falls back to raw text ---
    #[tokio::test]
    #[serial_test::serial]
    async fn test_llm_failure_uses_raw_text() {
        let mock_paste = Arc::new(MockPasteBackend::new());
        let state = test_state(
            MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("hello world this is a test sentence"),
            Some(MockLlmBackend::failing("sidecar crashed")),
            Box::new(SharedMockPaste(mock_paste.clone())),
        );
        let events = CollectingEvents::new();

        pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
        pipeline_stop_recording(&state, &events).await.unwrap();

        // Assert raw text was pasted (LLM failed, graceful fallback)
        let pasted = mock_paste.last_pasted().expect("should have pasted");
        assert_eq!(pasted.text, "hello world this is a test sentence");

        // Assert transcription records the failure with structured status
        let last = state.last_transcription.lock().await;
        let t = last.as_ref().unwrap();
        assert!(!t.llm_applied);
        match &t.llm_cleanup_status {
            LlmCleanupStatus::Failed { reason } => {
                assert!(reason.contains("sidecar crashed"));
            }
            other => panic!("expected Failed status, got {:?}", other),
        }
    }

    // --- Test 11: Short input emits SkippedTooShort status ---
    #[tokio::test]
    #[serial_test::serial]
    async fn test_short_input_emits_skipped_too_short() {
        let mock_paste = Arc::new(MockPasteBackend::new());
        let state = test_state(
            MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("hi there"),  // 2 words — under MIN_CLEANUP_WORDS
            Some(MockLlmBackend::fixed("SHOULD NOT BE USED")),
            Box::new(SharedMockPaste(mock_paste.clone())),
        );
        let events = CollectingEvents::new();

        pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
        pipeline_stop_recording(&state, &events).await.unwrap();

        // Raw text should be pasted; LLM was bypassed
        let pasted = mock_paste.last_pasted().expect("should have pasted");
        assert_eq!(pasted.text, "hi there");

        let last = state.last_transcription.lock().await;
        let t = last.as_ref().unwrap();
        assert!(!t.llm_applied);
        assert_eq!(t.llm_cleanup_status, LlmCleanupStatus::SkippedTooShort);
    }

    // --- Test 8: Short recording discarded ---
    #[tokio::test]
    async fn test_short_recording_discarded() {
        let mock_paste = Arc::new(MockPasteBackend::new());
        // Only 100 samples -- below the 4000-sample minimum
        let state = test_state(
            MockAudioCapture::new(vec![0.0f32; 100], 48_000),
            MockAsrEngine::with_text("should not reach ASR"),
            None,
            Box::new(SharedMockPaste(mock_paste.clone())),
        );
        let events = CollectingEvents::new();

        pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
        pipeline_stop_recording(&state, &events).await.unwrap();

        // Assert no paste, no transcription (short recording silently discarded)
        assert!(mock_paste.last_pasted().is_none());
        assert_eq!(state.get_state(), AppStateEnum::Idle);
    }

    // --- Test 9: Paste failure copies to clipboard as fallback ---
    #[tokio::test]
    #[serial_test::serial]
    async fn test_paste_failure_clipboard_fallback() {
        let mock_paste = Arc::new(MockPasteBackend::new());
        *mock_paste.paste_error.lock().unwrap() = Some("Accessibility denied".into());

        let state = test_state(
            MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("hello world"),
            None,
            Box::new(SharedMockPaste(mock_paste.clone())),
        );
        let events = CollectingEvents::new();

        pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
        pipeline_stop_recording(&state, &events).await.unwrap();

        // Assert paste was attempted but failed
        let paste_errors = events.paste_errors.lock().unwrap();
        assert!(!paste_errors.is_empty());

        // Assert text was copied to clipboard as fallback
        let copied = mock_paste.copied_texts.lock().unwrap();
        assert_eq!(copied.len(), 1);
        assert_eq!(copied[0], "hello world");
    }

    // --- Test 10: Auto-paste disabled -- copy to clipboard only ---
    #[tokio::test]
    #[serial_test::serial]
    async fn test_auto_paste_disabled() {
        let mock_paste = Arc::new(MockPasteBackend::new());

        let settings = Settings {
            auto_paste: false,
            llm_cleanup_enabled: false,
            ..Default::default()
        };

        let state = AppState::new_with_backends(
            Box::new(MockAudioCapture::sine_wave()),
            Box::new(MockAsrEngine::with_text("hello world")),
            None,
            Box::new(SharedMockPaste(mock_paste.clone())),
            settings,
        );
        let events = CollectingEvents::new();

        pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
        pipeline_stop_recording(&state, &events).await.unwrap();

        // Assert no paste (paste_text was never called)
        assert!(mock_paste.pasted_texts.lock().unwrap().is_empty());

        // Assert text was copied to clipboard instead
        let copied = mock_paste.copied_texts.lock().unwrap();
        assert_eq!(copied[0], "hello world");
    }
}
