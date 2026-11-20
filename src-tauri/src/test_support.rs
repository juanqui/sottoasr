// Mock implementations for integration tests.
// This module is only compiled under `#[cfg(test)]`.

use std::sync::atomic::AtomicBool;
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};

use crate::audio::capture::AudioCaptureBackend;
use crate::asr::engine::{AsrEngine, AsrResult};
use crate::llm::engine::{BatchItem, LlmBackend};
use crate::paste::PasteBackend;
use crate::pipeline::{Failure, Linger, Outcome, PipelineEvents};
use crate::models::{AppStateEnum, LlmCleanupStatus, Transcription};

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

    /// Create a mock with 1 second of near-silence at 48kHz. Real microphones
    /// never deliver exact zeros, and an all-zero capture is an error.
    pub fn silence() -> Self {
        Self::new(low_level_noise(48_000), 48_000)
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

/// Deterministic, inaudible, never-zero samples that stand in for a quiet room.
pub fn low_level_noise(len: usize) -> Vec<f32> {
    [1.0e-4_f32, -1.0e-4].into_iter().cycle().take(len).collect()
}

impl AudioCaptureBackend for MockAudioCapture {
    fn start(
        &mut self,
        sender: SyncSender<Vec<f32>>,
        _is_recording: Arc<AtomicBool>,
        _level_callback: Box<dyn Fn(f32) + Send + 'static>,
        _error_callback: Box<dyn Fn(String) + Send + 'static>,
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
    vocabulary_response: Option<(Vec<String>, String)>,
}

impl MockAsrEngine {
    /// Create a mock that returns the given text.
    pub fn with_text(text: &str) -> Self {
        Self {
            response: Ok(text.to_string()),
            vocabulary_response: None,
        }
    }

    pub fn with_vocabulary(base: &str, terms: &[&str], text: &str) -> Self {
        Self {
            response: Ok(base.into()),
            vocabulary_response: Some((terms.iter().map(|term| (*term).to_owned()).collect(), text.into())),
        }
    }

    /// Create a mock that returns an error.
    pub fn with_error(error: &str) -> Self {
        Self {
            response: Err(error.to_string()),
            vocabulary_response: None,
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
            Ok(text) => Ok(AsrResult { unboosted_text: None,
                text: text.clone(),
                duration_secs: 1.0,
                processing_time_secs: 0.01,
                rtfx: 100.0,
            }),
            Err(e) => Err(e.clone()),
        }
    }

    fn transcribe_file_with_vocabulary(&mut self, path: &str, terms: &[String]) -> Result<AsrResult, String> {
        let mut result = self.transcribe_file(path)?;
        if let Some((expected, boosted)) = &self.vocabulary_response {
            if terms == expected && *boosted != result.text {
                result.unboosted_text = Some(result.text);
                result.text = boosted.clone();
            }
        }
        Ok(result)
    }

    fn install_vocabulary(&mut self, _: crate::asr::engine::PreparedVocabulary) -> Result<(), String> {
        Ok(())
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

/// Mock LLM backend that applies a canned transformation per batch text.
pub struct MockLlmBackend {
    /// The transformation to apply.
    transform: LlmTransformFn,
}

impl MockLlmBackend {
    /// Create a mock that proposes the same complete, untrusted text for
    /// every batch item (the old serial "proposal" semantics, batched).
    pub fn proposal(text: &str) -> Self {
        let output = text.to_string();
        Self { transform: Box::new(move |_| Ok(output.clone())) }
    }

    /// Create a mock whose batch request fails as a whole (endpoint/transport
    /// fault). Batch-level `Err` is the retire/typed channel — the planner
    /// never receives per-item values from it.
    pub fn failing(error: &str) -> Self {
        let error = error.to_string();
        Self {
            transform: Box::new(move |_| Err(error.clone())),
        }
    }

    /// Create a mock that passes every text through unchanged.
    #[allow(dead_code)]
    pub fn passthrough() -> Self {
        Self {
            transform: Box::new(|text| Ok(text.to_string())),
        }
    }

    /// Create a mock from an arbitrary per-text transform. Tests that need
    /// window-aware proposals (the planner sends window keys, not the whole
    /// transcript) build the transform themselves.
    pub fn run(f: impl Fn(&str) -> Result<String, String> + Send + 'static) -> Self {
        Self { transform: Box::new(f) }
    }
}

impl LlmBackend for MockLlmBackend {
    fn cleanup_batch(&mut self, texts: &[String], _mode: crate::models::CleanupMode) -> Result<Vec<BatchItem>, String> {
        let mut items = Vec::with_capacity(texts.len());
        for text in texts {
            match (self.transform)(text) {
                // First batch-level fault aborts the request — the same
                // whole-request `Err` channel the real client exposes.
                Err(e) => return Err(e),
                Ok(proposal) => items.push(BatchItem::Proposal(Some(proposal))),
            }
        }
        Ok(items)
    }

    fn request_raw(&mut self, _req: &serde_json::Value) -> Result<serde_json::Value, String> {
        Ok(serde_json::json!({"ok": true}))
    }
}

/// A backend that says it serves Amazon Bedrock with `options`. It makes no
/// request: each text comes back as its own proposal, or the batch fails
/// with `failure`.
#[derive(Default)]
pub struct CloudCleanupStandIn {
    pub options: crate::models::AwsBedrockSettings,
    pub batches: Arc<std::sync::atomic::AtomicUsize>,
    pub failure: Option<String>,
}

impl LlmBackend for CloudCleanupStandIn {
    fn cleanup_batch(&mut self, texts: &[String], _mode: crate::models::CleanupMode) -> Result<Vec<BatchItem>, String> {
        self.batches.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        match &self.failure {
            Some(failure) => Err(failure.clone()),
            None => Ok(texts.iter().map(|text| BatchItem::Proposal(Some(text.clone()))).collect()),
        }
    }

    fn request_raw(&mut self, _req: &serde_json::Value) -> Result<serde_json::Value, String> {
        Err("a cloud backend has no model to load".into())
    }

    fn bedrock_options(&self) -> Option<&crate::models::AwsBedrockSettings> {
        Some(&self.options)
    }
}

/// A stand-in for the local sidecar that records its shutdown and each batch.
#[derive(Default)]
pub struct StoppableSidecar {
    pub stopped: Arc<AtomicBool>,
    pub batches: Arc<std::sync::atomic::AtomicUsize>,
}

impl LlmBackend for StoppableSidecar {
    fn cleanup_batch(&mut self, texts: &[String], _mode: crate::models::CleanupMode) -> Result<Vec<BatchItem>, String> {
        self.batches.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(texts.iter().map(|text| BatchItem::Proposal(Some(text.clone()))).collect())
    }

    fn request_raw(&mut self, _req: &serde_json::Value) -> Result<serde_json::Value, String> {
        Ok(serde_json::json!({"ok": true}))
    }

    fn shutdown(&mut self) {
        self.stopped.store(true, std::sync::atomic::Ordering::SeqCst);
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
    pub copy_error: Mutex<Option<String>>,
}

impl MockPasteBackend {
    pub fn new() -> Self {
        Self {
            pasted_texts: Mutex::new(Vec::new()),
            copied_texts: Mutex::new(Vec::new()),
            frontmost_pid: 12345,
            accessibility_trusted: true,
            paste_error: Mutex::new(None),
            copy_error: Mutex::new(None),
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
        if let Some(error) = self.copy_error.lock().unwrap().as_ref() { return Err(error.clone()); }
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
#[derive(Default)]
pub struct CollectingEvents {
    pub state_changes: Mutex<Vec<AppStateEnum>>,
    pub busy: Mutex<Vec<AppStateEnum>>,
    pub recording_started: Mutex<bool>,
    pub recording_stopped: Mutex<bool>,
    pub recording_cancelled: Mutex<bool>,
    pub time_warnings: Mutex<Vec<u64>>,
    pub failures: Mutex<Vec<Failure>>,
    pub cleanup_statuses: Mutex<Vec<LlmCleanupStatus>>,
    /// `(id, clipboard_only)` for every completed delivery.
    pub paste_ids: Mutex<Vec<(String, bool)>>,
    pub transcriptions: Mutex<Vec<Transcription>>,
    /// Whether each reported transcription reached durable history.
    pub saved: Mutex<Vec<bool>>,
    pub history_errors: Mutex<Vec<String>>,
    pub recordings_changed: Mutex<usize>,
    pub outcomes: Mutex<Vec<(u64, Outcome)>>,
}

impl CollectingEvents {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn failures_of(&self, kind: crate::pipeline::FailureKind) -> Vec<Failure> {
        self.failures.lock().unwrap().iter().filter(|failure| failure.kind == kind).cloned().collect()
    }
}

impl PipelineEvents for CollectingEvents {
    fn phase(&self, state: AppStateEnum) {
        self.state_changes.lock().unwrap().push(state);
    }
    fn busy(&self, state: AppStateEnum) {
        self.busy.lock().unwrap().push(state);
    }
    fn recording_started(&self, _generation: u64) {
        *self.recording_started.lock().unwrap() = true;
    }
    fn recording_stopped(&self, _generation: u64) {
        *self.recording_stopped.lock().unwrap() = true;
    }
    fn recording_cancelled(&self, _generation: u64) {
        *self.recording_cancelled.lock().unwrap() = true;
    }
    fn time_warning(&self, remaining_secs: u64) {
        self.time_warnings.lock().unwrap().push(remaining_secs);
    }
    fn failure(&self, failure: Failure) {
        self.failures.lock().unwrap().push(failure);
    }
    fn cleanup_status(&self, status: &LlmCleanupStatus) {
        self.cleanup_statuses.lock().unwrap().push(status.clone());
    }
    fn paste_complete(&self, id: &str, clipboard_only: bool) {
        self.paste_ids.lock().unwrap().push((id.to_string(), clipboard_only));
    }
    fn transcription_saved(&self, transcription: &Transcription, saved: &Result<Vec<String>, String>) {
        self.transcriptions.lock().unwrap().push(transcription.clone());
        self.saved.lock().unwrap().push(saved.is_ok());
    }
    fn history_error(&self, error: &str) {
        self.history_errors.lock().unwrap().push(error.to_string());
    }
    fn recordings_changed(&self) {
        *self.recordings_changed.lock().unwrap() += 1;
    }
    fn finished(&self, generation: u64, outcome: Outcome) -> Linger<'_> {
        self.outcomes.lock().unwrap().push((generation, outcome));
        Box::pin(std::future::ready(()))
    }
}
