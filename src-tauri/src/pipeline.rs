use crate::llm::cleanup::run_cleanup;
use crate::models::{AppStateEnum, LlmCleanupStatus, Transcription};
use crate::state::AppState;


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
    match crate::audio::capture::start_recording_capture(state, level_callback) {
        Ok(_) => {
            events.emit_recording_started();
            events.emit_state_changed(&AppStateEnum::Recording);
            Ok(())
        }
        Err(error) => {
            events.emit_recording_error(&error);
            Err(error)
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
    // Claim before stopping or draining: exactly one stop/cancel owns the audio.
    if !state.try_transition(AppStateEnum::Recording, AppStateEnum::Transcribing) {
        return Err(format!("Cannot stop recording: currently in {:?} state", state.get_state()));
    }

    events.emit_recording_stopped();
    events.emit_state_changed(&AppStateEnum::Transcribing);
    let finished = match crate::audio::capture::finish_recording_capture(state).await {
        Ok(finished) => finished,
        Err(error) => {
            state.set_state(AppStateEnum::Idle);
            events.emit_state_changed(&AppStateEnum::Idle);
            events.emit_transcription_error(&error);
            return Err(error);
        }
    };
    let capture_error = finished.capture_error;
    let duration_ms = finished.duration_ms;
    let Some(temp_path) = finished.audio_path else {
        state.set_state(AppStateEnum::Idle);
        events.emit_state_changed(&AppStateEnum::Idle);
        return Ok(());
    };

    let temp_path_str = temp_path.to_string_lossy().to_string();
    if let Some(error) = &capture_error {
        events.emit_recording_error(&format!("{error}. Only the captured portion was saved; nothing will be pasted. Audio retained at {}", temp_path.display()));
    }

    // 8. Assign job ID
    let job_id = state.new_job();

    // 9. Transcribe via ASR engine
    let vocabulary = state.recording_vocabulary.lock().unwrap_or_else(|error| error.into_inner()).clone();
    log::info!("Starting transcription...");
    let result = crate::asr::engine::with_engine(&state.asr_engine, move |engine| {
        engine.transcribe_file_with_vocabulary(&temp_path_str, &vocabulary)
    }).await;

    let result = result.map_err(|error| format!("{error}. Audio retained at {}", temp_path.display()));

    // 10. Handle ASR result
    match result {
        Ok(asr_result) => {
            log::info!("Transcription complete (RTF: {:.1}x)", asr_result.rtfx);

            // 10a. Check job staleness
            if !state.is_current_job(job_id) {
                log::info!("Job {} is stale, discarding transcription", job_id);
                state.set_state(AppStateEnum::Idle);
                events.emit_state_changed(&AppStateEnum::Idle);
                return Ok(());
            }

            let raw_asr_text = asr_result.unboosted_text.clone().unwrap_or_else(|| asr_result.text.clone());
            let mut final_text = asr_result.text.clone();
            let cleanup_suggestion = None;
            let cleanup_status: LlmCleanupStatus;

            // 10b. Read settings
            let settings = state.settings.lock().await;
            let llm_enabled = settings.llm_cleanup_enabled;
            let auto_paste = settings.auto_paste;
            let restore_clipboard = settings.restore_clipboard;
            let restore_focus_before_paste = settings.restore_focus_before_paste;
            let dictionary = settings.dictionary.clone();
            let protected_terms: Vec<String> = settings.vocabulary.iter().cloned()
                .chain(dictionary.iter().map(|entry| entry.replacement.clone())).collect();
            drop(settings);

            if capture_error.is_none() { final_text = crate::dictionary::apply(&final_text, &dictionary); }

            // 10c. LLM cleanup if enabled. The shared `run_cleanup()` helper
            // owns the spawn-or-reuse, timeout, kill-orphan, and zombie-handle
            // detection logic. See docs/specs/2026-04-11-llm-cleanup-reliability.md.
            if llm_enabled && capture_error.is_none() {
                state.set_state(AppStateEnum::CleaningUp);
                events.emit_state_changed(&AppStateEnum::CleaningUp);

                let (cleaned, status) = run_cleanup(state, &final_text, &protected_terms).await;
                cleanup_status = status;
                if matches!(cleanup_status, LlmCleanupStatus::Applied { .. }) {
                    final_text = cleaned;
                }
            } else {
                cleanup_status = LlmCleanupStatus::Disabled;
            }


            // 10d. Second staleness check
            if !state.is_current_job(job_id) {
                log::info!("Job {} is stale after cleanup, discarding", job_id);
                return Ok(());
            }

            // Cache the latest cleanup status on AppState so the frontend
            // can read it via get_llm_status. The hotkey path also emits a
            // Tauri event in addition to caching — pipeline.rs is test-only
            // so the cache is the only surface here.
            {
                let mut last = state.llm_last_status.lock().await;
                *last = cleanup_status.clone();
            }

            // 10e. Build Transcription struct
            let transcription = Transcription {
                id: uuid::Uuid::new_v4().to_string(),
                text: final_text.clone(),
                duration_ms,
                created_at: chrono::Utc::now(),
                word_count: final_text.split_whitespace().count(),
                cancelled: false,
                capture_error: capture_error.clone(),
                raw_text: (final_text != raw_asr_text).then_some(raw_asr_text),
                cleanup_suggestion,
                llm_applied: matches!(cleanup_status, LlmCleanupStatus::Applied { .. }),
                llm_cleanup_status: cleanup_status.clone(),
            };

            // 10f. Save to state.last_transcription + add_transcription
            {
                let mut last = state.last_transcription.lock().await;
                *last = Some(transcription.clone());
            }
            let saved = crate::commands::transcription::add_transcription(transcription.clone()).await;
            crate::audio::wav::finish_after_history(&temp_path, capture_error.is_some(), saved.is_ok());
            if let Err(error) = saved {
                log::error!("History save failed: {error}");
                events.emit_recording_error(&format!("{error}. Audio retained at {}", temp_path.display()));
            }

            events.emit_transcription_complete(&transcription);

            // 10g. Paste or copy to clipboard
            if capture_error.is_none() && !final_text.trim().is_empty() {
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

                    let paste_result = crate::paste::backend::write_text(
                            &state.paste_backend, &final_text,
                            crate::paste::backend::ClipboardAction::Paste { target_pid, restore: restore_clipboard },
                        ).await;

                    match paste_result {
                        Ok(()) => {
                            log::info!("Text pasted at cursor");
                            events.emit_paste_complete(&transcription.id);
                        }
                        Err(e) => {
                            log::error!("Paste failed: {}", e);
                            let error = match crate::paste::backend::write_text(&state.paste_backend, &final_text, crate::paste::backend::ClipboardAction::Copy).await {
                                Ok(()) => { log::info!("Text copied to clipboard as fallback"); e }
                                Err(copy_error) => format!("{e}. Clipboard copy also failed: {copy_error}"),
                            };
                            events.emit_paste_error(&error, &final_text);
                        }
                    }
                } else {
                    match crate::paste::backend::write_text(&state.paste_backend, &final_text, crate::paste::backend::ClipboardAction::Copy).await {
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
    if !state.try_transition(AppStateEnum::Recording, AppStateEnum::Transcribing) {
        log::warn!("Cannot cancel recording: currently in {:?} state", state.get_state());
        return None;
    }

    events.emit_recording_cancelled();
    events.emit_state_changed(&AppStateEnum::Transcribing);
    let finished = match crate::audio::capture::finish_recording_capture(state).await {
        Ok(finished) => finished,
        Err(error) => {
            state.set_state(AppStateEnum::Idle);
            events.emit_state_changed(&AppStateEnum::Idle);
            events.emit_transcription_error(&error);
            return None;
        }
    };
    let capture_error = finished.capture_error;
    let duration_ms = finished.duration_ms;
    if let Some(temp_path) = finished.audio_path {
        let temp_path_str = temp_path.to_string_lossy().to_string();
        if let Some(error) = &capture_error {
            events.emit_recording_error(&format!("{error}. Audio retained at {}", temp_path.display()));
        }

        let vocabulary = state.recording_vocabulary.lock().unwrap_or_else(|error| error.into_inner()).clone();
        let result = crate::asr::engine::with_engine(&state.asr_engine, move |engine| {
            engine.transcribe_file_with_vocabulary(&temp_path_str, &vocabulary)
        }).await;
        let result = result.map_err(|error| format!("{error}. Audio retained at {}", temp_path.display()));

        if let Err(error) = &result { events.emit_transcription_error(error); }
        if let Ok(asr_result) = result {
            let transcription = Transcription {
                id: uuid::Uuid::new_v4().to_string(),
                text: asr_result.text.clone(),
                duration_ms,
                created_at: chrono::Utc::now(),
                word_count: asr_result.text.split_whitespace().count(),
                cancelled: true,
                capture_error: capture_error.clone(),
                raw_text: asr_result.unboosted_text.clone(),
                cleanup_suggestion: None,
                llm_applied: false,
                llm_cleanup_status: LlmCleanupStatus::Idle,
            };
            let saved = crate::commands::transcription::add_transcription(transcription.clone()).await;
            crate::audio::wav::finish_after_history(&temp_path, capture_error.is_some(), saved.is_ok());
            if let Err(error) = saved {
                log::error!("History save failed: {error}");
                events.emit_recording_error(&format!("{error}. Audio retained at {}", temp_path.display()));
            }
            events.emit_transcription_complete(&transcription);
            log::info!("Cancelled transcription saved");

            state.set_state(AppStateEnum::Idle);
            events.emit_state_changed(&AppStateEnum::Idle);
            return Some(transcription);
        }

        state.set_state(AppStateEnum::Idle);
        events.emit_state_changed(&AppStateEnum::Idle);
        None
    } else {
        // Too short to transcribe -- just save a placeholder
        let transcription = Transcription {
            id: uuid::Uuid::new_v4().to_string(),
            text: String::new(),
            duration_ms,
            created_at: chrono::Utc::now(),
            word_count: 0,
            cancelled: true,
            capture_error: None,
            raw_text: None,
            cleanup_suggestion: None,
            llm_applied: false,
            llm_cleanup_status: LlmCleanupStatus::Idle,
        };
        if let Err(error) = crate::commands::transcription::add_transcription(transcription.clone()).await {
            log::error!("History save failed: {error}");
            events.emit_recording_error(&error);
        }
        events.emit_transcription_complete(&transcription);
        state.set_state(AppStateEnum::Idle);
        events.emit_state_changed(&AppStateEnum::Idle);
        Some(transcription)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use crate::models::Settings;
    use std::sync::Arc;

    #[test]
    fn busy_exit_preserves_capture_and_accepted_exit_blocks_new_recordings() {
        let state = AppState::new_with_backends(
            Box::new(MockAudioCapture::new(vec![0.1; 4800], 48_000)),
            Box::new(MockAsrEngine::with_text("test")), None,
            Box::new(MockPasteBackend::new()), Settings::default(),
        );
        let events = CollectingEvents::new();
        pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
        assert!(state.begin_exit().is_err());
        assert!(!state.is_exiting.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(state.get_state(), AppStateEnum::Recording);
        state.audio_capture.lock().unwrap().stop();
        state.set_state(AppStateEnum::Idle);
        state.begin_exit().unwrap();
        assert!(pipeline_start_recording(&state, &events, Box::new(|_| {})).is_err());
        assert_eq!(state.get_state(), AppStateEnum::Idle);
    }

    #[test]
    fn shortcut_transaction_excludes_capture_until_save_or_rollback_finishes() {
        let state = AppState::new_with_backends(
            Box::new(MockAudioCapture::new(vec![0.1; 4800], 48_000)),
            Box::new(MockAsrEngine::with_text("test")), None,
            Box::new(MockPasteBackend::new()), Settings::default(),
        );
        let events = CollectingEvents::new();
        let transaction = state.begin_shortcut_update().unwrap();
        assert!(pipeline_start_recording(&state, &events, Box::new(|_| {})).is_err());
        assert_eq!(state.get_state(), AppStateEnum::Idle);
        assert_eq!(state.recording_generation.load(std::sync::atomic::Ordering::SeqCst), 0);
        drop(transaction); // Also runs on a save/rollback error or cancelled task.
        pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
        assert!(state.begin_shortcut_update().is_err());
        state.audio_capture.lock().unwrap().stop();
    }

    #[tokio::test]
    async fn device_failure_during_stop_retains_tail_and_never_pastes_partial_text() {
        use crate::audio::capture::AudioCaptureBackend;
        use std::sync::{atomic::AtomicBool, mpsc::Sender};
        struct InterruptedCapture {
            sender: Option<Sender<Vec<f32>>>,
            error: Option<Box<dyn Fn(String) + Send>>,
        }
        impl AudioCaptureBackend for InterruptedCapture {
            fn start(&mut self, sender: Sender<Vec<f32>>, _: Arc<AtomicBool>, _: Box<dyn Fn(f32) + Send>, error: Box<dyn Fn(String) + Send>) -> Result<(), String> {
                sender.send(vec![0.25; 192_000]).unwrap();
                self.sender = Some(sender);
                self.error = Some(error);
                Ok(())
            }
            fn stop(&mut self) {
                if let Some(sender) = self.sender.take() { sender.send(vec![0.75, -0.5]).unwrap(); }
                if let Some(error) = self.error.take() { error("Microphone disconnected".into()); }
            }
            fn sample_rate(&self) -> u32 { 192_000 }
        }
        for cancel in [false, true] {
            let paste = Arc::new(MockPasteBackend::new());
            let state = AppState::new_with_backends(
                Box::new(InterruptedCapture { sender: None, error: None }),
                Box::new(MockAsrEngine::with_text("um, this is only the captured portion")),
                Some(Box::new(MockLlmBackend::failing("must not clean interrupted capture"))),
                Box::new(SharedMockPaste(paste.clone())),
                Settings { llm_cleanup_enabled: true, ..Default::default() },
            );
            let events = CollectingEvents::new();
            pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
            if cancel { pipeline_cancel_recording(&state, &events).await; }
            else { pipeline_stop_recording(&state, &events).await.unwrap(); }
            assert_eq!(state.get_state(), AppStateEnum::Idle);
            assert!(paste.pasted_texts.lock().unwrap().is_empty());
            assert!(paste.copied_texts.lock().unwrap().is_empty());
            let transcription = events.transcriptions.lock().unwrap()[0].clone();
            assert_eq!(transcription.capture_error.as_deref(), Some("Microphone disconnected"));
            assert_eq!(transcription.text, "um, this is only the captured portion");
            assert!(!transcription.llm_applied);
            let error = events.errors.lock().unwrap().iter().find(|error| error.contains("Audio retained at ")).unwrap().clone();
            let path = error.split_once("Audio retained at ").unwrap().1;
            let mut reader = hound::WavReader::open(path).unwrap();
            assert_eq!(reader.spec().sample_rate, 192_000);
            let samples: Vec<f32> = reader.samples::<f32>().map(Result::unwrap).collect();
            assert_eq!(&samples[192_000..192_002], &[0.75, -0.5]);
            drop(reader);
            std::fs::remove_file(path).unwrap(); // This test's synthetic recovery file only.
        }
    }

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

    #[tokio::test]
    #[serial_test::serial]
    async fn stop_drains_final_callback_and_uses_captured_duration() {
        use crate::asr::engine::{AsrEngine, AsrResult};
        use crate::audio::capture::AudioCaptureBackend;
        use std::sync::{atomic::AtomicBool, mpsc::Sender, Mutex};

        struct FinalCallbackCapture { sender: Option<Sender<Vec<f32>>> }
        impl AudioCaptureBackend for FinalCallbackCapture {
            fn start(&mut self, sender: Sender<Vec<f32>>, _: Arc<AtomicBool>, _: Box<dyn Fn(f32) + Send>, _: Box<dyn Fn(String) + Send>) -> Result<(), String> {
                sender.send(vec![0.25; 16_000 * 60]).unwrap();
                self.sender = Some(sender);
                Ok(())
            }
            fn stop(&mut self) {
                if let Some(sender) = self.sender.take() {
                    sender.send(vec![0.5; 16_000 * 5]).unwrap();
                    sender.send(vec![0.71, -0.62, 0.53, -0.44]).unwrap();
                }
            }
            fn sample_rate(&self) -> u32 { 16_000 }
        }
        struct InspectingAsr { observed: Arc<Mutex<Vec<f32>>> }
        impl AsrEngine for InspectingAsr {
            fn init(&mut self) -> Result<(), String> { Ok(()) }
            fn is_ready(&self) -> bool { true }
            fn is_model_available(&self) -> bool { true }
            fn backend_name(&self) -> &'static str { "WAV inspection" }
            fn transcribe_samples(&mut self, _: &[f32], _: u32) -> Result<AsrResult, String> { unreachable!() }
            fn transcribe_file(&mut self, path: &str) -> Result<AsrResult, String> {
                let mut reader = hound::WavReader::open(path).unwrap();
                assert_eq!(reader.spec().sample_rate, 16_000);
                *self.observed.lock().unwrap() = reader.samples().collect::<Result<_, _>>().unwrap();
                Ok(AsrResult { unboosted_text: None, text: "Complete recording.".into(), duration_secs: 0.0,
                    processing_time_secs: 0.1, rtfx: 0.0 })
            }
        }
        let observed = Arc::new(Mutex::new(Vec::new()));
        let state = AppState::new_with_backends(
            Box::new(FinalCallbackCapture { sender: None }),
            Box::new(InspectingAsr { observed: observed.clone() }), None,
            Box::new(MockPasteBackend::new()), Settings::default());
        let events = CollectingEvents::new();
        pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
        pipeline_stop_recording(&state, &events).await.unwrap();
        {
        let samples = observed.lock().unwrap();
        let captured = 16_000 * 65 + 4;
        assert_eq!(samples.len(), captured + 12_000);
        assert_eq!(&samples[captured - 4..captured], &[0.71, -0.62, 0.53, -0.44]);
        assert!(samples[16_000 * 60..captured - 4].iter().all(|sample| *sample == 0.5));
        assert!(samples[captured..].iter().all(|sample| *sample == 0.0));
        }
        assert_eq!(state.last_transcription.lock().await.as_ref().unwrap().duration_ms, 65_000);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn vocabulary_snapshot_and_original_survive_stop_cancel_and_replacements() {
        let raw = "Please use Quen for these notes.";
        for cancel in [false, true] {
            let state = test_state(
                MockAudioCapture::sine_wave(),
                MockAsrEngine::with_vocabulary(raw, &["Qwen"], "Please use Qwen for these notes."),
                None,
                Box::new(MockPasteBackend::new()),
            );
            state.vocabulary_terms.lock().unwrap().push("Qwen".into());
            state.settings.lock().await.dictionary = vec![crate::models::DictionaryEntry {
                heard: "notes".into(), replacement: "reports".into(),
            }];
            let events = CollectingEvents::new();
            pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
            // A later settings edit cannot alter this recording's snapshot.
            *state.vocabulary_terms.lock().unwrap() = vec!["Claude".into()];
            let transcription = if cancel {
                pipeline_cancel_recording(&state, &events).await.unwrap()
            } else {
                pipeline_stop_recording(&state, &events).await.unwrap();
                state.last_transcription.lock().await.clone().unwrap()
            };
            assert_eq!(transcription.raw_text.as_deref(), Some(raw));
            assert_eq!(transcription.text, if cancel {
                "Please use Qwen for these notes."
            } else {
                "Please use Qwen for these reports."
            });
        }
    }

    #[test]
    fn stale_release_cannot_claim_a_new_recording() {
        use crate::audio::capture::start_recording_capture;
        let state = test_state(MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("fixture"), None, Box::new(MockPasteBackend::new()));
        let first = start_recording_capture(&state, Box::new(|_| {})).unwrap();
        assert!(state.claim_recording_end(Some(first)));
        state.audio_capture.lock().unwrap().stop();
        state.set_state(AppStateEnum::Idle);
        let second = start_recording_capture(&state, Box::new(|_| {})).unwrap();
        assert!(second > first);
        assert!(!state.claim_recording_end(Some(first)));
        assert_eq!(state.get_state(), AppStateEnum::Recording);
        assert!(state.is_recording.load(std::sync::atomic::Ordering::SeqCst));
        assert!(state.claim_recording_end(Some(second)));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn concurrent_stop_and_cancel_have_one_audio_owner() {
        for cancel_first in [false, true] {
            let state = test_state(MockAudioCapture::sine_wave(),
                MockAsrEngine::with_text("All captured speech."), None, Box::new(MockPasteBackend::new()));
            let events = CollectingEvents::new();
            pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
            if cancel_first {
                let (cancelled, stopped) = tokio::join!(
                    pipeline_cancel_recording(&state, &events), pipeline_stop_recording(&state, &events));
                assert!(cancelled.is_some());
                assert!(stopped.is_err());
            } else {
                let (stopped, cancelled) = tokio::join!(
                    pipeline_stop_recording(&state, &events), pipeline_cancel_recording(&state, &events));
                assert!(stopped.is_ok());
                assert!(cancelled.is_none());
            }
            assert_eq!(events.transcriptions.lock().unwrap().len(), 1);
            assert_eq!(state.get_state(), AppStateEnum::Idle);
        }
    }

    #[test]
    fn shared_start_acquires_the_microphone_once_and_propagates_failure() {
        use crate::audio::capture::{start_recording_capture, AudioCaptureBackend};
        use std::sync::{atomic::{AtomicBool, AtomicUsize, Ordering}, mpsc::Sender};
        struct CountingCapture { starts: Arc<AtomicUsize>, fail: bool }
        impl AudioCaptureBackend for CountingCapture {
            fn start(&mut self, _: Sender<Vec<f32>>, _: Arc<AtomicBool>, _: Box<dyn Fn(f32) + Send + 'static>, _: Box<dyn Fn(String) + Send>) -> Result<(), String> {
                self.starts.fetch_add(1, Ordering::SeqCst);
                if self.fail { Err("microphone unavailable".into()) } else { Ok(()) }
            }
            fn stop(&mut self) {}
            fn sample_rate(&self) -> u32 { 16000 }
        }
        for fail in [false, true] {
            let starts = Arc::new(AtomicUsize::new(0));
            let state = AppState::new_with_backends(
                Box::new(CountingCapture { starts: Arc::clone(&starts), fail }),
                Box::new(MockAsrEngine::with_text("fixture")), None,
                Box::new(MockPasteBackend::new()), Settings::default(),
            );
            let result = start_recording_capture(&state, Box::new(|_| {}));
            assert_eq!(result.is_err(), fail);
            assert_eq!(starts.load(Ordering::SeqCst), 1);
            assert_eq!(state.get_state(), if fail { AppStateEnum::Idle } else { AppStateEnum::Recording });
            assert_eq!(state.is_recording.load(Ordering::SeqCst), !fail);
            if !fail {
                assert!(start_recording_capture(&state, Box::new(|_| {})).is_err());
                assert_eq!(starts.load(Ordering::SeqCst), 1);
                assert!(state.is_recording.load(Ordering::SeqCst));
            }
        }
    }

    // --- Test 1: Happy path with LLM cleanup ---
    #[tokio::test]
    #[serial_test::serial]
    async fn test_full_pipeline_happy_path() {
        let mock_paste = Arc::new(MockPasteBackend::new());
        let state = test_state(
            MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("Hello, um, world. This is a test sentence."),
            Some(MockLlmBackend::proposal("Hello, world. This is a test sentence.")),
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
        assert!(t.cleanup_suggestion.is_none());
        assert_eq!(t.raw_text.as_deref(), Some("Hello, um, world. This is a test sentence."));
        assert!(t.llm_applied);
        assert!(!t.cancelled);
        assert!(matches!(t.llm_cleanup_status, LlmCleanupStatus::Applied { .. }));
        // last_status should be cached on AppState too
        assert!(matches!(*state.llm_last_status.lock().await, LlmCleanupStatus::Applied { .. }));

        // The validated cleanup is the actual delivered text.
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
    async fn dictionary_and_cleanup_preserve_original_provenance() {
        let raw = "Please use um, Quen for the final report.";
        let expected = "Please use um, Qwen for the final report.";
        for (llm, cleaned) in [
            (None, None),
            (Some(MockLlmBackend::proposal("Invented text")), None),
            (Some(MockLlmBackend::proposal("Please use Qwen for the final report.")), Some("Please use Qwen for the final report.")),
        ] {
            let mock_paste = Arc::new(MockPasteBackend::new());
            let enabled = llm.is_some();
            let state = test_state(
                MockAudioCapture::sine_wave(),
                MockAsrEngine::with_text(raw),
                llm,
                Box::new(SharedMockPaste(mock_paste.clone())),
            );
            state.settings.lock().await.dictionary = vec![crate::models::DictionaryEntry {
                heard: "Quen".into(),
                replacement: "Qwen".into(),
            }];
            let events = CollectingEvents::new();
            pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
            pipeline_stop_recording(&state, &events).await.unwrap();

            let last = state.last_transcription.lock().await;
            let transcription = last.as_ref().unwrap();
            let expected = cleaned.unwrap_or(expected);
            assert_eq!(transcription.text, expected);
            assert_eq!(transcription.raw_text.as_deref(), Some(raw));
            assert_eq!(transcription.word_count, expected.split_whitespace().count());
            assert_eq!(transcription.llm_applied, cleaned.is_some());
            assert!(transcription.cleanup_suggestion.is_none());
            assert_eq!(mock_paste.last_pasted().unwrap().text, expected);
            if !enabled {
                assert_eq!(transcription.llm_cleanup_status, LlmCleanupStatus::Disabled);
            } else if cleaned.is_none() {
                assert!(matches!(transcription.llm_cleanup_status, LlmCleanupStatus::Failed { .. }));
            }
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn protected_word_deletions_cannot_change_delivery() {
        // Explicit literal mentions, quotes, names and required tail words are protected.
        // Short foreign phrases remain an independently measured model limitation.
        for (raw, harmful) in [
            ("The token um, is required in this exact output.", "The token is required in this exact output."),
            ("The word um is required here.", "The word is required here."),
            ("Dr. Um um confirmed the appointment.", "Dr. confirmed the appointment."),
            ("Please um retain the final instruction and number 859.", "Please retain the final instruction."),
        ] {
            for auto_paste in [false, true] {
                let paste = Arc::new(MockPasteBackend::new());
                let state = test_state(MockAudioCapture::sine_wave(),
                    MockAsrEngine::with_text(raw), Some(MockLlmBackend::proposal(harmful)),
                    Box::new(SharedMockPaste(paste.clone())));
                state.settings.lock().await.auto_paste = auto_paste;
                let events = CollectingEvents::new();
                pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
                pipeline_stop_recording(&state, &events).await.unwrap();
                let last = state.last_transcription.lock().await;
                let transcription = last.as_ref().unwrap();
                assert_eq!(transcription.text, raw); // Also the source for Copy Last.
                assert!(!transcription.llm_applied);
                assert!(transcription.cleanup_suggestion.is_none());
                assert!(matches!(transcription.llm_cleanup_status, LlmCleanupStatus::Failed { .. }));
                assert_eq!(events.transcriptions.lock().unwrap()[0].text, raw);
                if auto_paste { assert_eq!(paste.last_pasted().unwrap().text, raw); }
                else { assert_eq!(paste.copied_texts.lock().unwrap().last().map(String::as_str), Some(raw)); }
            }
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn all_filler_cleanup_saves_original_without_touching_clipboard() {
        for auto_paste in [false, true] {
            let paste = Arc::new(MockPasteBackend::new());
            let state = test_state(MockAudioCapture::sine_wave(),
                MockAsrEngine::with_text("Um, uh."), Some(MockLlmBackend::proposal("")),
                Box::new(SharedMockPaste(paste.clone())));
            state.settings.lock().await.auto_paste = auto_paste;
            let events = CollectingEvents::new();
            pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
            pipeline_stop_recording(&state, &events).await.unwrap();
            let last = state.last_transcription.lock().await;
            let item = last.as_ref().unwrap();
            assert_eq!(item.text, "");
            assert_eq!(item.raw_text.as_deref(), Some("Um, uh."));
            assert!(item.llm_applied);
            assert_eq!(item.word_count, 0);
            assert!(paste.pasted_texts.lock().unwrap().is_empty());
            assert!(paste.copied_texts.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn vocabulary_terms_are_protected_during_automatic_cleanup() {
        let raw = "Please keep um unchanged here.";
        let state = test_state(MockAudioCapture::sine_wave(), MockAsrEngine::with_text(raw),
            Some(MockLlmBackend::proposal("Please keep unchanged here.")), Box::new(MockPasteBackend::new()));
        state.settings.lock().await.vocabulary = vec!["um".into()];
        let events = CollectingEvents::new();
        pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
        pipeline_stop_recording(&state, &events).await.unwrap();
        let last = state.last_transcription.lock().await;
        let item = last.as_ref().unwrap();
        assert_eq!(item.text, raw);
        assert!(!item.llm_applied);
        assert!(matches!(item.llm_cleanup_status, LlmCleanupStatus::Failed { .. }));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn cleanup_no_op_preserves_text_without_an_applied_badge() {
        let raw = "Please keep um, every word in this transcript.";
        let state = test_state(
            MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text(raw),
            Some(MockLlmBackend::passthrough()),
            Box::new(MockPasteBackend::new()),
        );
        let events = CollectingEvents::new();
        pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
        pipeline_stop_recording(&state, &events).await.unwrap();
        let last = state.last_transcription.lock().await;
        let transcription = last.as_ref().unwrap();
        assert_eq!(transcription.text, raw);
        assert!(transcription.raw_text.is_none());
        assert!(!transcription.llm_applied);
        assert_eq!(transcription.llm_cleanup_status, LlmCleanupStatus::NoChanges);
    }

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
        let error = errors.iter().find(|error| error.contains("Model failed to load")).unwrap();
        let retained = error.split("Audio retained at ").nth(1).expect("recoverable audio path");
        assert!(std::path::Path::new(retained).is_file());
        let reader = hound::WavReader::open(retained).unwrap();
        assert!(reader.len() > 4000);
        std::fs::remove_file(retained).unwrap();
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
                Ok(AsrResult { unboosted_text: None,
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

    #[tokio::test]
    #[serial_test::serial]
    async fn stale_cleanup_does_not_publish_status_or_deliver_text() {
        struct SupersededCleanup(std::sync::Weak<AppState>);
        impl crate::llm::engine::LlmBackend for SupersededCleanup {
            fn cleanup(&mut self, _: &str) -> Result<String, String> {
                let state = self.0.upgrade().unwrap();
                state.new_job();
                state.set_state(AppStateEnum::Recording);
                Ok("Please keep the entire final instruction.".into())
            }
            fn request_raw(&mut self, _: &serde_json::Value) -> Result<serde_json::Value, String> {
                unreachable!()
            }
        }
        let paste = Arc::new(MockPasteBackend::new());
        let state = Arc::new(test_state(MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("Please um, keep the entire final instruction."),
            Some(MockLlmBackend::passthrough()), Box::new(SharedMockPaste(paste.clone()))));
        *state.llm_engine.lock().await = Some(Box::new(SupersededCleanup(Arc::downgrade(&state))));
        let events = CollectingEvents::new();
        pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
        pipeline_stop_recording(&state, &events).await.unwrap();
        assert!(state.last_transcription.lock().await.is_none());
        assert_eq!(*state.llm_last_status.lock().await, LlmCleanupStatus::Idle);
        assert_eq!(state.get_state(), AppStateEnum::Recording);
        assert!(events.transcriptions.lock().unwrap().is_empty());
        assert!(paste.pasted_texts.lock().unwrap().is_empty());
        assert!(paste.copied_texts.lock().unwrap().is_empty());
    }

    // --- Test 7: LLM cleanup failure falls back to raw text ---
    #[tokio::test]
    #[serial_test::serial]
    async fn test_llm_failure_uses_raw_text() {
        let mock_paste = Arc::new(MockPasteBackend::new());
        let state = test_state(
            MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("hello um, world this is a test sentence"),
            Some(MockLlmBackend::failing("sidecar crashed")),
            Box::new(SharedMockPaste(mock_paste.clone())),
        );
        let events = CollectingEvents::new();

        pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
        pipeline_stop_recording(&state, &events).await.unwrap();

        // Assert raw text was pasted (LLM failed, graceful fallback)
        let pasted = mock_paste.last_pasted().expect("should have pasted");
        assert_eq!(pasted.text, "hello um, world this is a test sentence");

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

    // Short transcripts must reach cleanup too.
    #[tokio::test]
    #[serial_test::serial]
    async fn test_short_input_reaches_cleanup() {
        let mock_paste = Arc::new(MockPasteBackend::new());
        let state = test_state(
            MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("um hello"),
            Some(MockLlmBackend::proposal("Hello")),
            Box::new(SharedMockPaste(mock_paste.clone())),
        );
        let events = CollectingEvents::new();

        pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
        pipeline_stop_recording(&state, &events).await.unwrap();

        // The old five-word gate must not bypass useful short cleanup.
        let pasted = mock_paste.last_pasted().expect("should have pasted");
        assert_eq!(pasted.text, "Hello");

        let last = state.last_transcription.lock().await;
        let t = last.as_ref().unwrap();
        assert!(t.llm_applied);
        assert!(matches!(t.llm_cleanup_status, LlmCleanupStatus::Applied { .. }));
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
    async fn failed_paste_and_failed_copy_report_both_errors_with_history_intact() {
        let paste = Arc::new(MockPasteBackend::new());
        *paste.paste_error.lock().unwrap() = Some("Paste permission denied".into());
        *paste.copy_error.lock().unwrap() = Some("Clipboard unavailable".into());
        let state = test_state(MockAudioCapture::sine_wave(), MockAsrEngine::with_text("Recoverable words."),
            None, Box::new(SharedMockPaste(Arc::clone(&paste))));
        let events = CollectingEvents::new();
        pipeline_start_recording(&state, &events, Box::new(|_| {})).unwrap();
        pipeline_stop_recording(&state, &events).await.unwrap();
        {
            let errors = events.paste_errors.lock().unwrap();
            assert!(errors[0].0.contains("Paste permission denied"));
            assert!(errors[0].0.contains("Clipboard unavailable"));
        }
        assert!(paste.copied_texts.lock().unwrap().is_empty());
        assert!(events.paste_ids.lock().unwrap().is_empty());
        assert_eq!(state.last_transcription.lock().await.as_ref().unwrap().text, "Recoverable words.");
    }

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
