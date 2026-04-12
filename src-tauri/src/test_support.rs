// Mock implementations for integration tests.
// This module is only compiled under `#[cfg(test)]`.

use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use crate::audio::capture::AudioCaptureBackend;
use crate::asr::engine::{AsrEngine, AsrResult};
use crate::llm::engine::LlmBackend;
use crate::paste::PasteBackend;
use crate::pipeline::PipelineEvents;
use crate::models::{AppStateEnum, Transcription};

// ======================== MockAudioCapture ========================

/// Mock audio capture that sends pre-loaded PCM samples when started.
pub struct MockAudioCapture {
    /// Samples to send when start() is called.
    samples: Vec<f32>,
    /// Sample rate to report.
    sample_rate: u32,
}

impl MockAudioCapture {
    /// Create a mock that sends the given samples as a single chunk.
    pub fn new(samples: Vec<f32>, sample_rate: u32) -> Self {
        Self { samples, sample_rate }
    }

    /// Create a mock with 1 second of silence at 48kHz.
    #[allow(dead_code)]
    pub fn silence() -> Self {
        Self::new(vec![0.0f32; 48_000], 48_000)
    }

    /// Create a mock with a synthetic 440Hz sine wave (1 second at 48kHz).
    pub fn sine_wave() -> Self {
        let sample_rate = 48_000u32;
        let samples: Vec<f32> = (0..sample_rate)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / sample_rate as f32).sin())
            .collect();
        Self::new(samples, sample_rate)
    }
}

impl AudioCaptureBackend for MockAudioCapture {
    fn start(
        &mut self,
        sender: Sender<Vec<f32>>,
        _is_recording: Arc<AtomicBool>,
        _level_callback: Box<dyn Fn(f32) + Send + 'static>,
    ) -> Result<(), String> {
        // Send all samples immediately as a single chunk
        sender.send(self.samples.clone())
            .map_err(|e| format!("MockAudioCapture: channel send failed: {}", e))?;
        Ok(())
    }

    fn stop(&mut self) {
        // No-op: mock has no stream to stop
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

// ======================== MockAsrEngine ========================

/// Mock ASR engine that returns a canned transcription.
pub struct MockAsrEngine {
    /// Text to return from transcribe_file/transcribe_samples.
    response: Result<String, String>,
}

impl MockAsrEngine {
    /// Create a mock that returns the given text.
    pub fn with_text(text: &str) -> Self {
        Self {
            response: Ok(text.to_string()),
        }
    }

    /// Create a mock that returns an error.
    pub fn with_error(error: &str) -> Self {
        Self {
            response: Err(error.to_string()),
        }
    }
}

impl AsrEngine for MockAsrEngine {
    fn init(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn is_ready(&self) -> bool {
        true
    }

    fn transcribe_file(&mut self, _path: &str) -> Result<AsrResult, String> {
        match &self.response {
            Ok(text) => Ok(AsrResult {
                text: text.clone(),
                duration_secs: 1.0,
                processing_time_secs: 0.01,
                rtfx: 100.0,
            }),
            Err(e) => Err(e.clone()),
        }
    }

    fn transcribe_samples(&mut self, _samples: &[f32], _sample_rate: u32) -> Result<AsrResult, String> {
        self.transcribe_file("")
    }

    fn is_model_available(&self) -> bool {
        true
    }

    fn backend_name(&self) -> &'static str {
        "mock"
    }
}

// ======================== MockLlmBackend ========================

/// Type alias for LLM transform functions.
type LlmTransformFn = Box<dyn Fn(&str) -> Result<String, String> + Send>;

/// Mock LLM backend that applies a canned transformation.
pub struct MockLlmBackend {
    /// The transformation to apply.
    transform: LlmTransformFn,
}

impl MockLlmBackend {
    /// Create a mock that returns the given fixed text regardless of input.
    pub fn fixed(output: &str) -> Self {
        let output = output.to_string();
        Self {
            transform: Box::new(move |_| Ok(output.clone())),
        }
    }

    /// Create a mock that returns an error.
    pub fn failing(error: &str) -> Self {
        let error = error.to_string();
        Self {
            transform: Box::new(move |_| Err(error.clone())),
        }
    }

    /// Create a mock that passes text through unchanged.
    #[allow(dead_code)]
    pub fn passthrough() -> Self {
        Self {
            transform: Box::new(|text| Ok(text.to_string())),
        }
    }
}

impl LlmBackend for MockLlmBackend {
    fn cleanup(&mut self, text: &str) -> Result<String, String> {
        (self.transform)(text)
    }

    fn request_raw(&mut self, _req: &serde_json::Value) -> Result<serde_json::Value, String> {
        Ok(serde_json::json!({"ok": true}))
    }
}

// ======================== MockPasteBackend ========================

/// Record of a paste operation for test assertion.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PasteRecord {
    pub text: String,
    pub target_pid: i32,
    pub restore_clipboard: bool,
}

/// Mock paste backend that records operations for assertion.
pub struct MockPasteBackend {
    /// All texts that were "pasted".
    pub pasted_texts: Mutex<Vec<PasteRecord>>,
    /// All texts that were "copied" to clipboard.
    pub copied_texts: Mutex<Vec<String>>,
    /// PID to return from get_frontmost_pid.
    pub frontmost_pid: i32,
    /// Whether to report accessibility as trusted.
    pub accessibility_trusted: bool,
    /// If set, paste operations return this error.
    pub paste_error: Mutex<Option<String>>,
}

impl MockPasteBackend {
    pub fn new() -> Self {
        Self {
            pasted_texts: Mutex::new(Vec::new()),
            copied_texts: Mutex::new(Vec::new()),
            frontmost_pid: 12345,
            accessibility_trusted: true,
            paste_error: Mutex::new(None),
        }
    }

    /// Get the most recently pasted text.
    pub fn last_pasted(&self) -> Option<PasteRecord> {
        self.pasted_texts.lock().unwrap().last().cloned()
    }
}

impl PasteBackend for MockPasteBackend {
    fn paste_text(&self, text: &str, target_pid: i32) -> Result<(), String> {
        if let Some(err) = self.paste_error.lock().unwrap().as_ref() {
            return Err(err.clone());
        }
        self.pasted_texts.lock().unwrap().push(PasteRecord {
            text: text.to_string(),
            target_pid,
            restore_clipboard: false,
        });
        Ok(())
    }

    fn paste_text_and_restore(&self, text: &str, target_pid: i32) -> Result<(), String> {
        if let Some(err) = self.paste_error.lock().unwrap().as_ref() {
            return Err(err.clone());
        }
        self.pasted_texts.lock().unwrap().push(PasteRecord {
            text: text.to_string(),
            target_pid,
            restore_clipboard: true,
        });
        Ok(())
    }

    fn copy_to_clipboard(&self, text: &str) -> Result<(), String> {
        self.copied_texts.lock().unwrap().push(text.to_string());
        Ok(())
    }

    fn get_frontmost_pid(&self) -> i32 {
        self.frontmost_pid
    }

    fn is_accessibility_trusted(&self) -> bool {
        self.accessibility_trusted
    }
}

// ======================== SharedMockPaste ========================

/// Wrapper to share a MockPasteBackend via Arc for test assertions.
pub struct SharedMockPaste(pub Arc<MockPasteBackend>);

impl PasteBackend for SharedMockPaste {
    fn paste_text(&self, text: &str, pid: i32) -> Result<(), String> {
        self.0.paste_text(text, pid)
    }
    fn paste_text_and_restore(&self, text: &str, pid: i32) -> Result<(), String> {
        self.0.paste_text_and_restore(text, pid)
    }
    fn copy_to_clipboard(&self, text: &str) -> Result<(), String> {
        self.0.copy_to_clipboard(text)
    }
    fn get_frontmost_pid(&self) -> i32 {
        self.0.get_frontmost_pid()
    }
    fn is_accessibility_trusted(&self) -> bool {
        self.0.is_accessibility_trusted()
    }
}

// ======================== CollectingEvents ========================

/// Collects all pipeline events for assertion in tests.
pub struct CollectingEvents {
    pub state_changes: Mutex<Vec<AppStateEnum>>,
    pub recording_started: Mutex<bool>,
    pub recording_stopped: Mutex<bool>,
    pub recording_cancelled: Mutex<bool>,
    pub transcriptions: Mutex<Vec<Transcription>>,
    pub errors: Mutex<Vec<String>>,
    pub paste_ids: Mutex<Vec<String>>,
    pub paste_errors: Mutex<Vec<(String, String)>>,
}

impl CollectingEvents {
    pub fn new() -> Self {
        Self {
            state_changes: Mutex::new(Vec::new()),
            recording_started: Mutex::new(false),
            recording_stopped: Mutex::new(false),
            recording_cancelled: Mutex::new(false),
            transcriptions: Mutex::new(Vec::new()),
            errors: Mutex::new(Vec::new()),
            paste_ids: Mutex::new(Vec::new()),
            paste_errors: Mutex::new(Vec::new()),
        }
    }
}

impl PipelineEvents for CollectingEvents {
    fn emit_state_changed(&self, state: &AppStateEnum) {
        self.state_changes.lock().unwrap().push(state.clone());
    }
    fn emit_recording_started(&self) {
        *self.recording_started.lock().unwrap() = true;
    }
    fn emit_recording_stopped(&self) {
        *self.recording_stopped.lock().unwrap() = true;
    }
    fn emit_recording_cancelled(&self) {
        *self.recording_cancelled.lock().unwrap() = true;
    }
    fn emit_transcription_complete(&self, t: &Transcription) {
        self.transcriptions.lock().unwrap().push(t.clone());
    }
    fn emit_transcription_error(&self, error: &str) {
        self.errors.lock().unwrap().push(error.to_string());
    }
    fn emit_paste_complete(&self, id: &str) {
        self.paste_ids.lock().unwrap().push(id.to_string());
    }
    fn emit_paste_error(&self, error: &str, text: &str) {
        self.paste_errors.lock().unwrap()
            .push((error.to_string(), text.to_string()));
    }
    fn emit_audio_level(&self, _level: f32) {}
    fn emit_recording_error(&self, error: &str) {
        self.errors.lock().unwrap().push(error.to_string());
    }
}
