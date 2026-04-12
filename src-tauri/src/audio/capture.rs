use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::mpsc::Sender;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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
    ) -> Result<(), String>;

    /// Stop capturing. Must be idempotent (calling stop when not started is a no-op).
    fn stop(&mut self);

    /// The sample rate of the captured audio. Valid after start() succeeds.
    /// Returns the rate used by the cpal stream (production) or a fixed value (tests).
    fn sample_rate(&self) -> u32;
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
                            log::info!("Audio level: {:.4} (emit #{})", rms, level_emit_count);
                        }
                    }
                },
                move |err| {
                    log::error!("Audio capture error: {}", err);
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
