//! FluidAudio ASR backend — macOS only.
//!
//! Uses Parakeet Ultra with CoreML's CPU/Neural Engine configuration. An
//! installation that still has only Parakeet TDT v3 uses it until Ultra is
//! downloaded and verified. Reproducible local accuracy and performance
//! evidence lives in benchmarks/asr.
//!
//! `model::prepare_speech_model` chooses the model, downloads missing files
//! at the pinned revision and verifies them before `init()` loads them.
//! Models are cached at:
//!   ~/Library/Application Support/FluidAudio/Models/
//!
//! Reference: https://github.com/FluidInference/fluidaudio-rs

use super::engine::{AsrEngine, AsrResult, PreparedVocabulary};

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

    /// Loads the verified files of the selected speech model; `replace` loads
    /// them over a loaded model.
    fn load(&mut self, replace: bool) -> Result<(), String> {
        let (directory, name) = super::model::selected_speech_model()
            .ok_or("Could not find your Application Support folder.")?;
        log::info!("Loading {name} from {}", directory.display());

        // Create the FluidAudio bridge
        if self.audio.is_none() {
            self.audio = Some(fluidaudio_rs::FluidAudio::new()
                .map_err(|e| format!("Failed to create FluidAudio: {:?}", e))?);
        }
        let audio = self.audio.as_mut().ok_or("FluidAudio bridge unavailable")?;

        if !audio.is_apple_silicon() {
            log::warn!("Intel Mac detected — ASR may not be available or will run on CPU");
        }

        // Runs on a blocking worker. Loading never downloads, and failed
        // CoreML loads preserve cached model files.
        self.ready = false;
        let loaded = if replace { audio.reload_asr(&directory) } else { audio.init_asr(&directory) };
        loaded.map_err(|e| format!("Failed to initialize ASR: {:?}", e))?;

        if !audio.is_asr_available() {
            return Err("ASR initialization completed but ASR is not available".into());
        }

        log::info!("FluidAudio 0.18.3 ready ({name}, CoreML/ANE)");
        self.ready = true;
        Ok(())
    }
}

impl AsrEngine for FluidAudioEngine {
    fn init(&mut self) -> Result<(), String> {
        if self.ready {
            return Ok(());
        }
        self.load(false)
    }

    fn reload(&mut self) -> Result<(), String> {
        self.load(true)
    }

    fn is_ready(&self) -> bool {
        self.ready
    }

    fn transcribe_file(&mut self, path: &str) -> Result<AsrResult, String> {
        self.transcribe_file_with_vocabulary(path, &[])
    }

    fn install_vocabulary(&mut self, model: PreparedVocabulary) -> Result<(), String> {
        if self.audio.is_none() {
            self.audio = Some(fluidaudio_rs::FluidAudio::new()?);
        }
        match model {
            PreparedVocabulary::FluidAudio(model) => {
                self.audio.as_mut().ok_or("FluidAudio bridge unavailable")?.attach_vocabulary(model);
                Ok(())
            }
            #[cfg(test)]
            PreparedVocabulary::Mock => Err("Cannot install a mock in the real ASR backend".into()),
        }
    }

    fn unload_vocabulary(&mut self) {
        if let Some(audio) = self.audio.as_mut() { audio.unload_vocabulary(); }
    }

    fn transcribe_file_with_vocabulary(&mut self, path: &str, terms: &[String]) -> Result<AsrResult, String> {
        let audio = self.audio.as_mut()
            .ok_or("FluidAudio not initialized. Call init() first.")?;
        // The SDK reads the vocabulary tokenizer from disk for each boosted
        // dictation. While its folder is being replaced, transcribe unboosted.
        let files = if terms.is_empty() { None } else { super::model::VOCABULARY_FILES.try_read().ok() };
        let terms: &[String] = if files.is_some() { terms } else { &[] };
        let started = std::time::Instant::now();
        let result = audio.transcribe_file_with_vocabulary(path, terms)
            .map_err(|e| format!("Transcription failed: {e}"))?;
        drop(files);
        let candidates = serde_json::from_str::<Vec<super::vocabulary::Candidate>>(&result.vocabulary_candidates)
            .unwrap_or_default();
        let text = super::vocabulary::apply_candidates(&result.text, terms, &candidates);
        let elapsed = started.elapsed().as_secs_f64();
        Ok(AsrResult {
            unboosted_text: (text != result.text).then_some(result.text),
            text,
            duration_secs: result.duration,
            processing_time_secs: elapsed,
            rtfx: if elapsed > 0.0 { (result.duration / elapsed) as f32 } else { 0.0 },
        })
    }

    fn transcribe_samples(&mut self, samples: &[f32], sample_rate: u32) -> Result<AsrResult, String> {
        // FluidAudio's Rust API only supports file-based transcription.
        // Write samples to a temp WAV file, transcribe it, then delete.
        let temp_dir = std::env::temp_dir();
        let temp_path = temp_dir.join(format!("sotto_recording_{}.wav", uuid::Uuid::new_v4()));

        crate::audio::wav::write_recording_wav(&temp_path, samples, sample_rate)?;

        // Transcribe the file
        let result = self.transcribe_file(
            temp_path.to_str().ok_or("Invalid temp path")?
        );

        crate::audio::wav::finish_recording_wav(&temp_path, result)
    }

    fn is_model_available(&self) -> bool {
        super::model::is_model_available()
    }

    fn backend_name(&self) -> &'static str {
        "FluidAudio (CoreML/ANE)"
    }
}
