use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;

#[derive(Default)]
pub struct CaptureHealth {
    pub generation: u64,
    pub error: Option<String>,
}

/// Metadata returned after microphone shutdown and checked WAV serialization.
/// No large sample buffer crosses back onto an async executor thread.
#[derive(Debug)]
pub(crate) struct FinishedCapture {
    pub sample_count: usize,
    pub sample_rate: u32,
    pub duration_ms: u64,
    pub capture_error: Option<String>,
    pub audio_path: Option<std::path::PathBuf>,
}

pub(crate) fn active_recording_path(state: &crate::state::AppState) -> Option<std::path::PathBuf> {
    state.recording_audio_path.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Stop finishes in-flight callbacks before the writer drains its bounded queue.
pub(crate) async fn finish_recording_capture(
    state: &crate::state::AppState,
) -> Result<FinishedCapture, String> {
    let capture = Arc::clone(&state.audio_capture);
    let health = Arc::clone(&state.capture_health);
    let writer = state.recording_writer.lock().unwrap_or_else(|e| e.into_inner()).take()
        .ok_or("No recording writer is active")?;
    let path = writer.path.clone();
    let result = tokio::task::spawn_blocking(move || {
        let sample_rate = {
            let mut capture = capture.lock().unwrap_or_else(|e| e.into_inner());
            capture.stop();
            capture.sample_rate()
        };
        let sample_count = writer.finish()?;
        let capture_error = health.lock().unwrap_or_else(|e| e.into_inner()).error.clone();
        let duration_ms = super::wav::captured_duration_ms(sample_count, sample_rate);
        let audio_path = if sample_count < 4000 && capture_error.is_none() {
            let _ = std::fs::remove_file(&path);
            None
        } else { Some(path) };
        Ok(FinishedCapture { sample_count, sample_rate, duration_ms, capture_error, audio_path })
    }).await.map_err(|e| format!("Audio finalization worker failed: {e}"))
        .and_then(|result| result);
    state.is_recording.store(false, Ordering::SeqCst);
    result
}

/// Trait for audio capture backends.
/// Production: wraps cpal. Tests: sends pre-recorded samples.
pub trait AudioCaptureBackend: Send {
    /// Start capturing audio.
    ///
    /// - `sender`: channel to send PCM chunks (mono f32) to the consumer.
    /// - `is_recording`: shared flag; the backend should stop sending when false.
    /// - `level_callback`: called with RMS level (~30 Hz) for waveform UI.
    fn start(
        &mut self,
        sender: SyncSender<Vec<f32>>,
        is_recording: Arc<AtomicBool>,
        level_callback: Box<dyn Fn(f32) + Send + 'static>,
        error_callback: Box<dyn Fn(String) + Send + 'static>,
    ) -> Result<(), String>;

    /// Stop capturing. Must be idempotent (calling stop when not started is a no-op).
    fn stop(&mut self);

    /// The sample rate of the captured audio. Valid after start() succeeds.
    /// Returns the rate used by the cpal stream (production) or a fixed value (tests).
    fn sample_rate(&self) -> u32;
}

/// Shared microphone start for hotkeys, IPC, and the pipeline regression tests.
/// Keep the state claim until the stream is acquired so two starts cannot race.
pub(crate) fn start_recording_capture(
    state: &crate::state::AppState,
    level_callback: Box<dyn Fn(f32) + Send + 'static>,
) -> Result<u64, String> {
    start_recording_capture_with_errors(state, level_callback, Box::new(|_, _| {}))
}

pub(crate) fn start_recording_capture_with_errors(
    state: &crate::state::AppState,
    level_callback: Box<dyn Fn(f32) + Send + 'static>,
    error_callback: Box<dyn Fn(u64, String) + Send + Sync + 'static>,
) -> Result<u64, String> {
    use crate::models::AppStateEnum;
    let mut current = state
        .current_state
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if state.is_exiting.load(Ordering::SeqCst) {
        return Err("SottoASR is quitting".into());
    }
    if state.shortcuts_updating.load(Ordering::SeqCst) {
        return Err("Keyboard shortcuts are being saved. Try recording again in a moment.".into());
    }
    if *current != AppStateEnum::Idle {
        return Err(format!(
            "Cannot start recording: currently in {current:?} state"
        ));
    }
    if let Some(error) = state.recording_storage_error.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return Err(format!("Recording storage is unavailable: {error}"));
    }
    *state
        .recording_vocabulary
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = state
        .vocabulary_terms
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    let generation = state.recording_generation.fetch_add(1, Ordering::SeqCst) + 1;
    *state
        .capture_health
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = CaptureHealth {
        generation,
        error: None,
    };
    let health = state.capture_health.clone();
    let on_error: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |error: String| {
        let mut health = health.lock().unwrap_or_else(|error| error.into_inner());
        if health.generation != generation || health.error.is_some() {
            return;
        }
        health.error = Some(error.clone());
        drop(health);
        error_callback(generation, error);
    });
    let path = super::recovery::recordings_dir()?.join(format!("sotto_{}.wav", uuid::Uuid::new_v4()));
    let (writer, sender, rate_sender) = super::recovery::RecordingWriter::start(path.clone(), on_error.clone())?;
    *state.recording_audio_path.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
    let recording = Arc::new(AtomicBool::new(true));
    state.is_recording.store(true, Ordering::SeqCst);
    let mut capture = state.audio_capture.lock().unwrap_or_else(|error| error.into_inner());
    let result = capture.start(sender, recording, level_callback, Box::new(move |error| on_error(error)));
    match result {
        Ok(()) => {
            if rate_sender.send(capture.sample_rate()).is_err() {
                capture.stop();
                state.is_recording.store(false, Ordering::SeqCst);
                drop(rate_sender);
                writer.abandon_start();
                *state.recording_audio_path.lock().unwrap_or_else(|e| e.into_inner()) = None;
                return Err("Recording storage worker stopped during microphone startup".into());
            }
            *state.recording_writer.lock().unwrap_or_else(|e| e.into_inner()) = Some(writer);
            state
                .target_pid
                .store(state.paste_backend.get_frontmost_pid(), Ordering::SeqCst);
            *current = AppStateEnum::Recording;
            Ok(generation)
        }
        Err(error) => {
            capture.stop();
            drop(rate_sender);
            writer.abandon_start();
            *state.recording_audio_path.lock().unwrap_or_else(|e| e.into_inner()) = None;
            state.is_recording.store(false, Ordering::SeqCst);
            Err(error)
        }
    }
}

pub struct AudioCapture {
    stream: Option<cpal::Stream>,
    /// Sample rate discovered during start(). Defaults to 48000.
    captured_sample_rate: u32,
}

// SAFETY: AudioCapture is only accessed through Mutex<AudioCapture> in AppState.
// The cpal::Stream is created and dropped within AudioCapture methods, and the
// Mutex ensures exclusive access across threads.
unsafe impl Send for AudioCapture {}

impl AudioCapture {
    pub fn new() -> Self {
        Self {
            stream: None,
            captured_sample_rate: 48000,
        }
    }
}

impl AudioCaptureBackend for AudioCapture {
    fn start(
        &mut self,
        sender: SyncSender<Vec<f32>>,
        is_recording: Arc<AtomicBool>,
        level_callback: Box<dyn Fn(f32) + Send + 'static>,
        error_callback: Box<dyn Fn(String) + Send + 'static>,
    ) -> Result<(), String> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or("No default input device available")?;

        let config = device
            .default_input_config()
            .map_err(|e| format!("Failed to get input config: {}", e))?;

        log::info!(
            "Audio capture: {} channels, {} Hz, {:?}",
            config.channels(),
            config.sample_rate().0,
            config.sample_format()
        );

        let channels = config.channels() as usize;
        let sample_rate = config.sample_rate().0;
        self.captured_sample_rate = sample_rate;
        let sender_clone = sender.clone();
        let is_recording_clone = is_recording.clone();
        let shared_error = Arc::new(std::sync::Mutex::new(error_callback));
        let callback_error = shared_error.clone();
        let send_error = move |error| {
            callback_error.lock().unwrap_or_else(|e| e.into_inner())(error);
        };

        // Level metering: accumulate ~33ms of samples, then emit RMS
        let level_window = sample_rate as usize / 30; // ~1600 samples at 48kHz
        let mut level_buffer = Vec::with_capacity(level_window);
        let mut level_emit_count: u64 = 0;

        // Pre-allocate mono buffer outside the callback to avoid heap
        // allocations on the real-time audio thread (only used when channels > 1).
        let mut mono_buffer: Vec<f32> = Vec::with_capacity(4096);

        let stream = device
            .build_input_stream(
                &config.into(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if !is_recording_clone.load(Ordering::Relaxed) {
                        return;
                    }

                    // Downmix to mono if needed, reusing pre-allocated buffer
                    let mono: &[f32] = if channels > 1 {
                        let mono_len = data.len() / channels;
                        mono_buffer.clear();
                        if mono_buffer.capacity() < mono_len {
                            mono_buffer.reserve(mono_len - mono_buffer.capacity());
                        }
                        for frame in data.chunks(channels) {
                            mono_buffer.push(frame.iter().sum::<f32>() / channels as f32);
                        }
                        &mono_buffer
                    } else {
                        data
                    };
                    // Bound each queued allocation as well as the queue length.
                    for chunk in mono.chunks(4096) {
                        if let Err(error) = sender_clone.try_send(chunk.to_vec()) {
                            is_recording_clone.store(false, Ordering::Relaxed);
                            send_error(format!("Recording storage could not keep up: {error}"));
                            break;
                        }
                    }

                    // Calculate audio level for waveform visualization
                    level_buffer.extend_from_slice(mono);
                    if level_buffer.len() >= level_window {
                        let rms = calculate_rms(&level_buffer);
                        level_callback(rms);
                        level_buffer.clear();

                        level_emit_count += 1;
                        if level_emit_count % 30 == 1 {
                            // Log every ~1 second to verify levels are flowing
                            log::debug!("Audio level: {:.4} (emit #{})", rms, level_emit_count);
                        }
                    }
                },
                move |err| {
                    log::error!("Audio capture error: {}", err);
                    shared_error.lock().unwrap_or_else(|e| e.into_inner())(format!("Microphone capture was interrupted: {err}"));
                },
                None,
            )
            .map_err(|e| format!("Failed to build input stream: {}", e))?;

        stream
            .play()
            .map_err(|e| format!("Failed to start audio stream: {}", e))?;

        self.stream = Some(stream);
        log::info!("Audio capture started");
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(stream) = self.stream.take() {
            drop(stream);
            log::info!("Audio capture stopped");
        }
    }

    fn sample_rate(&self) -> u32 {
        self.captured_sample_rate
    }
}

pub(crate) fn calculate_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|s| s * s).sum();
    (sum / samples.len() as f32).sqrt().min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_microphone_start_clears_reservation_and_active_path() {
        struct Unavailable;
        impl AudioCaptureBackend for Unavailable {
            fn start(&mut self, _: SyncSender<Vec<f32>>, _: Arc<AtomicBool>, _: Box<dyn Fn(f32) + Send>, _: Box<dyn Fn(String) + Send>) -> Result<(), String> {
                Err("microphone unavailable".into())
            }
            fn stop(&mut self) {}
            fn sample_rate(&self) -> u32 { 16000 }
        }
        let state = crate::state::AppState::new_with_backends(Box::new(Unavailable),
            Box::new(crate::test_support::MockAsrEngine::with_text("unused")), None,
            Box::new(crate::test_support::MockPasteBackend::new()), crate::models::Settings::default());
        assert!(start_recording_capture(&state, Box::new(|_| {})).unwrap_err().contains("microphone unavailable"));
        assert_eq!(state.get_state(), crate::models::AppStateEnum::Idle);
        assert!(!state.is_recording.load(Ordering::SeqCst));
        assert!(active_recording_path(&state).is_none());
        assert!(state.recording_writer.lock().unwrap().is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn finalization_yields_and_preserves_callbacks_from_stream_shutdown() {
        struct DelayedStop {
            started: Option<tokio::sync::oneshot::Sender<()>>,
            release: std::sync::mpsc::Receiver<()>,
            audio: Option<SyncSender<Vec<f32>>>,
            error: Option<Box<dyn Fn(String) + Send>>,
        }
        impl AudioCaptureBackend for DelayedStop {
            fn start(
                &mut self,
                sender: SyncSender<Vec<f32>>,
                _: Arc<AtomicBool>,
                _: Box<dyn Fn(f32) + Send>,
                error: Box<dyn Fn(String) + Send>,
            ) -> Result<(), String> {
                sender.send(vec![0.25; 192_000]).unwrap();
                self.audio = Some(sender);
                self.error = Some(error);
                Ok(())
            }
            fn stop(&mut self) {
                self.started.take().unwrap().send(()).unwrap();
                self.release
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .unwrap();
                self.audio.take().unwrap().send(vec![0.75, -0.5]).unwrap();
                self.error.take().unwrap()("device disconnected during shutdown".into());
            }
            fn sample_rate(&self) -> u32 {
                192_000
            }
        }
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let state = Arc::new(crate::state::AppState::new_with_backends(
            Box::new(DelayedStop {
                started: Some(started_tx),
                release: release_rx,
                audio: None,
                error: None,
            }),
            Box::new(crate::test_support::MockAsrEngine::with_text("unused")),
            None,
            Box::new(crate::test_support::MockPasteBackend::new()),
            crate::models::Settings::default(),
        ));
        let generation = start_recording_capture(&state, Box::new(|_| {})).unwrap();
        assert!(state.claim_recording_end(Some(generation)));
        let path = active_recording_path(&state).unwrap();
        let worker_state = Arc::clone(&state);
        let worker = tokio::spawn(async move { finish_recording_capture(&worker_state).await });
        started_rx.await.unwrap();
        // The sole Tokio thread can release Stop only when shutdown was offloaded.
        release_tx.send(()).unwrap();
        let finished = worker.await.unwrap().unwrap();
        assert!(!state.is_recording.load(Ordering::SeqCst));
        assert_eq!(finished.sample_count, 192_002);
        assert_eq!(finished.sample_rate, 192_000);
        assert_eq!(finished.duration_ms, 1000);
        assert_eq!(
            finished.capture_error.as_deref(),
            Some("device disconnected during shutdown")
        );
        assert_eq!(finished.audio_path, Some(path.clone()));
        let mut reader = hound::WavReader::open(path).unwrap();
        let samples = reader
            .samples::<f32>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(&samples[192_000..192_002], &[0.75, -0.5]);
        assert_eq!(samples.len(), 192_002 + 144_000);
    }


    #[test]
    fn capture_errors_notify_once_and_old_callbacks_cannot_poison_a_new_recording() {
        use std::sync::Mutex;
        type Errors = Arc<Mutex<Vec<Box<dyn Fn(String) + Send>>>>;
        struct ErrorCapture(Errors);
        impl AudioCaptureBackend for ErrorCapture {
            fn start(
                &mut self,
                _: SyncSender<Vec<f32>>,
                _: Arc<AtomicBool>,
                _: Box<dyn Fn(f32) + Send>,
                error: Box<dyn Fn(String) + Send>,
            ) -> Result<(), String> {
                self.0.lock().unwrap().push(error);
                Ok(())
            }
            fn stop(&mut self) {}
            fn sample_rate(&self) -> u32 {
                16000
            }
        }
        let errors: Errors = Default::default();
        let notifications = Arc::new(Mutex::new(Vec::new()));
        let state = crate::state::AppState::new_with_backends(
            Box::new(ErrorCapture(errors.clone())),
            Box::new(crate::test_support::MockAsrEngine::with_text("unused")),
            None,
            Box::new(crate::test_support::MockPasteBackend::new()),
            crate::models::Settings::default(),
        );
        let first = start_recording_capture(&state, Box::new(|_| {})).unwrap();
        assert!(state.claim_recording_end(Some(first)));
        state.set_state(crate::models::AppStateEnum::Idle);
        let notified = notifications.clone();
        let second = start_recording_capture_with_errors(
            &state,
            Box::new(|_| {}),
            Box::new(move |generation, error| {
                notified.lock().unwrap().push((generation, error));
            }),
        )
        .unwrap();
        let errors = errors.lock().unwrap();
        errors[0]("old device".into());
        assert!(state.capture_error().is_none());
        errors[1]("new device".into());
        errors[1]("duplicate".into());
        assert_eq!(state.capture_error().as_deref(), Some("new device"));
        assert_eq!(
            *notifications.lock().unwrap(),
            vec![(second, "new device".into())]
        );
    }

    #[test]
    fn rms_empty_returns_zero() {
        assert_eq!(calculate_rms(&[]), 0.0);
    }

    #[test]
    fn rms_all_zeros_returns_zero() {
        assert_eq!(calculate_rms(&[0.0, 0.0, 0.0, 0.0]), 0.0);
    }

    #[test]
    fn rms_known_value() {
        let rms = calculate_rms(&[1.0, -1.0, 1.0, -1.0]);
        assert!((rms - 1.0).abs() < 1e-6);
    }

    #[test]
    fn rms_known_value_half() {
        let rms = calculate_rms(&[0.5, -0.5]);
        assert!((rms - 0.5).abs() < 1e-6);
    }

    #[test]
    fn rms_single_sample() {
        let rms = calculate_rms(&[0.3]);
        assert!((rms - 0.3).abs() < 1e-6);
    }

    #[test]
    fn rms_clamps_to_one() {
        let rms = calculate_rms(&[5.0, 5.0, 5.0]);
        assert_eq!(rms, 1.0);
    }

}
