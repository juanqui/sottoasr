use std::sync::Arc;
use tokio::sync::Mutex;

/// Unified ASR engine trait.
/// Both FluidAudio (CoreML/ANE) and parakeet-rs (ONNX/CPU) implement this.
#[allow(dead_code)]
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

    /// Apply saved canonical vocabulary if the backend supports it and its
    /// auxiliary model is already prepared. Inference must never download.
    fn transcribe_file_with_vocabulary(&mut self, path: &str, _terms: &[String]) -> Result<AsrResult, String> {
        self.transcribe_file(path)
    }

    fn install_vocabulary(&mut self, _model: PreparedVocabulary) -> Result<(), String> {
        Err("Vocabulary assistance is unavailable for this ASR backend".into())
    }

    fn unload_vocabulary(&mut self) {}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unboosted_text: Option<String>,
    pub duration_secs: f64,
    pub processing_time_secs: f64,
    pub rtfx: f32,
}

pub enum PreparedVocabulary {
    #[cfg(feature = "asr-fluidaudio")]
    FluidAudio(fluidaudio_rs::VocabularyModel),
    #[cfg(test)]
    Mock,
}

impl PreparedVocabulary {
    pub fn load(allow_download: bool) -> Result<Self, String> {
        #[cfg(feature = "asr-fluidaudio")]
        { fluidaudio_rs::VocabularyModel::load(allow_download).map(Self::FluidAudio) }
        #[cfg(not(feature = "asr-fluidaudio"))]
        { let _ = allow_download; Err("Vocabulary assistance is unavailable for this ASR backend".into()) }
    }
}

/// Download and compile away from the resident decoder. Only installation
/// enters its critical section, and a cleared list drops the detached resource.
pub async fn prepare_and_attach(
    engine: &SharedAsrEngine,
    terms: &Arc<std::sync::Mutex<Vec<String>>>,
    load: impl FnOnce() -> Result<PreparedVocabulary, String> + Send + 'static,
) -> Result<bool, String> {
    let model = tokio::task::spawn_blocking(load).await
        .map_err(|error| format!("Vocabulary preparation worker failed: {error}"))??;
    let terms = Arc::clone(terms);
    with_engine(engine, move |engine| {
        let terms = terms.lock().unwrap_or_else(|error| error.into_inner());
        if terms.is_empty() { return Ok(false); }
        engine.install_vocabulary(model)?;
        Ok(true)
    }).await
}

/// The engine remains exclusively owned while a blocking worker accesses it.
/// Waiting callers yield to Tokio; the Swift semaphore never blocks its workers.
pub type SharedAsrEngine = Arc<Mutex<Box<dyn AsrEngine>>>;

pub async fn with_engine<T: Send + 'static>(
    engine: &SharedAsrEngine,
    operation: impl FnOnce(&mut dyn AsrEngine) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let engine = Arc::clone(engine);
    tokio::task::spawn_blocking(move || operation(engine.blocking_lock().as_mut()))
        .await
        .map_err(|error| format!("ASR worker failed: {error}"))?
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn detached_preparation_does_not_block_asr_or_resurrect_cleared_terms() {
        let engine: SharedAsrEngine = Arc::new(Mutex::new(Box::new(
            crate::test_support::MockAsrEngine::with_text("Complete recording."),
        )));
        let terms = Arc::new(std::sync::Mutex::new(vec!["Qwen".into()]));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let task_engine = Arc::clone(&engine);
        let task_terms = Arc::clone(&terms);
        let preparation = tokio::spawn(async move {
            prepare_and_attach(&task_engine, &task_terms, move || {
                let _ = started_tx.send(());
                release_rx.recv_timeout(std::time::Duration::from_secs(2))
                    .map_err(|_| "ASR waited behind setup".to_owned())?;
                Ok(PreparedVocabulary::Mock)
            }).await
        });
        started_rx.await.unwrap();
        let result = with_engine(&engine, |engine| engine.transcribe_file("synthetic.wav")).await.unwrap();
        assert_eq!(result.text, "Complete recording.");
        terms.lock().unwrap().clear();
        release_tx.send(()).unwrap();
        assert!(!preparation.await.unwrap().unwrap());
        terms.lock().unwrap().push("Qwen".into());
        assert!(prepare_and_attach(&engine, &terms, || Ok(PreparedVocabulary::Mock)).await.unwrap());
        assert!(prepare_and_attach(&engine, &terms, || Err("download interrupted".into())).await.is_err());
        assert_eq!(with_engine(&engine, |engine| engine.transcribe_file("synthetic.wav")).await.unwrap().text, "Complete recording.");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn blocking_asr_keeps_the_async_runtime_responsive() {
        let engine: SharedAsrEngine = Arc::new(Mutex::new(Box::new(
            crate::test_support::MockAsrEngine::with_text("fixture"),
        )));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let task_engine = Arc::clone(&engine);
        let operation = tokio::spawn(async move {
            with_engine(&task_engine, move |_| {
                let _ = started_tx.send(());
                release_rx.recv_timeout(std::time::Duration::from_secs(2))
                    .map_err(|_| "async runtime was blocked".to_owned())
            }).await
        });
        started_rx.await.unwrap();
        // This can release the worker only if the current-thread runtime was
        // free to poll us while synchronous ASR was still running.
        assert!(engine.try_lock().is_err());
        release_tx.send(()).unwrap();
        operation.await.unwrap().unwrap();
        assert!(engine.try_lock().is_ok());
    }
}
