//! FluidAudio ASR backend — macOS only.
//!
//! Uses CoreML models running on the Apple Neural Engine for
//! maximum performance (~190x real-time on M4 Pro).
//!
//! Model download is handled automatically by FluidAudio's Swift layer
//! on first call to `init_asr()`. Models are cached at:
//!   ~/Library/Application Support/FluidAudio/Models/
//!
//! Reference: https://github.com/FluidInference/fluidaudio-rs

use super::engine::{AsrEngine, AsrResult};
use std::path::PathBuf;

/// FluidAudio-powered ASR engine.
/// Wraps fluidaudio-rs which bridges to the FluidAudio Swift SDK via C FFI.
pub struct FluidAudioEngine {
    audio: Option<fluidaudio_rs::FluidAudio>,
    ready: bool,
}

impl FluidAudioEngine {
    pub fn new() -> Self {
        Self {
            audio: None,
            ready: false,
        }
    }

    /// Get the model cache directory used by FluidAudio.
    fn model_cache_dir() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("~"))
            .join("FluidAudio")
            .join("Models")
    }
}

impl AsrEngine for FluidAudioEngine {
    fn init(&mut self) -> Result<(), String> {
        if self.ready {
            return Ok(());
        }

        log::info!("Initializing FluidAudio ASR engine...");
        log::info!("Models will be cached at: {:?}", Self::model_cache_dir());

        // Create the FluidAudio bridge
        let audio = fluidaudio_rs::FluidAudio::new()
            .map_err(|e| format!("Failed to create FluidAudio: {:?}", e))?;

        // Log system info
        let info = audio.system_info();
        log::info!(
            "System: {} {} ({:.0} GB RAM, Apple Silicon: {})",
            info.platform, info.chip_name, info.memory_gb, info.is_apple_silicon
        );

        if !audio.is_apple_silicon() {
            log::warn!("Intel Mac detected — ASR may not be available or will run on CPU");
        }

        // Initialize ASR — this downloads + compiles CoreML models on first run.
        // BLOCKS for 20-30s on first launch, ~1s on subsequent launches.
        log::info!("Loading ASR models (first run downloads ~500 MB from HuggingFace)...");
        audio.init_asr()
            .map_err(|e| format!("Failed to initialize ASR: {:?}", e))?;

        if !audio.is_asr_available() {
            return Err("ASR initialization completed but ASR is not available".into());
        }

        log::info!("FluidAudio ASR ready (Parakeet TDT v3, CoreML/ANE)");
        self.audio = Some(audio);
        self.ready = true;
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.ready
    }

    fn transcribe_file(&mut self, path: &str) -> Result<AsrResult, String> {
        let audio = self.audio.as_ref()
            .ok_or("FluidAudio not initialized. Call init() first.")?;

        let result = audio.transcribe_file(path)
            .map_err(|e| format!("Transcription failed: {:?}", e))?;

        Ok(AsrResult {
            text: result.text,
            duration_secs: result.duration,
            processing_time_secs: result.processing_time,
            rtfx: result.rtfx,
        })
    }

    fn transcribe_samples(&mut self, samples: &[f32], sample_rate: u32) -> Result<AsrResult, String> {
        // FluidAudio's Rust API only supports file-based transcription.
        // Write samples to a temp WAV file, transcribe it, then delete.
        let temp_dir = std::env::temp_dir();
        let temp_path = temp_dir.join(format!("sotto_recording_{}.wav", uuid::Uuid::new_v4()));

        // Write WAV file
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };

        let mut writer = hound::WavWriter::create(&temp_path, spec)
            .map_err(|e| format!("Failed to create temp WAV: {}", e))?;

        for &sample in samples {
            writer.write_sample(sample)
                .map_err(|e| format!("Failed to write sample: {}", e))?;
        }
        writer.finalize()
            .map_err(|e| format!("Failed to finalize WAV: {}", e))?;

        // Transcribe the file
        let result = self.transcribe_file(
            temp_path.to_str().ok_or("Invalid temp path")?
        );

        // Clean up temp file
        let _ = std::fs::remove_file(&temp_path);

        result
    }

    fn is_model_available(&self) -> bool {
        // FluidAudio manages its own model cache.
        // Models are at ~/Library/Application Support/FluidAudio/Models/
        let model_dir = Self::model_cache_dir();
        let asr_dir = model_dir.join("parakeet-tdt-0.6b-v3-coreml");
        asr_dir.exists()
    }

    fn backend_name(&self) -> &'static str {
        "FluidAudio (CoreML/ANE)"
    }
}
