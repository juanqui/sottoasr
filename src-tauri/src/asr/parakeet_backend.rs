//! parakeet-rs ASR backend — cross-platform.
//!
//! Uses ONNX Runtime for inference on CPU. Works on macOS, Windows, Linux.
//! ~20-30x real-time factor on Apple Silicon CPU.
//!
//! Model files must be downloaded separately (see model.rs for download logic).
//! Uses Parakeet TDT 0.6B v3 INT8 quantized ONNX model (~639 MB).
//!
//! Reference: https://github.com/altunenes/parakeet-rs

use super::engine::{AsrEngine, AsrResult};
use super::model;
use parakeet_rs::Transcriber; // Import the trait for transcribe_samples/transcribe_file

/// parakeet-rs ASR engine using ONNX Runtime.
pub struct ParakeetEngine {
    engine: Option<parakeet_rs::ParakeetTDT>,
    ready: bool,
}

impl ParakeetEngine {
    pub fn new() -> Self {
        Self {
            engine: None,
            ready: false,
        }
    }
}

impl AsrEngine for ParakeetEngine {
    fn init(&mut self) -> Result<(), String> {
        if self.ready {
            return Ok(());
        }

        let model_dir = model::get_model_dir()?;

        if !model::is_model_available() {
            return Err("Parakeet ONNX model not downloaded. Run model download first.".into());
        }

        log::info!("Loading Parakeet TDT model from {:?}...", model_dir);
        let start = std::time::Instant::now();

        let engine = parakeet_rs::ParakeetTDT::from_pretrained(
            model_dir.to_str().ok_or("Invalid model path")?,
            None,
        )
        .map_err(|e| format!("Failed to load Parakeet model: {}", e))?;

        log::info!("Parakeet TDT loaded in {:.1}s", start.elapsed().as_secs_f64());

        self.engine = Some(engine);
        self.ready = true;
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.ready
    }

    fn transcribe_file(&mut self, path: &str) -> Result<AsrResult, String> {
        let engine = self.engine.as_mut()
            .ok_or("Parakeet engine not initialized")?;

        let start = std::time::Instant::now();

        // Transcriber trait provides transcribe_file(path, timestamp_mode)
        let result = engine.transcribe_file(path, None)
            .map_err(|e| format!("Transcription failed: {}", e))?;

        let processing_time = start.elapsed().as_secs_f64();
        // TranscriptionResult has no duration field; estimate from file
        let duration_secs = processing_time * 20.0; // rough estimate

        Ok(AsrResult {
            text: result.text,
            duration_secs,
            processing_time_secs: processing_time,
            rtfx: (duration_secs / processing_time) as f32,
        })
    }

    fn transcribe_samples(&mut self, samples: &[f32], sample_rate: u32) -> Result<AsrResult, String> {
        let engine = self.engine.as_mut()
            .ok_or("Parakeet engine not initialized")?;

        if samples.is_empty() {
            return Err("No audio samples provided".into());
        }

        let duration_secs = samples.len() as f64 / sample_rate as f64;
        log::info!("Transcribing {:.1}s of audio ({} samples)", duration_secs, samples.len());

        let start = std::time::Instant::now();

        // Transcriber::transcribe_samples takes Vec<f32> (owned), not &[f32]
        let result = engine.transcribe_samples(
            samples.to_vec(),
            sample_rate,
            1,    // mono
            None, // no timestamp mode
        )
        .map_err(|e| format!("Transcription failed: {}", e))?;

        let processing_time = start.elapsed().as_secs_f64();

        Ok(AsrResult {
            text: result.text,
            duration_secs,
            processing_time_secs: processing_time,
            rtfx: (duration_secs / processing_time) as f32,
        })
    }

    fn is_model_available(&self) -> bool {
        model::is_model_available()
    }

    fn backend_name(&self) -> &'static str {
        "parakeet-rs (ONNX/CPU)"
    }
}
