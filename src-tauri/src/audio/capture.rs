use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
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

/// The caller must first claim Recording -> Transcribing. Keep that claim until
/// this worker completes; it owns stream shutdown, final callbacks, and samples.
pub(crate) async fn finish_recording_capture(
    state: &crate::state::AppState,
) -> Result<FinishedCapture, String> {
    let path = std::env::temp_dir().join(format!("sotto_{}.wav", uuid::Uuid::new_v4()));
    finish_recording_capture_to(state, path).await
}

async fn finish_recording_capture_to(
    state: &crate::state::AppState,
    path: std::path::PathBuf,
) -> Result<FinishedCapture, String> {
    let capture = Arc::clone(&state.audio_capture);
    let receiver = Arc::clone(&state.audio_receiver);
    let health = Arc::clone(&state.capture_health);
    let result = tokio::task::spawn_blocking(move || {
        let sample_rate = {
            let mut capture = capture.lock().unwrap_or_else(|error| error.into_inner());
            capture.stop();
            capture.sample_rate()
        };
        // Stop finishes in-flight callbacks. Read health only afterward so a
        // failure racing a manual Stop cannot label a partial recording normal.
        let capture_error = health
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .error
            .clone();
        let mut samples = Vec::new();
        {
            let receiver = receiver.lock().unwrap_or_else(|error| error.into_inner());
            while let Ok(chunk) = receiver.try_recv() {
                samples.extend(chunk);
            }
        }
        let sample_count = samples.len();
        let duration_ms = super::wav::captured_duration_ms(sample_count, sample_rate);
        let audio_path = if sample_count >= 4000 || capture_error.is_some() {
            super::wav::write_recording_wav(&path, &samples, sample_rate)?;
            Some(path)
        } else {
            None
        };
        Ok(FinishedCapture {
            sample_count,
            sample_rate,
            duration_ms,
            capture_error,
            audio_path,
        })
    })
    .await
    .map_err(|error| format!("Audio finalization worker failed: {error}"))
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
        sender: Sender<Vec<f32>>,
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
    error_callback: Box<dyn Fn(u64, String) + Send + 'static>,
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
    *state
        .recording_vocabulary
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = state
        .vocabulary_terms
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    if let Ok(receiver) = state.audio_receiver.lock() {
        while receiver.try_recv().is_ok() {}
    }
    let sender = state
        .audio_sender
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
    let on_error = Box::new(move |error: String| {
        let mut health = health.lock().unwrap_or_else(|error| error.into_inner());
        if health.generation != generation || health.error.is_some() {
            return;
        }
        health.error = Some(error.clone());
        drop(health);
        error_callback(generation, error);
    });
    let recording = Arc::new(AtomicBool::new(true));
    state.is_recording.store(true, Ordering::SeqCst);
    let result = state
        .audio_capture
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .start(sender, recording, level_callback, on_error);
    match result {
        Ok(()) => {
            state
                .target_pid
                .store(state.paste_backend.get_frontmost_pid(), Ordering::SeqCst);
            *current = AppStateEnum::Recording;
            Ok(generation)
        }
        Err(error) => {
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
        sender: Sender<Vec<f32>>,
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

                    // Send samples to receiver for transcription
                    // (Vec allocation here is unavoidable — the channel requires owned data)
                    let _ = sender_clone.send(mono.to_vec());

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
                    error_callback(format!("Microphone capture was interrupted: {err}"));
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

    #[tokio::test(flavor = "current_thread")]
    async fn finalization_yields_and_preserves_callbacks_from_stream_shutdown() {
        struct DelayedStop {
            started: Option<tokio::sync::oneshot::Sender<()>>,
            release: std::sync::mpsc::Receiver<()>,
            audio: Option<Sender<Vec<f32>>>,
            error: Option<Box<dyn Fn(String) + Send>>,
        }
        impl AudioCaptureBackend for DelayedStop {
            fn start(
                &mut self,
                sender: Sender<Vec<f32>>,
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
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("shutdown.wav");
        let worker_state = Arc::clone(&state);
        let worker_path = path.clone();
        let worker =
            tokio::spawn(
                async move { finish_recording_capture_to(&worker_state, worker_path).await },
            );
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

    #[tokio::test]
    async fn finalization_reports_write_failure_without_overwriting_and_skips_only_short_capture() {
        for sample_count in [1, 4000] {
            let state = crate::state::AppState::new_with_backends(
                Box::new(crate::test_support::MockAudioCapture::new(
                    vec![0.25; sample_count],
                    16_000,
                )),
                Box::new(crate::test_support::MockAsrEngine::with_text("unused")),
                None,
                Box::new(crate::test_support::MockPasteBackend::new()),
                crate::models::Settings::default(),
            );
            start_recording_capture(&state, Box::new(|_| {})).unwrap();
            assert!(state.claim_recording_end(None));
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("existing.wav");
            std::fs::write(&path, b"preserve existing recording").unwrap();
            let result = finish_recording_capture_to(&state, path.clone()).await;
            assert!(!state.is_recording.load(Ordering::SeqCst));
            if sample_count == 1 {
                assert!(result.unwrap().audio_path.is_none());
            } else {
                assert!(result.unwrap_err().contains("WAV create failed"));
            }
            assert_eq!(std::fs::read(path).unwrap(), b"preserve existing recording");
        }
    }

    #[test]
    fn capture_errors_notify_once_and_old_callbacks_cannot_poison_a_new_recording() {
        use std::sync::Mutex;
        type Errors = Arc<Mutex<Vec<Box<dyn Fn(String) + Send>>>>;
        struct ErrorCapture(Errors);
        impl AudioCaptureBackend for ErrorCapture {
            fn start(
                &mut self,
                _: Sender<Vec<f32>>,
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
