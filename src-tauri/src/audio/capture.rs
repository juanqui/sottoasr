use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::mpsc::Sender;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct AudioCapture {
    stream: Option<cpal::Stream>,
}

// Safety: cpal::Stream manages its own audio thread internally.
// We only access AudioCapture from behind a Mutex in AppState.
unsafe impl Send for AudioCapture {}

impl AudioCapture {
    pub fn new() -> Self {
        Self { stream: None }
    }

    pub fn start(
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
        let sender_clone = sender.clone();
        let is_recording_clone = is_recording.clone();

        // Level metering: accumulate ~33ms of samples, then emit RMS
        let level_window = sample_rate as usize / 30; // ~1600 samples at 48kHz
        let mut level_buffer = Vec::with_capacity(level_window);
        let mut level_emit_count: u64 = 0;

        let stream = device
            .build_input_stream(
                &config.into(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if !is_recording_clone.load(Ordering::Relaxed) {
                        return;
                    }

                    // Downmix to mono if needed
                    let mono: Vec<f32> = if channels > 1 {
                        data.chunks(channels)
                            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
                            .collect()
                    } else {
                        data.to_vec()
                    };

                    // Send samples to receiver for transcription
                    let _ = sender_clone.send(mono.clone());

                    // Calculate audio level for waveform visualization
                    level_buffer.extend_from_slice(&mono);
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

    pub fn stop(&mut self) {
        if let Some(stream) = self.stream.take() {
            drop(stream);
            log::info!("Audio capture stopped");
        }
    }
}

fn calculate_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|s| s * s).sum();
    (sum / samples.len() as f32).sqrt().min(1.0)
}
