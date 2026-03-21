/// Unified ASR engine trait.
/// Both FluidAudio (CoreML/ANE) and parakeet-rs (ONNX/CPU) implement this.
pub trait AsrEngine: Send {
    /// Initialize the engine and load/download models.
    /// For FluidAudio: triggers CoreML model download + Neural Engine compilation on first run.
    /// For parakeet-rs: loads ONNX model from disk (must be pre-downloaded).
    fn init(&mut self) -> Result<(), String>;

    /// Check if the engine is ready for transcription.
    fn is_ready(&self) -> bool;

    /// Transcribe audio from a WAV file path.
    /// Returns the transcribed text.
    fn transcribe_file(&mut self, path: &str) -> Result<AsrResult, String>;

    /// Transcribe raw audio samples (16kHz mono f32).
    /// Not all backends support this — FluidAudio requires a file path.
    fn transcribe_samples(&mut self, samples: &[f32], sample_rate: u32) -> Result<AsrResult, String>;

    /// Check if the model files are present (downloaded/cached).
    fn is_model_available(&self) -> bool;

    /// Get a human-readable name for this backend.
    fn backend_name(&self) -> &'static str;
}

/// Result from an ASR transcription.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AsrResult {
    pub text: String,
    pub duration_secs: f64,
    pub processing_time_secs: f64,
    pub rtfx: f32,
}

/// Create the ASR engine for the current build configuration.
/// - With `asr-fluidaudio` feature: FluidAudio (CoreML/ANE) — macOS only, best performance
/// - With `asr-parakeet` feature: parakeet-rs (ONNX Runtime) — cross-platform, CPU
/// - Neither: placeholder that returns an error
pub fn create_engine() -> Box<dyn AsrEngine> {
    #[cfg(feature = "asr-fluidaudio")]
    {
        log::info!("ASR backend: FluidAudio (CoreML / Apple Neural Engine)");
        Box::new(super::fluidaudio_backend::FluidAudioEngine::new())
    }

    #[cfg(all(feature = "asr-parakeet", not(feature = "asr-fluidaudio")))]
    {
        log::info!("ASR backend: parakeet-rs (ONNX Runtime / CPU)");
        Box::new(super::parakeet_backend::ParakeetEngine::new())
    }

    #[cfg(not(any(feature = "asr-fluidaudio", feature = "asr-parakeet")))]
    {
        log::warn!("No ASR backend enabled! Enable 'asr-fluidaudio' or 'asr-parakeet' feature.");
        Box::new(NoOpEngine)
    }
}

/// Fallback engine when no ASR backend feature is enabled.
#[cfg(not(any(feature = "asr-fluidaudio", feature = "asr-parakeet")))]
struct NoOpEngine;

#[cfg(not(any(feature = "asr-fluidaudio", feature = "asr-parakeet")))]
impl AsrEngine for NoOpEngine {
    fn init(&mut self) -> Result<(), String> {
        Err("No ASR backend compiled. Rebuild with --features asr-fluidaudio or asr-parakeet".into())
    }
    fn is_ready(&self) -> bool { false }
    fn transcribe_file(&mut self, _: &str) -> Result<AsrResult, String> {
        Err("No ASR backend available".into())
    }
    fn transcribe_samples(&mut self, _: &[f32], _: u32) -> Result<AsrResult, String> {
        Err("No ASR backend available".into())
    }
    fn is_model_available(&self) -> bool { false }
    fn backend_name(&self) -> &'static str { "none" }
}
