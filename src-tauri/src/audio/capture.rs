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

/// Below this many samples a recording is too short to transcribe.
const MIN_RECORDING_SAMPLES: usize = 4000;

/// Stop finishes in-flight callbacks before the writer drains its bounded queue.
pub(crate) async fn finish_recording_capture(
    state: &crate::state::AppState,
) -> Result<FinishedCapture, String> {
    let capture = Arc::clone(&state.audio_capture);
    let health = Arc::clone(&state.capture_health);
    let writer = state.recording_writer.lock().unwrap_or_else(|e| e.into_inner()).take()
        .ok_or("No recording writer is active")?;
    let path = writer.path.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<FinishedCapture, String> {
        let sample_rate = {
            let mut capture = capture.lock().unwrap_or_else(|e| e.into_inner());
            capture.stop();
            capture.sample_rate()
        };
        let written = writer.finish()?;
        let sample_count = written.sample_count;
        let capture_error = health.lock().unwrap_or_else(|e| e.into_inner()).error.clone();
        let duration_ms = super::wav::captured_duration_ms(sample_count, sample_rate);
        if capture_error.is_none() {
            if sample_count < MIN_RECORDING_SAMPLES {
                let _ = std::fs::remove_file(&path);
                return Ok(FinishedCapture { sample_count, sample_rate, duration_ms, capture_error, audio_path: None });
            }
            // Exact zeros come from a denied or muted input, never from a quiet room.
            if written.all_zero {
                let _ = std::fs::remove_file(&path);
                return Err("The microphone sent only silence. Turn on Microphone access for SottoASR in System Settings > Privacy & Security > Microphone, and check that the microphone is not muted.".into());
            }
        }
        Ok(FinishedCapture { sample_count, sample_rate, duration_ms, capture_error, audio_path: Some(path) })
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

    /// The microphone the next start() opens, by cpal device name.
    /// `None` is the system default.
    fn set_input_device(&mut self, _name: Option<String>) {}

    /// True for real hardware. Only live devices get the stall watchdog, so a
    /// slow test run cannot interrupt a scripted capture.
    fn is_live_device(&self) -> bool {
        false
    }

    /// Samples waiting for the writer. A live backend reserves each chunk
    /// before it sends (see `queue_audio`) and stops with "could not keep up"
    /// once more than the queue's headroom is waiting. Scripted backends may
    /// ignore it.
    fn set_queue_gauge(&mut self, _gauge: super::recovery::QueueGauge) {}
}

/// Start capture without a device-error handler. Production always passes one.
#[cfg(test)]
pub(crate) fn start_recording_capture(
    state: &crate::state::AppState,
    level_callback: Box<dyn Fn(f32) + Send + 'static>,
) -> Result<u64, String> {
    start_recording_capture_with_errors(state, level_callback, Box::new(|_, _| {}))
}

/// Shared microphone start for hotkeys and IPC.
/// Keep the state claim until the stream is acquired so two starts cannot race.
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
    let mut capture = state.audio_capture.lock().unwrap_or_else(|error| error.into_inner());
    capture.set_input_device(state.input_device.lock().unwrap_or_else(|error| error.into_inner()).clone());
    let path = super::recovery::recordings_dir()?.join(format!("sotto_{}.wav", uuid::Uuid::new_v4()));
    let (writer, sender, rate_sender) =
        super::recovery::RecordingWriter::start(path.clone(), on_error.clone(), capture.is_live_device())?;
    capture.set_queue_gauge(writer.queue_gauge());
    *state.recording_audio_path.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
    let recording = Arc::new(AtomicBool::new(true));
    state.is_recording.store(true, Ordering::SeqCst);
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
    /// Selected microphone name; `None` follows the system default.
    input_device: Option<String>,
    /// The current recording's writer queue.
    queue_gauge: super::recovery::QueueGauge,
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
            input_device: None,
            queue_gauge: Default::default(),
        }
    }
}

/// Input devices whose name is `name`. Settings saves names trimmed, so both
/// sides are compared without surrounding whitespace.
fn input_devices_named<'a>(
    host: &cpal::Host,
    name: &'a str,
) -> Result<impl Iterator<Item = cpal::Device> + 'a, String> {
    let name = name.trim();
    Ok(host
        .input_devices()
        .map_err(|error| format!("Could not list microphones: {error}"))?
        .filter(move |device| device.name().is_ok_and(|candidate| candidate.trim() == name)))
}

/// Open the microphone through `input_devices()`: cpal attaches its disconnect
/// listener only to devices that did not come from `default_input_device()`.
/// A selected device that is missing is an error, never another microphone.
fn open_input_device(host: &cpal::Host, selected: Option<&str>) -> Result<cpal::Device, String> {
    if let Some(name) = selected {
        return input_devices_named(host, name)?.next().ok_or_else(|| {
            format!("The microphone \"{name}\" is not connected. Connect it, or choose another microphone in Settings.")
        });
    }
    let default = host
        .default_input_device()
        .ok_or("No microphone is available. Connect a microphone, then try again.")?;
    let Ok(default_name) = default.name() else { return Ok(default) };
    let Ok(mut matches) = input_devices_named(host, &default_name) else { return Ok(default) };
    // When several devices share the default's name, only the default handle
    // is certain to be the right one.
    match (matches.next(), matches.next()) {
        (Some(device), None) => Ok(device),
        _ => Ok(default),
    }
}

/// Largest chunk sent to the writer, so each queued allocation stays small.
const MAX_CHUNK_SAMPLES: usize = 4096;

/// Hand captured mono samples to the writer. Fails once more than the queue's
/// headroom of audio at `sample_rate` is waiting, or when the channel is full
/// or closed. Runs on the real-time audio thread: it allocates only the chunk.
fn queue_audio(
    sender: &SyncSender<Vec<f32>>,
    queue: &super::recovery::QueueGauge,
    samples: &[f32],
    sample_rate: u32,
) -> Result<(), String> {
    for chunk in samples.chunks(MAX_CHUNK_SAMPLES) {
        if !queue.reserve(chunk.len(), sample_rate) {
            return Err(format!(
                "Recording storage could not keep up: more than {} seconds of audio was waiting to be saved",
                super::recovery::QUEUE_HEADROOM_SECS
            ));
        }
        if let Err(error) = sender.try_send(chunk.to_vec()) {
            queue.release(chunk.len());
            return Err(format!("Recording storage could not keep up: {error}"));
        }
    }
    Ok(())
}

impl AudioCaptureBackend for AudioCapture {
    fn start(
        &mut self,
        sender: SyncSender<Vec<f32>>,
        is_recording: Arc<AtomicBool>,
        level_callback: Box<dyn Fn(f32) + Send + 'static>,
        error_callback: Box<dyn Fn(String) + Send + 'static>,
    ) -> Result<(), String> {
        use crate::commands::permissions::{microphone_authorization, MicAuthorization};
        match microphone_authorization() {
            MicAuthorization::Denied => {
                return Err("SottoASR does not have Microphone access. Turn it on in System Settings > Privacy & Security > Microphone, then try again.".into());
            }
            MicAuthorization::Restricted => {
                return Err("Microphone access is restricted on this Mac. Check System Settings > Privacy & Security > Microphone, or ask your administrator.".into());
            }
            MicAuthorization::Authorized | MicAuthorization::NotDetermined => {}
        }
        let host = cpal::default_host();
        let device = open_input_device(&host, self.input_device.as_deref())?;

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
        let queue = self.queue_gauge.clone();
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
                    if let Err(error) = queue_audio(&sender_clone, &queue, mono, sample_rate) {
                        is_recording_clone.store(false, Ordering::Relaxed);
                        send_error(error);
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
            // Stop the audio unit explicitly: the microphone must not keep
            // running whatever the library does when the stream is dropped.
            if let Err(error) = stream.pause() {
                log::warn!("Audio capture pause failed: {error}");
            }
            drop(stream);
            log::info!("Audio capture stopped");
        }
    }

    fn sample_rate(&self) -> u32 {
        self.captured_sample_rate
    }

    fn set_input_device(&mut self, name: Option<String>) {
        self.input_device = name;
    }

    fn is_live_device(&self) -> bool {
        true
    }

    fn set_queue_gauge(&mut self, gauge: super::recovery::QueueGauge) {
        self.queue_gauge = gauge;
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
    use std::time::Duration;

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

    fn mock_state(capture: Box<dyn AudioCaptureBackend>) -> crate::state::AppState {
        crate::state::AppState::new_with_backends(
            capture,
            Box::new(crate::test_support::MockAsrEngine::with_text("unused")),
            None,
            Box::new(crate::test_support::MockPasteBackend::new()),
            crate::models::Settings::default(),
        )
    }

    /// Delivers one chunk, then goes quiet with its sender still open.
    struct OneChunkCapture {
        samples: Vec<f32>,
        live: bool,
        sender: Option<SyncSender<Vec<f32>>>,
    }
    impl OneChunkCapture {
        fn new(samples: Vec<f32>, live: bool) -> Box<Self> {
            Box::new(Self { samples, live, sender: None })
        }
    }
    impl AudioCaptureBackend for OneChunkCapture {
        fn start(
            &mut self,
            sender: SyncSender<Vec<f32>>,
            _: Arc<AtomicBool>,
            _: Box<dyn Fn(f32) + Send>,
            _: Box<dyn Fn(String) + Send>,
        ) -> Result<(), String> {
            sender.send(self.samples.clone()).unwrap();
            self.sender = Some(sender);
            Ok(())
        }
        fn stop(&mut self) {
            self.sender = None;
        }
        fn sample_rate(&self) -> u32 {
            16_000
        }
        fn is_live_device(&self) -> bool {
            self.live
        }
    }

    #[tokio::test]
    async fn live_device_that_stops_delivering_interrupts_the_capture_once() {
        crate::audio::recovery::set_test_stall_limits(Duration::from_millis(40), Duration::from_millis(20));
        let state = mock_state(OneChunkCapture::new(vec![0.25; 8_000], true));
        let (notify, notifications) = std::sync::mpsc::channel();
        let generation = start_recording_capture_with_errors(
            &state,
            Box::new(|_| {}),
            Box::new(move |generation: u64, error: String| {
                let _ = notify.send((generation, error));
            }),
        )
        .unwrap();
        let (notified, error) = notifications.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(notified, generation);
        assert!(error.contains("stopped sending audio"));
        // Several more stall periods pass; a repeat would arrive here.
        std::thread::sleep(Duration::from_millis(150));
        assert!(state.claim_recording_end(Some(generation)));
        let finished = finish_recording_capture(&state).await.unwrap();
        assert!(notifications.try_recv().is_err());
        assert_eq!(finished.capture_error, Some(error));
        assert_eq!(finished.sample_count, 8_000);
        assert!(finished.audio_path.is_some_and(|path| path.exists()));
    }

    #[tokio::test]
    async fn scripted_backend_never_trips_the_stall_watchdog() {
        crate::audio::recovery::set_test_stall_limits(Duration::from_millis(20), Duration::from_millis(20));
        let state = mock_state(OneChunkCapture::new(vec![0.25; 8_000], false));
        let (notify, notifications) = std::sync::mpsc::channel();
        let generation = start_recording_capture_with_errors(
            &state,
            Box::new(|_| {}),
            Box::new(move |generation: u64, error: String| {
                let _ = notify.send((generation, error));
            }),
        )
        .unwrap();
        // Far longer than the stall limits.
        std::thread::sleep(Duration::from_millis(150));
        assert!(state.claim_recording_end(Some(generation)));
        let finished = finish_recording_capture(&state).await.unwrap();
        assert!(notifications.try_recv().is_err());
        assert!(finished.capture_error.is_none());
        assert!(finished.audio_path.is_some_and(|path| path.exists()));
    }

    #[tokio::test]
    async fn all_zero_capture_is_an_error_and_removes_its_recording() {
        let state = mock_state(OneChunkCapture::new(vec![0.0; 4_000], false));
        let generation = start_recording_capture(&state, Box::new(|_| {})).unwrap();
        let path = active_recording_path(&state).unwrap();
        assert!(state.claim_recording_end(Some(generation)));
        let error = finish_recording_capture(&state).await.unwrap_err();
        assert!(error.contains("Privacy & Security > Microphone"));
        assert!(error.contains("muted"));
        assert!(!path.exists());
        assert!(!state.is_recording.load(Ordering::SeqCst));
    }

    /// Start and finish one recording; returns its reserved path and the result.
    async fn record(
        state: &crate::state::AppState,
    ) -> (std::path::PathBuf, Result<FinishedCapture, String>) {
        let generation = start_recording_capture(state, Box::new(|_| {})).unwrap();
        let path = active_recording_path(state).unwrap();
        assert!(state.claim_recording_end(Some(generation)));
        (path, finish_recording_capture(state).await)
    }

    #[tokio::test]
    async fn silence_rule_ignores_stop_padding_and_keeps_any_real_input() {
        // Stop pads with zeros; counting them would turn this into a silence error.
        let state = mock_state(OneChunkCapture::new(vec![0.0; 3_999], false));
        let (path, finished) = record(&state).await;
        let finished = finished.unwrap();
        assert_eq!(finished.sample_count, 3_999);
        assert!(finished.capture_error.is_none());
        assert!(finished.audio_path.is_none());
        assert!(!path.exists());

        let mut samples = vec![0.0; 8_000];
        samples[4_321] = 0.001;
        let state = mock_state(OneChunkCapture::new(samples, false));
        let (path, finished) = record(&state).await;
        let finished = finished.unwrap();
        assert_eq!(finished.sample_count, 8_000);
        assert!(finished.capture_error.is_none());
        assert_eq!(finished.audio_path.as_deref(), Some(path.as_path()));
        assert!(path.exists());
    }

    /// Scripted backend only: a real cpal stream needs a microphone.
    #[tokio::test]
    async fn stop_clears_the_recording_and_a_second_start_works() {
        let state = mock_state(OneChunkCapture::new(vec![0.25; 8_000], false));
        for _ in 0..2 {
            let (path, finished) = record(&state).await;
            let finished = finished.unwrap();
            assert!(!state.is_recording.load(Ordering::SeqCst));
            assert!(state.recording_writer.lock().unwrap().is_none());
            assert_eq!(finished.sample_count, 8_000);
            assert_eq!(finished.audio_path, Some(path));
            // The stop path returns to Idle after delivery.
            state.set_state(crate::models::AppStateEnum::Idle);
        }
    }

    /// Honours the gauge through `queue_audio`, as the cpal callback does. Its
    /// start() runs before the writer receives the sample rate, so the writer
    /// is held back and dequeues nothing while it sends.
    struct GaugedCapture {
        gauge: Arc<std::sync::Mutex<Option<crate::audio::recovery::QueueGauge>>>,
        accepted: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl AudioCaptureBackend for GaugedCapture {
        fn start(
            &mut self,
            sender: SyncSender<Vec<f32>>,
            recording: Arc<AtomicBool>,
            _: Box<dyn Fn(f32) + Send>,
            error: Box<dyn Fn(String) + Send>,
        ) -> Result<(), String> {
            let gauge = self.gauge.lock().unwrap().clone().unwrap();
            // 100 ms callbacks at 16 kHz: the 101st would leave 10.1 s waiting.
            let callback = vec![0.25; 1_600];
            for _ in 0..101 {
                if let Err(message) = queue_audio(&sender, &gauge, &callback, 16_000) {
                    recording.store(false, Ordering::Relaxed);
                    error(message);
                    break;
                }
                self.accepted.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
        fn stop(&mut self) {}
        fn sample_rate(&self) -> u32 {
            16_000
        }
        fn set_queue_gauge(&mut self, gauge: crate::audio::recovery::QueueGauge) {
            *self.gauge.lock().unwrap() = Some(gauge);
        }
    }

    #[tokio::test]
    async fn queue_reports_it_could_not_keep_up_only_past_ten_seconds_of_backlog() {
        let gauge = Arc::new(std::sync::Mutex::new(None));
        let accepted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let state = mock_state(Box::new(GaugedCapture {
            gauge: gauge.clone(),
            accepted: accepted.clone(),
        }));
        let (notify, notifications) = std::sync::mpsc::channel();
        let generation = start_recording_capture_with_errors(
            &state,
            Box::new(|_| {}),
            Box::new(move |generation: u64, error: String| {
                let _ = notify.send((generation, error));
            }),
        )
        .unwrap();
        // Exactly 10 s were accepted before the refusal.
        assert_eq!(accepted.load(Ordering::SeqCst), 100);
        let (notified, error) = notifications.try_recv().unwrap();
        assert_eq!(notified, generation);
        assert!(error.contains("could not keep up"));
        assert!(state.claim_recording_end(Some(generation)));
        let finished = finish_recording_capture(&state).await.unwrap();
        assert!(notifications.try_recv().is_err());
        assert_eq!(finished.capture_error, Some(error));
        assert_eq!(finished.sample_count, 160_000);
        // The writer released every chunk it dequeued.
        assert_eq!(gauge.lock().unwrap().as_ref().unwrap().queued(), 0);
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
