//! The only dictation pipeline: start, stop, cancel and the work after a stop.
//!
//! Hotkeys, IPC and the tests below all drive these functions. Effects that
//! need Tauri (events, overlay, cues, the cancel shortcut, spawned tasks) go
//! through `PipelineEvents`; production uses `hotkeys::manager::TauriEvents`.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::asr::engine::AsrResult;
use crate::llm::cleanup::run_cleanup;
use crate::models::{AppStateEnum, LlmCleanupStatus, Transcription};
use crate::paste::backend::{write_text, ClipboardAction};
use crate::state::AppState;

/// Maximum recording duration before auto-stop (20 minutes).
/// Raised from 12 min in the 2026-04-11 reliability spec to support long
/// dictations. At 1.24 tok/word and 150 WPM, 20 min ≈ 3000 words ≈ 3720
/// output tokens — still well inside the sidecar's 16384-token ceiling.
pub const MAX_RECORDING_SECS: u64 = 20 * 60;
/// Seconds before max duration to show a warning (1 minute before).
pub const WARNING_BEFORE_LIMIT_SECS: u64 = 60;

/// Every transcription may take this long, plus half the recording's length.
const ASR_BASE_DEADLINE: Duration = Duration::from_secs(60);

pub(crate) const MODEL_LOADING_ERROR: &str = "Speech model is still loading; your recording was kept.";
pub(crate) const ASR_STUCK_ERROR: &str =
    "Speech recognition is not responding; your recording was kept. Quit and reopen SottoASR.";
const ASR_DEADLINE_ERROR: &str =
    "Transcription took too long; your recording was kept. Quit and reopen SottoASR if the next dictation also fails.";
const OWN_TARGET_COPIED: &str =
    "SottoASR was the active app, so the text was copied instead of pasted.";
const OWN_TARGET_NOT_PASTED: &str =
    "SottoASR was the active app, so the text was not pasted";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// The microphone could not start, or it stopped during the recording.
    Recording,
    Transcription,
    Paste,
    HistorySave,
}

/// A problem the user must see. The adapter shows it in the overlay.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub kind: FailureKind,
    pub generation: u64,
    pub error: String,
    /// The retained recording, when the user may need it.
    pub audio_path: Option<String>,
    /// Paste failures only: whether the text is on the clipboard.
    pub clipboard_available: Option<bool>,
}

impl Failure {
    fn new(kind: FailureKind, generation: u64, error: String) -> Self {
        Self { kind, generation, error, audio_path: None, clipboard_available: None }
    }

    fn with_audio(mut self, path: &Path) -> Self {
        self.audio_path = Some(path.to_string_lossy().into_owned());
        self
    }
}

/// How a dictation ended, so the adapter can keep or hide the overlay.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Delivery finished, or there was no text to deliver. The cleanup badge
    /// may stay visible briefly before the overlay hides.
    Delivered(LlmCleanupStatus),
    /// Too short to keep, or cancelled: nothing to show.
    Dismissed,
    /// A failure was reported; it stays visible until the user dismisses it.
    Failed,
    /// A newer recording or job took over; nothing was delivered or saved.
    Stale,
}

/// Overlay work the adapter finishes before the pipeline returns to Idle.
pub type Linger<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

/// Everything the pipeline reports. Production maps these calls to Tauri
/// events, the overlay and sound cues; tests collect them.
pub trait PipelineEvents: Send + Sync {
    /// The application state moved to `state`.
    fn phase(&self, state: AppStateEnum);
    /// A start was requested while a dictation is still being processed.
    fn busy(&self, state: AppStateEnum);
    fn recording_started(&self, generation: u64);
    fn recording_stopped(&self, generation: u64);
    fn recording_cancelled(&self, generation: u64);
    /// The recording stops automatically in `remaining_secs`.
    fn time_warning(&self, remaining_secs: u64);
    fn failure(&self, failure: Failure);
    fn cleanup_status(&self, status: &LlmCleanupStatus);
    fn paste_complete(&self, id: &str, clipboard_only: bool);
    fn transcription_saved(&self, transcription: &Transcription, saved: &Result<Vec<String>, String>);
    fn history_error(&self, error: &str);
    fn recordings_changed(&self);
    /// Called once per claimed recording, before the state returns to Idle.
    fn finished(&self, generation: u64, outcome: Outcome) -> Linger<'_>;
}

fn is_busy(state: &AppStateEnum) -> bool {
    matches!(state, AppStateEnum::Transcribing | AppStateEnum::CleaningUp | AppStateEnum::Pasting)
}

/// Open the microphone and enter Recording.
///
/// `level_callback` receives the input level about 30 times a second.
/// `error_callback` receives `(generation, error)` once when the device fails;
/// the caller stops that generation after the start events are published.
pub fn start(
    state: &AppState,
    events: &dyn PipelineEvents,
    level_callback: Box<dyn Fn(f32) + Send + 'static>,
    error_callback: Box<dyn Fn(u64, String) + Send + Sync + 'static>,
) -> Result<u64, String> {
    let current = state.get_state();
    if is_busy(&current) {
        log::info!("Start ignored while {current:?}");
        let error = format!("Cannot start recording: currently in {current:?} state");
        events.busy(current);
        return Err(error);
    }
    match crate::audio::capture::start_recording_capture_with_errors(state, level_callback, error_callback) {
        Ok(generation) => {
            events.phase(AppStateEnum::Recording);
            events.recording_started(generation);
            log::info!("Recording started — microphone active");
            Ok(generation)
        }
        Err(error) => {
            log::error!("Failed to start audio capture: {error}");
            if state.get_state() == AppStateEnum::Idle {
                let generation = state.recording_generation.load(Ordering::SeqCst);
                events.failure(Failure::new(FailureKind::Recording, generation, error.clone()));
                events.recordings_changed();
            }
            Err(error)
        }
    }
}

/// Wait out the recording limit, warning a minute before it. Returns true
/// when `generation` is still recording at the limit and must be stopped.
pub async fn wait_for_recording_limit(state: &AppState, events: &dyn PipelineEvents, generation: u64) -> bool {
    let still_recording = || {
        state.recording_generation.load(Ordering::SeqCst) == generation
            && state.get_state() == AppStateEnum::Recording
    };
    tokio::time::sleep(Duration::from_secs(MAX_RECORDING_SECS - WARNING_BEFORE_LIMIT_SECS)).await;
    if !still_recording() {
        return false;
    }
    log::info!("Recording approaching limit — {WARNING_BEFORE_LIMIT_SECS}s warning");
    events.time_warning(WARNING_BEFORE_LIMIT_SECS);
    tokio::time::sleep(Duration::from_secs(WARNING_BEFORE_LIMIT_SECS)).await;
    if !still_recording() {
        return false;
    }
    log::info!("Max recording duration ({MAX_RECORDING_SECS}s) reached — auto-stopping");
    true
}

/// A claimed recording whose capture is finalized, ready for `complete`.
pub struct StopJob {
    job_id: u64,
    generation: u64,
    audio_path: PathBuf,
    duration_ms: u64,
    capture_error: Option<String>,
    vocabulary: Vec<String>,
    cancelled: bool,
}

impl StopJob {
    /// A newer job or recording owns the result.
    fn is_stale(&self, state: &AppState) -> bool {
        !state.is_current_job(self.job_id)
            || state.recording_generation.load(Ordering::SeqCst) != self.generation
    }

    fn recording_id(&self) -> String {
        crate::audio::recovery::recording_id(&self.audio_path).expect("capture creates a recording UUID")
    }
}

/// Claim the end of the recording and finalize its WAV. `generation` limits
/// the claim to one recording, so a late key release or timer cannot stop a
/// newer one. Returns the job to pass to `complete`, or `None` when nothing
/// was claimed or nothing remains to transcribe.
pub async fn claim_and_finish(
    state: &AppState,
    events: &dyn PipelineEvents,
    generation: Option<u64>,
) -> Option<StopJob> {
    if !state.claim_recording_end(generation) {
        log::warn!("Cannot stop recording: currently in {:?} state", state.get_state());
        return None;
    }
    let generation = state.recording_generation.load(Ordering::SeqCst);
    events.recording_stopped(generation);
    events.phase(AppStateEnum::Transcribing);
    finish_capture(state, events, generation, false).await
}

/// Cancel the recording: nothing is pasted, but what was said is still
/// transcribed and saved to History as cancelled by `complete`.
pub async fn cancel(state: &AppState, events: &dyn PipelineEvents) -> Option<StopJob> {
    if !state.claim_recording_end(None) {
        log::warn!("Cannot cancel recording: currently in {:?} state", state.get_state());
        return None;
    }
    let generation = state.recording_generation.load(Ordering::SeqCst);
    events.recording_cancelled(generation);
    events.phase(AppStateEnum::Transcribing);
    finish_capture(state, events, generation, true).await
}

/// Quit Anyway, first step, run synchronously before the app exits: accept
/// the exit and claim a live recording, so no key release, timer or device
/// error can also take its writer. Returns whether a recording was claimed.
pub fn claim_for_exit(state: &AppState) -> bool {
    state.force_exit.store(true, Ordering::SeqCst);
    state.claim_recording_end(None)
}

/// Quit Anyway, second step after a successful `claim_for_exit`: finalize
/// the recording's WAV so the next launch offers it for recovery. Returns
/// the kept recording.
pub async fn finalize_for_exit(state: &AppState) -> Option<PathBuf> {
    match crate::audio::capture::finish_recording_capture(state).await {
        Ok(finished) => finished.audio_path,
        Err(error) => {
            log::error!("Could not finalize the recording before quitting: {error}");
            None
        }
    }
}

/// How long Quit Anyway waits for a stop that claimed the recording first.
const EXIT_FINALIZE_WAIT: Duration = Duration::from_secs(2);
const EXIT_FINALIZE_POLL: Duration = Duration::from_millis(20);

/// Quit Anyway when `claim_for_exit` found the recording already claimed by
/// a key release, timer or device error: wait, at most two seconds, until
/// that stop has joined the writer, so the WAV keeps its last second.
/// Returns whether the recording finished saving in time.
pub async fn wait_for_capture_finalized(state: &AppState) -> bool {
    let deadline = tokio::time::Instant::now() + EXIT_FINALIZE_WAIT;
    while state.is_recording.load(Ordering::SeqCst) {
        if tokio::time::Instant::now() >= deadline {
            log::warn!("Quitting before the recording finished saving");
            return false;
        }
        tokio::time::sleep(EXIT_FINALIZE_POLL).await;
    }
    true
}

async fn finish_capture(
    state: &AppState,
    events: &dyn PipelineEvents,
    generation: u64,
    cancelled: bool,
) -> Option<StopJob> {
    let finished = match crate::audio::capture::finish_recording_capture(state).await {
        Ok(finished) => finished,
        Err(error) => {
            log::error!("Could not finish the recording: {error}");
            // These are microphone and recording-storage errors, not ASR ones.
            events.failure(Failure::new(FailureKind::Recording, generation, error));
            settle(state, events, generation, Outcome::Failed).await;
            return None;
        }
    };
    let Some(audio_path) = finished.audio_path else {
        log::info!("Recording too short ({} samples)", finished.sample_count);
        let outcome = if cancelled {
            save_short_cancellation(events, generation, finished.duration_ms).await
        } else {
            Outcome::Dismissed
        };
        settle(state, events, generation, outcome).await;
        return None;
    };
    log::info!(
        "Captured {} samples at {} Hz ({:.3}s)",
        finished.sample_count, finished.sample_rate, finished.duration_ms as f64 / 1000.0
    );
    if let Some(error) = &finished.capture_error {
        let error = format!("{error}. Only the captured portion was saved; nothing will be pasted.");
        events.failure(Failure::new(FailureKind::Recording, generation, error).with_audio(&audio_path));
    }
    let vocabulary = state.recording_vocabulary.lock().unwrap_or_else(|error| error.into_inner()).clone();
    Some(StopJob {
        job_id: state.new_job(),
        generation,
        audio_path,
        duration_ms: finished.duration_ms,
        capture_error: finished.capture_error,
        vocabulary,
        cancelled,
    })
}

/// Transcribe, clean up, deliver and save a claimed recording, then return
/// to Idle. The adapter spawns this; tests await it.
pub async fn complete(state: &AppState, events: &dyn PipelineEvents, mut job: StopJob) {
    log::info!("Starting transcription...");
    let vocabulary = std::mem::take(&mut job.vocabulary);
    let path = job.audio_path.to_string_lossy().into_owned();
    let outcome = match transcribe_with_deadline(state, path, vocabulary, job.duration_ms).await {
        Ok(asr) if job.cancelled => save_cancelled(state, events, &job, asr).await,
        Ok(asr) => deliver(state, events, &job, asr).await,
        Err(_) if job.is_stale(state) => {
            log::info!("Job {} is stale, discarding its transcription error", job.job_id);
            Outcome::Stale
        }
        Err(error) => {
            log::error!("Transcription failed: {error}");
            events.failure(Failure::new(FailureKind::Transcription, job.generation, error).with_audio(&job.audio_path));
            Outcome::Failed
        }
    };
    settle(state, events, job.generation, outcome).await;
}

/// Let the adapter finish its overlay work, then return to Idle unless a
/// newer recording owns the state.
async fn settle(state: &AppState, events: &dyn PipelineEvents, generation: u64, outcome: Outcome) {
    events.finished(generation, outcome).await;
    if state.recording_generation.load(Ordering::SeqCst) == generation {
        state.set_state(AppStateEnum::Idle);
        events.phase(AppStateEnum::Idle);
        events.recordings_changed();
    }
}

/// Run ASR with a deadline of 60 s plus half the recording's duration.
///
/// Fails at once while the model is loading, or while an earlier call that
/// missed its deadline still holds the engine. A timeout cannot stop the
/// blocking call; it sets `asr_stuck`, which that call clears when it returns.
/// Never changes the application state.
pub async fn transcribe_with_deadline(
    state: &AppState,
    path: String,
    vocabulary: Vec<String>,
    duration_ms: u64,
) -> Result<AsrResult, String> {
    if state.asr_stuck.load(Ordering::SeqCst) {
        return Err(ASR_STUCK_ERROR.into());
    }
    if state.asr_initializing.load(Ordering::SeqCst) || !state.is_model_loaded.load(Ordering::SeqCst) {
        let init_error = state.asr_init_error.lock().unwrap_or_else(|error| error.into_inner()).clone();
        return Err(match init_error {
            // Loading finished and failed: say why instead of "still loading".
            Some(error) if !state.asr_initializing.load(Ordering::SeqCst) => {
                format!("{}. Your recording was kept.", error.trim_end_matches('.'))
            }
            _ => MODEL_LOADING_ERROR.into(),
        });
    }
    let deadline = ASR_BASE_DEADLINE + Duration::from_millis(duration_ms / 2);
    let returned = Arc::new(AtomicBool::new(false));
    let release = StuckRelease { returned: Arc::clone(&returned), stuck: Arc::clone(&state.asr_stuck) };
    let call = crate::asr::engine::with_engine(&state.asr_engine, move |engine| {
        let _release = release;
        engine.transcribe_file_with_vocabulary(&path, &vocabulary)
    });
    match tokio::time::timeout(deadline, call).await {
        Ok(result) => result,
        Err(_) => {
            state.asr_stuck.store(true, Ordering::SeqCst);
            // The call may have returned between the deadline and the store.
            if returned.load(Ordering::SeqCst) {
                state.asr_stuck.store(false, Ordering::SeqCst);
            }
            log::error!("Transcription missed its {}s deadline", deadline.as_secs());
            Err(ASR_DEADLINE_ERROR.into())
        }
    }
}

/// Dropped when the blocking transcription returns or unwinds, including
/// after its deadline has passed.
struct StuckRelease {
    returned: Arc<AtomicBool>,
    stuck: Arc<AtomicBool>,
}

impl Drop for StuckRelease {
    fn drop(&mut self) {
        self.returned.store(true, Ordering::SeqCst);
        self.stuck.store(false, Ordering::SeqCst);
    }
}

struct DeliveryPrefs {
    auto_paste: bool,
    restore_clipboard: bool,
    restore_focus: bool,
}

struct DeliveryError {
    error: String,
    clipboard_available: bool,
}

async fn deliver(state: &AppState, events: &dyn PipelineEvents, job: &StopJob, asr: AsrResult) -> Outcome {
    log::info!("Transcription complete (RTF: {:.1}x)", asr.rtfx);
    if job.is_stale(state) {
        log::info!("Job {} is stale, discarding transcription", job.job_id);
        return Outcome::Stale;
    }
    let raw_asr_text = asr.unboosted_text.unwrap_or_else(|| asr.text.clone());
    let mut final_text = asr.text;
    let (llm_enabled, prefs, dictionary, protected_terms) = {
        let settings = state.settings.lock().await;
        let protected_terms: Vec<String> = settings.vocabulary.iter().cloned()
            .chain(settings.dictionary.iter().map(|entry| entry.replacement.clone()))
            .collect();
        let prefs = DeliveryPrefs {
            auto_paste: settings.auto_paste,
            restore_clipboard: settings.restore_clipboard,
            restore_focus: settings.restore_focus_before_paste,
        };
        (settings.llm_cleanup_enabled, prefs, settings.dictionary.clone(), protected_terms)
    };
    let interrupted = job.capture_error.is_some();
    if !interrupted {
        final_text = crate::dictionary::apply(&final_text, &dictionary);
    }
    let cleanup_status = if llm_enabled && !interrupted {
        state.set_state(AppStateEnum::CleaningUp);
        events.phase(AppStateEnum::CleaningUp);
        let (cleaned, status) = run_cleanup(state, &final_text, &protected_terms).await;
        if matches!(status, LlmCleanupStatus::Applied { .. }) {
            final_text = cleaned;
        }
        status
    } else {
        LlmCleanupStatus::Disabled
    };
    if job.is_stale(state) {
        log::info!("Job {} is stale after cleanup, discarding", job.job_id);
        return Outcome::Stale;
    }
    *state.llm_last_status.lock().await = cleanup_status.clone();
    events.cleanup_status(&cleanup_status);

    let transcription = Transcription {
        id: job.recording_id(),
        word_count: final_text.split_whitespace().count(),
        raw_text: (final_text != raw_asr_text).then_some(raw_asr_text),
        text: final_text,
        duration_ms: job.duration_ms,
        created_at: chrono::Utc::now(),
        cancelled: false,
        capture_error: job.capture_error.clone(),
        cleanup_suggestion: None,
        llm_applied: matches!(cleanup_status, LlmCleanupStatus::Applied { .. }),
        llm_cleanup_status: cleanup_status.clone(),
        saved_at: None,
    };
    *state.last_transcription.lock().await = Some(transcription.clone());

    // Deliver first: the history write must not delay the text (L-02).
    let delivered = if !interrupted && !transcription.text.trim().is_empty() {
        Some(deliver_text(state, events, &transcription.id, &transcription.text, &prefs).await)
    } else {
        None
    };
    let saved = crate::commands::transcription::add_transcription(transcription.clone()).await;
    if let Err(error) = &saved {
        log::error!("History save failed: {error}");
        events.history_error(error);
    }
    crate::audio::wav::finish_after_history(&job.audio_path, interrupted, saved.is_ok());
    events.transcription_saved(&transcription, &saved);

    match (delivered, saved.err()) {
        (Some(Err(paste)), history_error) => {
            let error = match &history_error {
                Some(history) => with_history_error(&paste.error, history),
                None => paste.error,
            };
            let mut failure = Failure::new(FailureKind::Paste, job.generation, error);
            failure.clipboard_available = Some(paste.clipboard_available);
            if history_error.is_some() {
                failure = failure.with_audio(&job.audio_path);
            }
            events.failure(failure);
            Outcome::Failed
        }
        (_, Some(history)) => {
            let failure = match &job.capture_error {
                Some(interruption) => Failure::new(FailureKind::Recording, job.generation, format!(
                    "{interruption}. History also could not be saved: {history}. Audio was retained; nothing was pasted."
                )),
                None => Failure::new(FailureKind::HistorySave, job.generation, history),
            };
            events.failure(failure.with_audio(&job.audio_path));
            Outcome::Failed
        }
        _ if interrupted => Outcome::Failed,
        _ => Outcome::Delivered(cleanup_status),
    }
}

fn with_history_error(error: &str, history: &str) -> String {
    format!("{}. History also could not be saved: {history}", error.trim_end_matches('.'))
}

/// Paste at the target app, or copy when auto-paste is off. A failed paste
/// falls back to the clipboard.
async fn deliver_text(
    state: &AppState,
    events: &dyn PipelineEvents,
    id: &str,
    text: &str,
    prefs: &DeliveryPrefs,
) -> Result<(), DeliveryError> {
    if !prefs.auto_paste {
        return match write_text(&state.paste_backend, text, ClipboardAction::Copy).await {
            Ok(()) => {
                log::info!("Text copied to clipboard (auto_paste disabled)");
                events.paste_complete(id, true);
                Ok(())
            }
            Err(error) => {
                log::error!("Clipboard copy failed: {error}");
                Err(DeliveryError { error, clipboard_available: false })
            }
        };
    }
    let Some(target_pid) = paste_target(state, prefs.restore_focus) else {
        // Cmd+V into our own overlay or Settings would lose the dictation.
        log::warn!("Paste target is SottoASR itself; copying instead");
        return Err(match write_text(&state.paste_backend, text, ClipboardAction::Copy).await {
            Ok(()) => DeliveryError { error: OWN_TARGET_COPIED.into(), clipboard_available: true },
            Err(copy_error) => DeliveryError {
                error: format!("{OWN_TARGET_NOT_PASTED}. Clipboard copy also failed: {copy_error}"),
                clipboard_available: false,
            },
        });
    };
    let action = ClipboardAction::Paste { target_pid, restore: prefs.restore_clipboard };
    match write_text(&state.paste_backend, text, action).await {
        Ok(()) => {
            log::info!("Text pasted at cursor");
            events.paste_complete(id, false);
            Ok(())
        }
        Err(error) => {
            log::error!("Paste failed: {error}");
            match write_text(&state.paste_backend, text, ClipboardAction::Copy).await {
                Ok(()) => {
                    log::info!("Text copied to clipboard as fallback");
                    Err(DeliveryError { error, clipboard_available: true })
                }
                Err(copy_error) => Err(DeliveryError {
                    error: format!("{error}. Clipboard copy also failed: {copy_error}"),
                    clipboard_available: false,
                }),
            }
        }
    }
}

/// The app that receives the paste: with `restore_focus`, the one frontmost
/// at start unless the user deliberately switched to another app; otherwise
/// 0, which posts to whatever is frontmost now. `None` when the receiver
/// would be SottoASR itself, whichever way it was chosen.
fn paste_target(state: &AppState, restore_focus: bool) -> Option<i32> {
    let own_pid = std::process::id() as i32;
    let current_pid = state.paste_backend.get_frontmost_pid();
    let target_pid = if !restore_focus {
        0
    } else {
        let start_pid = state.target_pid.load(Ordering::SeqCst);
        if current_pid == start_pid || current_pid == own_pid || current_pid == 0 {
            start_pid
        } else {
            log::info!("User switched apps during recording: {start_pid} → {current_pid}, pasting at current");
            current_pid
        }
    };
    let receiver = if target_pid == 0 { current_pid } else { target_pid };
    (receiver != own_pid).then_some(target_pid)
}

async fn save_cancelled(state: &AppState, events: &dyn PipelineEvents, job: &StopJob, asr: AsrResult) -> Outcome {
    if job.is_stale(state) {
        log::info!("Job {} is stale, discarding cancelled transcription", job.job_id);
        return Outcome::Stale;
    }
    let transcription = Transcription {
        id: job.recording_id(),
        word_count: asr.text.split_whitespace().count(),
        text: asr.text,
        duration_ms: job.duration_ms,
        created_at: chrono::Utc::now(),
        cancelled: true,
        capture_error: job.capture_error.clone(),
        raw_text: asr.unboosted_text,
        cleanup_suggestion: None,
        llm_applied: false,
        llm_cleanup_status: LlmCleanupStatus::Idle,
        saved_at: None,
    };
    let saved = crate::commands::transcription::add_transcription(transcription.clone()).await;
    let interrupted = job.capture_error.is_some();
    crate::audio::wav::finish_after_history(&job.audio_path, interrupted, saved.is_ok());
    let failed = match &saved {
        Ok(_) => {
            log::info!("Cancelled transcription saved");
            interrupted
        }
        Err(error) => {
            log::error!("History save failed: {error}");
            events.history_error(error);
            events.failure(Failure::new(FailureKind::HistorySave, job.generation, error.clone()).with_audio(&job.audio_path));
            true
        }
    };
    events.transcription_saved(&transcription, &saved);
    if failed { Outcome::Failed } else { Outcome::Dismissed }
}

/// A cancelled recording too short to transcribe still leaves a History entry.
async fn save_short_cancellation(events: &dyn PipelineEvents, generation: u64, duration_ms: u64) -> Outcome {
    let transcription = Transcription {
        id: uuid::Uuid::new_v4().to_string(),
        text: String::new(),
        duration_ms,
        created_at: chrono::Utc::now(),
        word_count: 0,
        cancelled: true,
        capture_error: None,
        raw_text: None,
        cleanup_suggestion: None,
        llm_applied: false,
        llm_cleanup_status: LlmCleanupStatus::Idle,
        saved_at: None,
    };
    let saved = crate::commands::transcription::add_transcription(transcription.clone()).await;
    events.transcription_saved(&transcription, &saved);
    match &saved {
        Ok(_) => Outcome::Dismissed,
        Err(error) => {
            log::error!("History save failed: {error}");
            events.history_error(error);
            events.failure(Failure::new(FailureKind::HistorySave, generation, error.clone()));
            Outcome::Failed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use crate::models::Settings;
    use std::sync::Mutex;

    fn begin(state: &AppState, events: &CollectingEvents) -> Result<u64, String> {
        start(state, events, Box::new(|_| {}), Box::new(|_, _| {}))
    }

    /// Stop and run the whole pipeline, as the adapter's spawned task does.
    /// Returns whether a job was claimed.
    async fn stop(state: &AppState, events: &CollectingEvents) -> bool {
        let Some(job) = claim_and_finish(state, events, None).await else { return false };
        complete(state, events, job).await;
        true
    }

    async fn cancel_and_complete(state: &AppState, events: &CollectingEvents) -> bool {
        let Some(job) = cancel(state, events).await else { return false };
        complete(state, events, job).await;
        true
    }

    fn last_outcome(events: &CollectingEvents) -> Option<Outcome> {
        events.outcomes.lock().unwrap().last().map(|(_, outcome)| outcome.clone())
    }

    #[test]
    fn busy_exit_preserves_capture_and_accepted_exit_blocks_new_recordings() {
        let state = AppState::new_with_backends(
            Box::new(MockAudioCapture::new(vec![0.1; 4800], 48_000)),
            Box::new(MockAsrEngine::with_text("test")), None,
            Box::new(MockPasteBackend::new()), Settings::default(),
        );
        let events = CollectingEvents::new();
        begin(&state, &events).unwrap();
        assert!(state.begin_exit().is_err());
        assert!(!state.is_exiting.load(Ordering::SeqCst));
        assert_eq!(state.get_state(), AppStateEnum::Recording);
        state.audio_capture.lock().unwrap().stop();
        state.set_state(AppStateEnum::Idle);
        state.begin_exit().unwrap();
        assert!(begin(&state, &events).is_err());
        assert_eq!(state.get_state(), AppStateEnum::Idle);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn quit_anyway_claims_the_recording_and_finalizes_its_audio() {
        let state = AppState::new_with_backends(
            Box::new(MockAudioCapture::new(vec![0.25; 8_000], 16_000)),
            Box::new(MockAsrEngine::with_text("test")), None,
            Box::new(MockPasteBackend::new()), Settings::default(),
        );
        let events = CollectingEvents::new();
        let generation = begin(&state, &events).unwrap();
        assert!(claim_for_exit(&state));
        // The exit is accepted while the dictation is still in flight.
        state.begin_exit().unwrap();
        assert!(state.is_exiting.load(Ordering::SeqCst));
        let audio = finalize_for_exit(&state).await.expect("the recording is kept for recovery");
        // A key release, timer or device error arriving now cannot take the writer.
        assert!(claim_and_finish(&state, &events, Some(generation)).await.is_none());
        assert!(!*events.recording_stopped.lock().unwrap());
        let mut reader = hound::WavReader::open(&audio).unwrap();
        let samples: Vec<f32> = reader.samples::<f32>().map(Result::unwrap).collect();
        assert!(samples.len() >= 8_000);
        assert!(samples[..8_000].iter().all(|sample| *sample == 0.25));
        drop(reader);
        std::fs::remove_file(&audio).unwrap(); // This test's synthetic recovery file only.
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn quit_anyway_waits_briefly_for_a_racing_stop_to_save_the_recording() {
        let state = test_state(MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("unused"), None, Box::new(MockPasteBackend::new()));
        // Nothing is being saved: no wait.
        let started = tokio::time::Instant::now();
        assert!(wait_for_capture_finalized(&state).await);
        assert_eq!(started.elapsed(), Duration::ZERO);

        // A stop that claimed the recording first joins its writer after 300 ms.
        state.is_recording.store(true, Ordering::SeqCst);
        let started = tokio::time::Instant::now();
        let racing_stop = async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            state.is_recording.store(false, Ordering::SeqCst);
        };
        let (saved, ()) = tokio::join!(wait_for_capture_finalized(&state), racing_stop);
        assert!(saved);
        assert!(started.elapsed() < Duration::from_millis(300) + EXIT_FINALIZE_POLL * 2);

        // A stop that never finishes cannot hold the exit past the bound.
        state.is_recording.store(true, Ordering::SeqCst);
        let started = tokio::time::Instant::now();
        assert!(!wait_for_capture_finalized(&state).await);
        assert!(started.elapsed() >= EXIT_FINALIZE_WAIT);
        assert!(started.elapsed() < EXIT_FINALIZE_WAIT + EXIT_FINALIZE_POLL * 2);
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    #[serial_test::serial]
    async fn recording_limit_warns_a_minute_early_then_stops_that_recording() {
        let state = test_state(MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("long"), None, Box::new(MockPasteBackend::new()));
        let events = CollectingEvents::new();
        let generation = begin(&state, &events).unwrap();
        let started = tokio::time::Instant::now();
        assert!(wait_for_recording_limit(&state, &events, generation).await);
        assert!(started.elapsed() >= Duration::from_secs(MAX_RECORDING_SECS));
        assert_eq!(*events.time_warnings.lock().unwrap(), vec![WARNING_BEFORE_LIMIT_SECS]);
        // The adapter then stops exactly this recording.
        let job = claim_and_finish(&state, &events, Some(generation)).await.unwrap();
        std::fs::remove_file(&job.audio_path).unwrap();
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    #[serial_test::serial]
    async fn recording_limit_ignores_a_superseded_recording() {
        let state = test_state(MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("long"), None, Box::new(MockPasteBackend::new()));
        let events = CollectingEvents::new();
        let generation = begin(&state, &events).unwrap();
        // A newer recording took over before the warning was due.
        state.recording_generation.fetch_add(1, Ordering::SeqCst);
        assert!(!wait_for_recording_limit(&state, &events, generation).await);
        assert!(events.time_warnings.lock().unwrap().is_empty());
        let job = claim_and_finish(&state, &events, None).await.unwrap();
        std::fs::remove_file(&job.audio_path).unwrap();
    }

    #[tokio::test]
    async fn a_silent_microphone_is_reported_as_a_recording_failure() {
        let state = test_state(MockAudioCapture::new(vec![0.0; 8_000], 16_000),
            MockAsrEngine::with_text("unused"), None, Box::new(MockPasteBackend::new()));
        let events = CollectingEvents::new();
        begin(&state, &events).unwrap();
        assert!(claim_and_finish(&state, &events, None).await.is_none());
        let failures = events.failures.lock().unwrap().clone();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].kind, FailureKind::Recording);
        assert!(failures[0].error.contains("silence"), "{}", failures[0].error);
        assert!(events.transcriptions.lock().unwrap().is_empty());
        assert_eq!(last_outcome(&events), Some(Outcome::Failed));
        assert_eq!(state.get_state(), AppStateEnum::Idle);
    }

    #[test]
    fn shortcut_transaction_excludes_capture_until_save_or_rollback_finishes() {
        let state = AppState::new_with_backends(
            Box::new(MockAudioCapture::new(vec![0.1; 4800], 48_000)),
            Box::new(MockAsrEngine::with_text("test")), None,
            Box::new(MockPasteBackend::new()), Settings::default(),
        );
        let events = CollectingEvents::new();
        let transaction = state.begin_shortcut_update().unwrap();
        assert!(begin(&state, &events).is_err());
        assert_eq!(state.get_state(), AppStateEnum::Idle);
        assert_eq!(state.recording_generation.load(Ordering::SeqCst), 0);
        drop(transaction); // Also runs on a save/rollback error or cancelled task.
        begin(&state, &events).unwrap();
        assert!(state.begin_shortcut_update().is_err());
        state.audio_capture.lock().unwrap().stop();
    }

    #[test]
    fn start_while_processing_reports_busy_and_leaves_the_dictation_alone() {
        let state = test_state(MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("hello"), None, Box::new(MockPasteBackend::new()));
        let events = CollectingEvents::new();
        for busy in [AppStateEnum::Transcribing, AppStateEnum::CleaningUp, AppStateEnum::Pasting] {
            state.set_state(busy.clone());
            assert!(begin(&state, &events).is_err());
            assert_eq!(state.get_state(), busy);
        }
        assert_eq!(*events.busy.lock().unwrap(),
            vec![AppStateEnum::Transcribing, AppStateEnum::CleaningUp, AppStateEnum::Pasting]);
        assert_eq!(state.recording_generation.load(Ordering::SeqCst), 0);
        assert!(events.failures.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn device_failure_during_stop_retains_tail_and_never_pastes_partial_text() {
        use crate::audio::capture::AudioCaptureBackend;
        use std::sync::mpsc::SyncSender;
        struct InterruptedCapture {
            sender: Option<SyncSender<Vec<f32>>>,
            error: Option<Box<dyn Fn(String) + Send>>,
        }
        impl AudioCaptureBackend for InterruptedCapture {
            fn start(&mut self, sender: SyncSender<Vec<f32>>, _: Arc<AtomicBool>, _: Box<dyn Fn(f32) + Send>, error: Box<dyn Fn(String) + Send>) -> Result<(), String> {
                sender.send(vec![0.25; 192_000]).unwrap();
                self.sender = Some(sender);
                self.error = Some(error);
                Ok(())
            }
            fn stop(&mut self) {
                if let Some(sender) = self.sender.take() { sender.send(vec![0.75, -0.5]).unwrap(); }
                if let Some(error) = self.error.take() { error("Microphone disconnected".into()); }
            }
            fn sample_rate(&self) -> u32 { 192_000 }
        }
        for cancelled in [false, true] {
            let paste = Arc::new(MockPasteBackend::new());
            let state = AppState::new_with_backends(
                Box::new(InterruptedCapture { sender: None, error: None }),
                Box::new(MockAsrEngine::with_text("um, this is only the captured portion")),
                Some(Box::new(MockLlmBackend::failing("must not clean interrupted capture"))),
                Box::new(SharedMockPaste(paste.clone())),
                Settings { llm_cleanup_enabled: true, ..Default::default() },
            );
            let events = CollectingEvents::new();
            begin(&state, &events).unwrap();
            if cancelled { assert!(cancel_and_complete(&state, &events).await); }
            else { assert!(stop(&state, &events).await); }
            assert_eq!(state.get_state(), AppStateEnum::Idle);
            assert!(paste.pasted_texts.lock().unwrap().is_empty());
            assert!(paste.copied_texts.lock().unwrap().is_empty());
            let transcription = events.transcriptions.lock().unwrap()[0].clone();
            assert_eq!(transcription.capture_error.as_deref(), Some("Microphone disconnected"));
            assert_eq!(transcription.text, "um, this is only the captured portion");
            assert!(!transcription.llm_applied);
            assert_eq!(last_outcome(&events), Some(Outcome::Failed));
            let failure = events.failures_of(FailureKind::Recording).remove(0);
            assert!(failure.error.contains("Microphone disconnected"));
            let path = failure.audio_path.expect("interrupted audio is retained");
            let mut reader = hound::WavReader::open(&path).unwrap();
            assert_eq!(reader.spec().sample_rate, 192_000);
            let samples: Vec<f32> = reader.samples::<f32>().map(Result::unwrap).collect();
            assert_eq!(&samples[192_000..192_002], &[0.75, -0.5]);
            drop(reader);
            std::fs::remove_file(&path).unwrap(); // This test's synthetic recovery file only.
        }
    }

    /// Helper: create an AppState with standard mock backends.
    fn test_state(
        audio: MockAudioCapture,
        asr: MockAsrEngine,
        llm: Option<MockLlmBackend>,
        paste: Box<dyn crate::paste::PasteBackend>,
    ) -> AppState {
        let llm_enabled = llm.is_some();
        let settings = Settings {
            auto_paste: true,
            restore_clipboard: true,
            restore_focus_before_paste: true,
            llm_cleanup_enabled: llm_enabled,
            ..Default::default()
        };

        AppState::new_with_backends(
            Box::new(audio),
            Box::new(asr),
            llm.map(|l| Box::new(l) as Box<dyn crate::llm::engine::LlmBackend>),
            paste,
            settings,
        )
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn stop_drains_final_callback_and_uses_captured_duration() {
        use crate::asr::engine::AsrEngine;
        use crate::audio::capture::AudioCaptureBackend;
        use std::sync::mpsc::SyncSender;

        struct FinalCallbackCapture { sender: Option<SyncSender<Vec<f32>>> }
        impl AudioCaptureBackend for FinalCallbackCapture {
            fn start(&mut self, sender: SyncSender<Vec<f32>>, _: Arc<AtomicBool>, _: Box<dyn Fn(f32) + Send>, _: Box<dyn Fn(String) + Send>) -> Result<(), String> {
                sender.send(vec![0.25; 16_000 * 60]).unwrap();
                self.sender = Some(sender);
                Ok(())
            }
            fn stop(&mut self) {
                if let Some(sender) = self.sender.take() {
                    sender.send(vec![0.5; 16_000 * 5]).unwrap();
                    sender.send(vec![0.71, -0.62, 0.53, -0.44]).unwrap();
                }
            }
            fn sample_rate(&self) -> u32 { 16_000 }
        }
        struct InspectingAsr { observed: Arc<Mutex<Vec<f32>>> }
        impl AsrEngine for InspectingAsr {
            fn init(&mut self) -> Result<(), String> { Ok(()) }
            fn is_ready(&self) -> bool { true }
            fn is_model_available(&self) -> bool { true }
            fn backend_name(&self) -> &'static str { "WAV inspection" }
            fn transcribe_samples(&mut self, _: &[f32], _: u32) -> Result<AsrResult, String> { unreachable!() }
            fn transcribe_file(&mut self, path: &str) -> Result<AsrResult, String> {
                let mut reader = hound::WavReader::open(path).unwrap();
                assert_eq!(reader.spec().sample_rate, 16_000);
                *self.observed.lock().unwrap() = reader.samples().collect::<Result<_, _>>().unwrap();
                Ok(AsrResult { unboosted_text: None, text: "Complete recording.".into(), duration_secs: 0.0,
                    processing_time_secs: 0.1, rtfx: 0.0 })
            }
        }
        let observed = Arc::new(Mutex::new(Vec::new()));
        let state = AppState::new_with_backends(
            Box::new(FinalCallbackCapture { sender: None }),
            Box::new(InspectingAsr { observed: observed.clone() }), None,
            Box::new(MockPasteBackend::new()), Settings::default());
        let events = CollectingEvents::new();
        begin(&state, &events).unwrap();
        assert!(stop(&state, &events).await);
        {
        let samples = observed.lock().unwrap();
        let captured = 16_000 * 65 + 4;
        assert_eq!(samples.len(), captured + 12_000);
        assert_eq!(&samples[captured - 4..captured], &[0.71, -0.62, 0.53, -0.44]);
        assert!(samples[16_000 * 60..captured - 4].iter().all(|sample| *sample == 0.5));
        assert!(samples[captured..].iter().all(|sample| *sample == 0.0));
        }
        assert_eq!(state.last_transcription.lock().await.as_ref().unwrap().duration_ms, 65_000);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn vocabulary_snapshot_and_original_survive_stop_cancel_and_replacements() {
        let raw = "Please use Quen for these notes.";
        for cancelled in [false, true] {
            let state = test_state(
                MockAudioCapture::sine_wave(),
                MockAsrEngine::with_vocabulary(raw, &["Qwen"], "Please use Qwen for these notes."),
                None,
                Box::new(MockPasteBackend::new()),
            );
            state.vocabulary_terms.lock().unwrap().push("Qwen".into());
            state.settings.lock().await.dictionary = vec![crate::models::DictionaryEntry {
                heard: "notes".into(), replacement: "reports".into(),
            }];
            let events = CollectingEvents::new();
            begin(&state, &events).unwrap();
            // A later settings edit cannot alter this recording's snapshot.
            *state.vocabulary_terms.lock().unwrap() = vec!["Claude".into()];
            let transcription = if cancelled {
                assert!(cancel_and_complete(&state, &events).await);
                events.transcriptions.lock().unwrap()[0].clone()
            } else {
                assert!(stop(&state, &events).await);
                state.last_transcription.lock().await.clone().unwrap()
            };
            assert_eq!(transcription.cancelled, cancelled);
            assert_eq!(transcription.raw_text.as_deref(), Some(raw));
            assert_eq!(transcription.text, if cancelled {
                "Please use Qwen for these notes."
            } else {
                "Please use Qwen for these reports."
            });
        }
    }

    #[test]
    fn stale_release_cannot_claim_a_new_recording() {
        use crate::audio::capture::start_recording_capture;
        let state = test_state(MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("fixture"), None, Box::new(MockPasteBackend::new()));
        let first = start_recording_capture(&state, Box::new(|_| {})).unwrap();
        assert!(state.claim_recording_end(Some(first)));
        state.audio_capture.lock().unwrap().stop();
        state.set_state(AppStateEnum::Idle);
        let second = start_recording_capture(&state, Box::new(|_| {})).unwrap();
        assert!(second > first);
        assert!(!state.claim_recording_end(Some(first)));
        assert_eq!(state.get_state(), AppStateEnum::Recording);
        assert!(state.is_recording.load(Ordering::SeqCst));
        assert!(state.claim_recording_end(Some(second)));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn concurrent_stop_and_cancel_have_one_audio_owner() {
        for cancel_first in [false, true] {
            let state = test_state(MockAudioCapture::sine_wave(),
                MockAsrEngine::with_text("All captured speech."), None, Box::new(MockPasteBackend::new()));
            let events = CollectingEvents::new();
            begin(&state, &events).unwrap();
            let (stopped, cancelled) = if cancel_first {
                let (cancelled, stopped) = tokio::join!(
                    cancel_and_complete(&state, &events), stop(&state, &events));
                (stopped, cancelled)
            } else {
                tokio::join!(stop(&state, &events), cancel_and_complete(&state, &events))
            };
            assert_eq!(cancelled, cancel_first);
            assert_eq!(stopped, !cancel_first);
            assert_eq!(events.transcriptions.lock().unwrap().len(), 1);
            assert_eq!(state.get_state(), AppStateEnum::Idle);
        }
    }

    #[test]
    fn shared_start_acquires_the_microphone_once_and_propagates_failure() {
        use crate::audio::capture::{start_recording_capture, AudioCaptureBackend};
        use std::sync::{atomic::AtomicUsize, mpsc::SyncSender};
        struct CountingCapture { starts: Arc<AtomicUsize>, fail: bool }
        impl AudioCaptureBackend for CountingCapture {
            fn start(&mut self, _: SyncSender<Vec<f32>>, _: Arc<AtomicBool>, _: Box<dyn Fn(f32) + Send + 'static>, _: Box<dyn Fn(String) + Send>) -> Result<(), String> {
                self.starts.fetch_add(1, Ordering::SeqCst);
                if self.fail { Err("microphone unavailable".into()) } else { Ok(()) }
            }
            fn stop(&mut self) {}
            fn sample_rate(&self) -> u32 { 16000 }
        }
        for fail in [false, true] {
            let starts = Arc::new(AtomicUsize::new(0));
            let state = AppState::new_with_backends(
                Box::new(CountingCapture { starts: Arc::clone(&starts), fail }),
                Box::new(MockAsrEngine::with_text("fixture")), None,
                Box::new(MockPasteBackend::new()), Settings::default(),
            );
            let result = start_recording_capture(&state, Box::new(|_| {}));
            assert_eq!(result.is_err(), fail);
            assert_eq!(starts.load(Ordering::SeqCst), 1);
            assert_eq!(state.get_state(), if fail { AppStateEnum::Idle } else { AppStateEnum::Recording });
            assert_eq!(state.is_recording.load(Ordering::SeqCst), !fail);
            if !fail {
                assert!(start_recording_capture(&state, Box::new(|_| {})).is_err());
                assert_eq!(starts.load(Ordering::SeqCst), 1);
                assert!(state.is_recording.load(Ordering::SeqCst));
            }
        }
    }

    // --- Test 1: Happy path with LLM cleanup ---
    #[tokio::test]
    #[serial_test::serial]
    async fn test_full_pipeline_happy_path() {
        let mock_paste = Arc::new(MockPasteBackend::new());
        let state = test_state(
            MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("Hello, um, world. This is a test sentence."),
            Some(MockLlmBackend::proposal("Hello, world. This is a test sentence.")),
            Box::new(SharedMockPaste(mock_paste.clone())),
        );
        let events = CollectingEvents::new();

        // Start recording
        begin(&state, &events).unwrap();
        assert_eq!(state.get_state(), AppStateEnum::Recording);

        // Stop recording -- triggers transcription + LLM + paste
        assert!(stop(&state, &events).await);

        // Assert final state
        assert_eq!(state.get_state(), AppStateEnum::Idle);

        // Assert transcription was saved
        let last = state.last_transcription.lock().await;
        let t = last.as_ref().expect("transcription should be saved");
        assert_eq!(t.text, "Hello, world. This is a test sentence.");
        assert!(t.cleanup_suggestion.is_none());
        assert_eq!(t.raw_text.as_deref(), Some("Hello, um, world. This is a test sentence."));
        assert!(t.llm_applied);
        assert!(!t.cancelled);
        assert!(matches!(t.llm_cleanup_status, LlmCleanupStatus::Applied { .. }));
        // last_status should be cached on AppState and published for the badge
        assert!(matches!(*state.llm_last_status.lock().await, LlmCleanupStatus::Applied { .. }));
        assert!(matches!(events.cleanup_statuses.lock().unwrap().as_slice(), [LlmCleanupStatus::Applied { .. }]));
        assert!(matches!(last_outcome(&events), Some(Outcome::Delivered(LlmCleanupStatus::Applied { .. }))));

        // The validated cleanup is the actual delivered text.
        let pasted = mock_paste.last_pasted().expect("should have pasted");
        assert_eq!(pasted.text, "Hello, world. This is a test sentence.");
        assert!(pasted.restore_clipboard);
        assert!(!events.paste_ids.lock().unwrap().is_empty());
        assert_eq!(*events.saved.lock().unwrap(), vec![true]);

        // Assert state transitions
        let states = events.state_changes.lock().unwrap();
        assert!(states.contains(&AppStateEnum::Recording));
        assert!(states.contains(&AppStateEnum::Transcribing));
        assert!(states.contains(&AppStateEnum::CleaningUp));
        assert!(states.contains(&AppStateEnum::Idle));
    }

    // --- Test 2: Pipeline without LLM ---
    #[tokio::test]
    #[serial_test::serial]
    async fn test_pipeline_without_llm() {
        let mock_paste = Arc::new(MockPasteBackend::new());
        let state = test_state(
            MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("hello world"),
            None, // No LLM
            Box::new(SharedMockPaste(mock_paste.clone())),
        );
        let events = CollectingEvents::new();

        begin(&state, &events).unwrap();
        assert!(stop(&state, &events).await);

        // Assert raw ASR text was pasted (no LLM cleanup)
        let pasted = mock_paste.last_pasted().expect("should have pasted");
        assert_eq!(pasted.text, "hello world");

        // Assert transcription has no raw_text (LLM was not used)
        let last = state.last_transcription.lock().await;
        let t = last.as_ref().unwrap();
        assert!(!t.llm_applied);
        assert!(t.raw_text.is_none());
        // With llm_cleanup_enabled=false, status should be Disabled
        assert_eq!(t.llm_cleanup_status, LlmCleanupStatus::Disabled);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn dictionary_and_cleanup_preserve_original_provenance() {
        let raw = "Please use um, Quen for the final report.";
        let expected = "Please use um, Qwen for the final report.";
        for (llm, cleaned) in [
            (None, None),
            (Some(MockLlmBackend::proposal("Invented text")), None),
            (Some(MockLlmBackend::proposal("Please use Qwen for the final report.")), Some("Please use Qwen for the final report.")),
        ] {
            let mock_paste = Arc::new(MockPasteBackend::new());
            let enabled = llm.is_some();
            let state = test_state(
                MockAudioCapture::sine_wave(),
                MockAsrEngine::with_text(raw),
                llm,
                Box::new(SharedMockPaste(mock_paste.clone())),
            );
            state.settings.lock().await.dictionary = vec![crate::models::DictionaryEntry {
                heard: "Quen".into(),
                replacement: "Qwen".into(),
            }];
            let events = CollectingEvents::new();
            begin(&state, &events).unwrap();
            assert!(stop(&state, &events).await);

            let last = state.last_transcription.lock().await;
            let transcription = last.as_ref().unwrap();
            let expected = cleaned.unwrap_or(expected);
            assert_eq!(transcription.text, expected);
            assert_eq!(transcription.raw_text.as_deref(), Some(raw));
            assert_eq!(transcription.word_count, expected.split_whitespace().count());
            assert_eq!(transcription.llm_applied, cleaned.is_some());
            assert!(transcription.cleanup_suggestion.is_none());
            assert_eq!(mock_paste.last_pasted().unwrap().text, expected);
            if !enabled {
                assert_eq!(transcription.llm_cleanup_status, LlmCleanupStatus::Disabled);
            } else if cleaned.is_none() {
                assert!(matches!(transcription.llm_cleanup_status, LlmCleanupStatus::Rejected { .. }));
            }
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn protected_word_deletions_cannot_change_delivery() {
        // Explicit literal mentions, quotes, names and required tail words are protected.
        // Short foreign phrases remain an independently measured model limitation.
        for (raw, harmful) in [
            ("The token um, is required in this exact output.", "The token is required in this exact output."),
            ("The word um is required here.", "The word is required here."),
            ("Dr. Um um confirmed the appointment.", "Dr. confirmed the appointment."),
            ("Please um retain the final instruction and number 859.", "Please retain the final instruction."),
        ] {
            for auto_paste in [false, true] {
                let paste = Arc::new(MockPasteBackend::new());
                let state = test_state(MockAudioCapture::sine_wave(),
                    MockAsrEngine::with_text(raw), Some(MockLlmBackend::proposal(harmful)),
                    Box::new(SharedMockPaste(paste.clone())));
                state.settings.lock().await.auto_paste = auto_paste;
                let events = CollectingEvents::new();
                begin(&state, &events).unwrap();
                assert!(stop(&state, &events).await);
                let last = state.last_transcription.lock().await;
                let transcription = last.as_ref().unwrap();
                let expected = if raw.starts_with("Please um") {
                    "Please retain the final instruction and number 859."
                } else { raw };
                assert_eq!(transcription.text, expected); // Also the source for Copy Last.
                assert_eq!(transcription.llm_applied, expected != raw);
                assert!(transcription.cleanup_suggestion.is_none());
                if expected == raw {
                    // The harmful edit never lands. Which raw-preserving
                    // status surfaces depends on the work mask: a fully
                    // protected `raw` is provably no-work and is NEVER
                    // dispatched (honest `NoChanges` — the model was not
                    // consulted); a partially-active window that dispatches
                    // and is refused surfaces `Rejected`. Delivery
                    // invariance above is the contract.
                    assert!(
                        matches!(
                            transcription.llm_cleanup_status,
                            LlmCleanupStatus::Rejected { .. } | LlmCleanupStatus::NoChanges
                        ),
                        "protected harmful proposal for {raw:?} must stay raw-preserving, got {:?}",
                        transcription.llm_cleanup_status
                    );
                } else {
                    assert!(matches!(transcription.llm_cleanup_status, LlmCleanupStatus::Applied { .. }));
                    assert_eq!(transcription.raw_text.as_deref(), Some(raw));
                }
                assert_eq!(events.transcriptions.lock().unwrap()[0].text, expected);
                if auto_paste { assert_eq!(paste.last_pasted().unwrap().text, expected); }
                else { assert_eq!(paste.copied_texts.lock().unwrap().last().map(String::as_str), Some(expected)); }
            }
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn all_filler_cleanup_saves_original_without_touching_clipboard() {
        for auto_paste in [false, true] {
            let paste = Arc::new(MockPasteBackend::new());
            let state = test_state(MockAudioCapture::sine_wave(),
                MockAsrEngine::with_text("Um, uh."), Some(MockLlmBackend::proposal("")),
                Box::new(SharedMockPaste(paste.clone())));
            state.settings.lock().await.auto_paste = auto_paste;
            let events = CollectingEvents::new();
            begin(&state, &events).unwrap();
            assert!(stop(&state, &events).await);
            let last = state.last_transcription.lock().await;
            let item = last.as_ref().unwrap();
            assert_eq!(item.text, "");
            assert_eq!(item.raw_text.as_deref(), Some("Um, uh."));
            assert!(item.llm_applied);
            assert_eq!(item.word_count, 0);
            assert!(paste.pasted_texts.lock().unwrap().is_empty());
            assert!(paste.copied_texts.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn vocabulary_terms_are_protected_during_automatic_cleanup() {
        let raw = "Please keep um unchanged here.";
        let state = test_state(MockAudioCapture::sine_wave(), MockAsrEngine::with_text(raw),
            Some(MockLlmBackend::proposal("Please keep unchanged here.")), Box::new(MockPasteBackend::new()));
        state.settings.lock().await.vocabulary = vec!["um".into()];
        let events = CollectingEvents::new();
        begin(&state, &events).unwrap();
        assert!(stop(&state, &events).await);
        let last = state.last_transcription.lock().await;
        let item = last.as_ref().unwrap();
        assert_eq!(item.text, raw);
        assert!(!item.llm_applied);
        // Same mask semantics as protected_word_deletions: the single window
        // is fully term-protected ⇒ provably no work ⇒ never dispatched
        // (NoChanges), or dispatched-and-refused (Rejected). Both preserve
        // the raw; the vocabulary guard is what makes the harmful edit
        // impossible either way.
        assert!(
            matches!(
                item.llm_cleanup_status,
                LlmCleanupStatus::Rejected { .. } | LlmCleanupStatus::NoChanges
            ),
            "{:?}",
            item.llm_cleanup_status
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn cleanup_no_op_preserves_text_without_an_applied_badge() {
        let raw = "Please keep um, every word in this transcript.";
        let state = test_state(
            MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text(raw),
            Some(MockLlmBackend::passthrough()),
            Box::new(MockPasteBackend::new()),
        );
        let events = CollectingEvents::new();
        begin(&state, &events).unwrap();
        assert!(stop(&state, &events).await);
        let last = state.last_transcription.lock().await;
        let transcription = last.as_ref().unwrap();
        assert_eq!(transcription.text, raw);
        assert!(transcription.raw_text.is_none());
        assert!(!transcription.llm_applied);
        assert_eq!(transcription.llm_cleanup_status, LlmCleanupStatus::NoChanges);
    }

    // --- Test 3: ASR error ---
    #[tokio::test]
    #[serial_test::serial]
    async fn test_pipeline_asr_error() {
        let mock_paste = Arc::new(MockPasteBackend::new());
        let state = test_state(
            MockAudioCapture::sine_wave(),
            MockAsrEngine::with_error("Model failed to load"),
            None,
            Box::new(SharedMockPaste(mock_paste.clone())),
        );
        let events = CollectingEvents::new();

        begin(&state, &events).unwrap();
        assert!(stop(&state, &events).await);

        // Assert state returned to Idle
        assert_eq!(state.get_state(), AppStateEnum::Idle);

        // Assert no paste occurred
        assert!(mock_paste.last_pasted().is_none());
        assert!(mock_paste.copied_texts.lock().unwrap().is_empty());

        // Assert the error was reported with the recoverable audio
        let failure = events.failures_of(FailureKind::Transcription).remove(0);
        assert!(failure.error.contains("Model failed to load"));
        let retained = failure.audio_path.expect("recoverable audio path");
        assert!(std::path::Path::new(&retained).is_file());
        let reader = hound::WavReader::open(&retained).unwrap();
        assert!(reader.len() > 4000);
        drop(reader);
        std::fs::remove_file(&retained).unwrap();
    }

    // --- Test 4: Recording cancellation ---
    #[tokio::test]
    #[serial_test::serial]
    async fn test_pipeline_cancel() {
        let mock_paste = Arc::new(MockPasteBackend::new());
        let state = test_state(
            MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("hello world"),
            None,
            Box::new(SharedMockPaste(mock_paste.clone())),
        );
        let events = CollectingEvents::new();

        begin(&state, &events).unwrap();
        assert_eq!(state.get_state(), AppStateEnum::Recording);

        // Cancel instead of stop
        assert!(cancel_and_complete(&state, &events).await);

        // Assert state returned to Idle
        assert_eq!(state.get_state(), AppStateEnum::Idle);

        // Assert no paste occurred
        assert!(mock_paste.last_pasted().is_none());
        assert!(mock_paste.copied_texts.lock().unwrap().is_empty());

        // Assert cancellation event was emitted
        assert!(*events.recording_cancelled.lock().unwrap());
        assert!(events.transcriptions.lock().unwrap()[0].cancelled);
        assert_eq!(last_outcome(&events), Some(Outcome::Dismissed));
    }

    // --- Test 5: State machine guards ---
    #[tokio::test]
    async fn test_start_while_recording_fails() {
        let state = test_state(
            MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("hello"),
            None,
            Box::new(MockPasteBackend::new()),
        );
        let events = CollectingEvents::new();

        // Start recording
        begin(&state, &events).unwrap();
        assert_eq!(state.get_state(), AppStateEnum::Recording);

        // Attempt to start again -- should fail
        let result = begin(&state, &events);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Cannot start recording"));
        assert!(events.busy.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_stop_while_idle_is_ignored() {
        let state = test_state(
            MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("hello"),
            None,
            Box::new(MockPasteBackend::new()),
        );
        let events = CollectingEvents::new();

        // State is Idle, stopping claims nothing and changes nothing
        assert_eq!(state.get_state(), AppStateEnum::Idle);
        assert!(claim_and_finish(&state, &events, None).await.is_none());
        assert!(cancel(&state, &events).await.is_none());
        assert!(!*events.recording_stopped.lock().unwrap());
        assert!(!*events.recording_cancelled.lock().unwrap());
        assert!(events.outcomes.lock().unwrap().is_empty());
        assert_eq!(state.get_state(), AppStateEnum::Idle);
    }

    // --- Test 6: Job ID staleness ---
    #[tokio::test]
    #[serial_test::serial]
    async fn test_stale_job_discarded() {
        use crate::asr::engine::AsrEngine;

        /// Mock ASR that bumps the job ID as a side effect.
        struct StaleJobAsrEngine {
            state_ref: Arc<AppState>,
        }
        impl AsrEngine for StaleJobAsrEngine {
            fn init(&mut self) -> Result<(), String> { Ok(()) }
            fn is_ready(&self) -> bool { true }
            fn transcribe_file(&mut self, _path: &str) -> Result<AsrResult, String> {
                self.state_ref.new_job();
                Ok(AsrResult { unboosted_text: None,
                    text: "stale result".to_string(),
                    duration_secs: 1.0,
                    processing_time_secs: 0.01,
                    rtfx: 100.0,
                })
            }
            fn transcribe_samples(&mut self, _: &[f32], _: u32) -> Result<AsrResult, String> {
                self.transcribe_file("")
            }
            fn is_model_available(&self) -> bool { true }
            fn backend_name(&self) -> &'static str { "stale-job-mock" }
        }

        let mock_paste = Arc::new(MockPasteBackend::new());

        let settings = Settings {
            auto_paste: true,
            llm_cleanup_enabled: false,
            ..Default::default()
        };

        let state = Arc::new(AppState::new_with_backends(
            Box::new(MockAudioCapture::sine_wave()),
            Box::new(MockAsrEngine::with_text("placeholder")),
            None,
            Box::new(SharedMockPaste(mock_paste.clone())),
            settings,
        ));

        // Replace ASR engine with the side-effect mock
        {
            let mut engine = state.asr_engine.lock().await;
            *engine = Box::new(StaleJobAsrEngine { state_ref: state.clone() });
        }

        let events = CollectingEvents::new();

        begin(&state, &events).unwrap();
        assert!(stop(&state, &events).await);

        // Assert no paste occurred (stale job discarded after ASR returned)
        assert!(mock_paste.last_pasted().is_none());
        assert!(events.transcriptions.lock().unwrap().is_empty());
        assert_eq!(last_outcome(&events), Some(Outcome::Stale));
        // The recording generation did not move on, so this job still owns the state.
        assert_eq!(state.get_state(), AppStateEnum::Idle);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn stale_cleanup_does_not_publish_status_or_deliver_text() {
        struct SupersededCleanup(std::sync::Weak<AppState>);
        impl crate::llm::engine::LlmBackend for SupersededCleanup {
            fn cleanup_batch(
                &mut self,
                texts: &[String],
                _mode: crate::models::CleanupMode,
            ) -> Result<Vec<crate::llm::engine::BatchItem>, String> {
                // A newer recording starts while this cleanup runs.
                let state = self.0.upgrade().unwrap();
                state.new_job();
                state.recording_generation.fetch_add(1, Ordering::SeqCst);
                state.set_state(AppStateEnum::Recording);
                Ok(texts
                    .iter()
                    .map(|t| crate::llm::engine::BatchItem::Proposal(Some(t.clone())))
                    .collect())
            }
            fn request_raw(&mut self, _: &serde_json::Value) -> Result<serde_json::Value, String> {
                unreachable!()
            }
        }
        let paste = Arc::new(MockPasteBackend::new());
        let state = Arc::new(test_state(MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("Please um, keep the entire final instruction."),
            Some(MockLlmBackend::passthrough()), Box::new(SharedMockPaste(paste.clone()))));
        *state.llm_engine.lock().await = Some(Box::new(SupersededCleanup(Arc::downgrade(&state))));
        let events = CollectingEvents::new();
        begin(&state, &events).unwrap();
        assert!(stop(&state, &events).await);
        assert!(state.last_transcription.lock().await.is_none());
        assert_eq!(*state.llm_last_status.lock().await, LlmCleanupStatus::Idle);
        assert!(events.cleanup_statuses.lock().unwrap().is_empty());
        assert_eq!(state.get_state(), AppStateEnum::Recording);
        assert!(events.transcriptions.lock().unwrap().is_empty());
        assert!(paste.pasted_texts.lock().unwrap().is_empty());
        assert!(paste.copied_texts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn stale_generation_discards_delivery() {
        let paste = Arc::new(MockPasteBackend::new());
        let state = test_state(MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("Words from a superseded recording."), None,
            Box::new(SharedMockPaste(paste.clone())));
        let events = CollectingEvents::new();
        begin(&state, &events).unwrap();
        let job = claim_and_finish(&state, &events, None).await.unwrap();
        let audio = job.audio_path.clone();
        // A newer recording now owns the application state.
        state.recording_generation.fetch_add(1, Ordering::SeqCst);
        complete(&state, &events, job).await;
        assert!(paste.pasted_texts.lock().unwrap().is_empty());
        assert!(paste.copied_texts.lock().unwrap().is_empty());
        assert!(state.last_transcription.lock().await.is_none());
        assert!(events.transcriptions.lock().unwrap().is_empty());
        assert!(events.failures.lock().unwrap().is_empty());
        assert_eq!(last_outcome(&events), Some(Outcome::Stale));
        // Not reset to Idle: the state belongs to the newer recording.
        assert_eq!(state.get_state(), AppStateEnum::Transcribing);
        assert!(audio.is_file(), "an undelivered recording keeps its audio");
        std::fs::remove_file(&audio).unwrap();
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn paste_happens_before_the_history_save() {
        use crate::paste::PasteBackend;
        struct GatedPaste {
            entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
            release: Mutex<std::sync::mpsc::Receiver<()>>,
            pasted: Arc<Mutex<Vec<String>>>,
        }
        impl PasteBackend for GatedPaste {
            fn paste_text(&self, text: &str, _target_pid: i32) -> Result<(), String> {
                if let Some(entered) = self.entered.lock().unwrap().take() { let _ = entered.send(()); }
                self.release.lock().unwrap().recv_timeout(Duration::from_secs(5))
                    .map_err(|_| "The test never released the paste".to_string())?;
                self.pasted.lock().unwrap().push(text.to_string());
                Ok(())
            }
            fn paste_text_and_restore(&self, text: &str, target_pid: i32) -> Result<(), String> {
                self.paste_text(text, target_pid)
            }
            fn copy_to_clipboard(&self, _text: &str) -> Result<(), String> { Ok(()) }
            fn get_frontmost_pid(&self) -> i32 { 4242 }
            fn is_accessibility_trusted(&self) -> bool { true }
        }
        let (entered, paste_entered) = tokio::sync::oneshot::channel();
        let (release, waiting) = std::sync::mpsc::channel();
        let pasted = Arc::new(Mutex::new(Vec::new()));
        let state = test_state(MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("Deliver before saving."), None,
            Box::new(GatedPaste { entered: Mutex::new(Some(entered)), release: Mutex::new(waiting), pasted: pasted.clone() }));
        let events = CollectingEvents::new();
        begin(&state, &events).unwrap();
        let job = claim_and_finish(&state, &events, None).await.unwrap();
        let id = job.recording_id();
        let observe = async {
            paste_entered.await.unwrap();
            let history = crate::commands::transcription::get_transcriptions().await.unwrap();
            let saved_during_paste = history.iter().any(|item| item.id == id);
            release.send(()).unwrap();
            saved_during_paste
        };
        let ((), saved_during_paste) = tokio::join!(complete(&state, &events, job), observe);
        assert!(!saved_during_paste, "history must be written after the paste");
        assert_eq!(*pasted.lock().unwrap(), vec!["Deliver before saving.".to_string()]);
        let history = crate::commands::transcription::get_transcriptions().await.unwrap();
        assert!(history.iter().any(|item| item.id == id));
        assert_eq!(*events.saved.lock().unwrap(), vec![true]);
        assert_eq!(state.get_state(), AppStateEnum::Idle);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn history_failure_after_paste_keeps_the_audio() {
        let paste = Arc::new(MockPasteBackend::new());
        let state = test_state(MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("Delivered but not saved."), None,
            Box::new(SharedMockPaste(paste.clone())));
        let events = CollectingEvents::new();
        begin(&state, &events).unwrap();
        let job = claim_and_finish(&state, &events, None).await.unwrap();
        let audio = job.audio_path.clone();
        crate::commands::transcription::fail_next_save(&job.recording_id());
        complete(&state, &events, job).await;

        assert_eq!(paste.last_pasted().unwrap().text, "Delivered but not saved.");
        assert_eq!(*events.saved.lock().unwrap(), vec![false]);
        assert_eq!(events.history_errors.lock().unwrap().len(), 1);
        let failures = events.failures.lock().unwrap().clone();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].kind, FailureKind::HistorySave);
        let audio_text = audio.to_string_lossy().into_owned();
        assert_eq!(failures[0].audio_path.as_deref(), Some(audio_text.as_str()));
        assert!(audio.is_file(), "an unsaved transcript keeps its recording");
        assert_eq!(last_outcome(&events), Some(Outcome::Failed));
        assert_eq!(state.get_state(), AppStateEnum::Idle);
        std::fs::remove_file(&audio).unwrap();
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn own_pid_target_copies_instead_of_pasting() {
        // The guard holds whether or not focus is restored before the paste.
        for restore_focus in [true, false] {
            let paste = Arc::new(MockPasteBackend {
                frontmost_pid: std::process::id() as i32,
                ..MockPasteBackend::new()
            });
            let state = test_state(MockAudioCapture::sine_wave(),
                MockAsrEngine::with_text("Copied, not pasted."), None,
                Box::new(SharedMockPaste(paste.clone())));
            state.settings.lock().await.restore_focus_before_paste = restore_focus;
            let events = CollectingEvents::new();
            begin(&state, &events).unwrap();
            assert!(stop(&state, &events).await);

            assert!(paste.pasted_texts.lock().unwrap().is_empty());
            assert_eq!(*paste.copied_texts.lock().unwrap(), vec!["Copied, not pasted.".to_string()]);
            let failures = events.failures_of(FailureKind::Paste);
            assert_eq!(failures.len(), 1);
            assert_eq!(failures[0].error, OWN_TARGET_COPIED);
            assert_eq!(failures[0].clipboard_available, Some(true));
            assert_eq!(*events.saved.lock().unwrap(), vec![true]);
            assert_eq!(state.get_state(), AppStateEnum::Idle);
        }
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    #[serial_test::serial]
    async fn asr_deadline_keeps_audio_and_later_requests_fail_at_once() {
        use crate::asr::engine::AsrEngine;
        struct HangingAsr {
            entered: Option<tokio::sync::oneshot::Sender<()>>,
            release: std::sync::mpsc::Receiver<()>,
            panics: bool,
        }
        impl AsrEngine for HangingAsr {
            fn init(&mut self) -> Result<(), String> { Ok(()) }
            fn is_ready(&self) -> bool { true }
            fn is_model_available(&self) -> bool { true }
            fn backend_name(&self) -> &'static str { "hanging" }
            fn transcribe_samples(&mut self, _: &[f32], _: u32) -> Result<AsrResult, String> { unreachable!() }
            fn transcribe_file(&mut self, _: &str) -> Result<AsrResult, String> {
                if let Some(entered) = self.entered.take() { let _ = entered.send(()); }
                // Returns when the test releases it, or when the test ends.
                let _ = self.release.recv();
                if self.panics {
                    panic!("speech recognition crashed after its deadline");
                }
                Err("returned after its deadline".into())
            }
        }
        // The stuck flag clears when the late call returns and when it panics.
        for panics in [false, true] {
            let (entered, asr_entered) = tokio::sync::oneshot::channel();
            let (release, waiting) = std::sync::mpsc::channel();
            let paste = Arc::new(MockPasteBackend::new());
            let state = AppState::new_with_backends(
                Box::new(MockAudioCapture::silence()),
                Box::new(HangingAsr { entered: Some(entered), release: waiting, panics }), None,
                Box::new(SharedMockPaste(paste.clone())), Settings::default(),
            );
            let events = CollectingEvents::new();
            begin(&state, &events).unwrap();
            let job = claim_and_finish(&state, &events, None).await.unwrap();
            let audio = job.audio_path.clone();
            let deadline = ASR_BASE_DEADLINE + Duration::from_millis(job.duration_ms / 2);
            let expire = async {
                asr_entered.await.unwrap();
                tokio::time::advance(deadline + Duration::from_secs(1)).await;
            };
            tokio::join!(complete(&state, &events, job), expire);

            assert_eq!(state.get_state(), AppStateEnum::Idle);
            assert!(state.asr_stuck.load(Ordering::SeqCst));
            let failures = events.failures_of(FailureKind::Transcription);
            assert_eq!(failures.len(), 1);
            assert_eq!(failures[0].error, ASR_DEADLINE_ERROR);
            let audio_text = audio.to_string_lossy().into_owned();
            assert_eq!(failures[0].audio_path.as_deref(), Some(audio_text.as_str()));
            assert!(audio.is_file(), "a timed-out recording keeps its audio");
            assert!(paste.pasted_texts.lock().unwrap().is_empty());

            // The stuck call still holds the engine; later requests must not wait for it.
            let later = transcribe_with_deadline(&state, audio_text.clone(), Vec::new(), 1_000).await;
            assert_eq!(later.unwrap_err(), ASR_STUCK_ERROR);

            release.send(()).unwrap();
            // The engine is free once the stuck call ends, and the flag clears first.
            drop(state.asr_engine.lock().await);
            assert!(!state.asr_stuck.load(Ordering::SeqCst), "stuck flag after panics={panics}");
            std::fs::remove_file(&audio).unwrap();
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn transcription_waits_for_the_speech_model() {
        for (init_error, expected) in [
            (None, MODEL_LOADING_ERROR.to_string()),
            // A load that failed reports why, instead of "still loading" forever.
            (Some("Speech model files are damaged."), "Speech model files are damaged. Your recording was kept.".to_string()),
        ] {
            let paste = Arc::new(MockPasteBackend::new());
            let state = test_state(MockAudioCapture::sine_wave(),
                MockAsrEngine::with_text("not yet"), None, Box::new(SharedMockPaste(paste.clone())));
            state.is_model_loaded.store(false, Ordering::SeqCst);
            *state.asr_init_error.lock().unwrap() = init_error.map(str::to_string);
            let events = CollectingEvents::new();
            begin(&state, &events).unwrap();
            assert!(stop(&state, &events).await);
            let failure = events.failures_of(FailureKind::Transcription).remove(0);
            assert_eq!(failure.error, expected);
            let audio = failure.audio_path.expect("the recording is kept");
            assert!(std::path::Path::new(&audio).is_file());
            assert!(paste.pasted_texts.lock().unwrap().is_empty());
            assert_eq!(state.get_state(), AppStateEnum::Idle);
            std::fs::remove_file(&audio).unwrap();
        }
    }

    // --- Test 7: LLM cleanup failure falls back to raw text ---
    #[tokio::test]
    #[serial_test::serial]
    async fn test_llm_failure_uses_raw_text() {
        let mock_paste = Arc::new(MockPasteBackend::new());
        let state = test_state(
            MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("hello um, world this is a test sentence"),
            Some(MockLlmBackend::failing("sidecar crashed")),
            Box::new(SharedMockPaste(mock_paste.clone())),
        );
        let events = CollectingEvents::new();

        begin(&state, &events).unwrap();
        assert!(stop(&state, &events).await);

        // Assert raw text was pasted (LLM failed, graceful fallback)
        let pasted = mock_paste.last_pasted().expect("should have pasted");
        assert_eq!(pasted.text, "hello um, world this is a test sentence");

        // Assert transcription records the failure with structured status
        let last = state.last_transcription.lock().await;
        let t = last.as_ref().unwrap();
        assert!(!t.llm_applied);
        match &t.llm_cleanup_status {
            LlmCleanupStatus::Failed { reason } => {
                assert!(reason.contains("sidecar crashed"));
            }
            other => panic!("expected Failed status, got {:?}", other),
        }
    }

    // Short transcripts must reach cleanup too.
    #[tokio::test]
    #[serial_test::serial]
    async fn test_short_input_reaches_cleanup() {
        let mock_paste = Arc::new(MockPasteBackend::new());
        let state = test_state(
            MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("um hello"),
            Some(MockLlmBackend::proposal("Hello")),
            Box::new(SharedMockPaste(mock_paste.clone())),
        );
        let events = CollectingEvents::new();

        begin(&state, &events).unwrap();
        assert!(stop(&state, &events).await);

        // The old five-word gate must not bypass useful short cleanup.
        let pasted = mock_paste.last_pasted().expect("should have pasted");
        assert_eq!(pasted.text, "Hello");

        let last = state.last_transcription.lock().await;
        let t = last.as_ref().unwrap();
        assert!(t.llm_applied);
        assert!(matches!(t.llm_cleanup_status, LlmCleanupStatus::Applied { .. }));
    }

    // --- Test 8: Short recording discarded ---
    #[tokio::test]
    async fn test_short_recording_discarded() {
        let mock_paste = Arc::new(MockPasteBackend::new());
        // Only 100 samples -- below the 4000-sample minimum
        let state = test_state(
            MockAudioCapture::new(low_level_noise(100), 48_000),
            MockAsrEngine::with_text("should not reach ASR"),
            None,
            Box::new(SharedMockPaste(mock_paste.clone())),
        );
        let events = CollectingEvents::new();

        begin(&state, &events).unwrap();
        assert!(claim_and_finish(&state, &events, None).await.is_none());

        // Assert no paste, no transcription (short recording silently discarded)
        assert!(*events.recording_stopped.lock().unwrap());
        assert!(mock_paste.last_pasted().is_none());
        assert!(events.transcriptions.lock().unwrap().is_empty());
        assert_eq!(last_outcome(&events), Some(Outcome::Dismissed));
        assert_eq!(state.get_state(), AppStateEnum::Idle);
    }

    // --- Test 9: Paste failure copies to clipboard as fallback ---
    #[tokio::test]
    #[serial_test::serial]
    async fn test_paste_failure_clipboard_fallback() {
        let mock_paste = Arc::new(MockPasteBackend::new());
        *mock_paste.paste_error.lock().unwrap() = Some("Accessibility denied".into());

        let state = test_state(
            MockAudioCapture::sine_wave(),
            MockAsrEngine::with_text("hello world"),
            None,
            Box::new(SharedMockPaste(mock_paste.clone())),
        );
        let events = CollectingEvents::new();

        begin(&state, &events).unwrap();
        assert!(stop(&state, &events).await);

        // Assert paste was attempted but failed, with the text on the clipboard
        let paste_failures = events.failures_of(FailureKind::Paste);
        assert_eq!(paste_failures.len(), 1);
        assert!(paste_failures[0].error.contains("Accessibility denied"));
        assert_eq!(paste_failures[0].clipboard_available, Some(true));

        // Assert text was copied to clipboard as fallback
        let copied = mock_paste.copied_texts.lock().unwrap();
        assert_eq!(copied.len(), 1);
        assert_eq!(copied[0], "hello world");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn failed_paste_and_failed_copy_report_both_errors_with_history_intact() {
        let paste = Arc::new(MockPasteBackend::new());
        *paste.paste_error.lock().unwrap() = Some("Paste permission denied".into());
        *paste.copy_error.lock().unwrap() = Some("Clipboard unavailable".into());
        let state = test_state(MockAudioCapture::sine_wave(), MockAsrEngine::with_text("Recoverable words."),
            None, Box::new(SharedMockPaste(Arc::clone(&paste))));
        let events = CollectingEvents::new();
        begin(&state, &events).unwrap();
        assert!(stop(&state, &events).await);
        {
            let failures = events.failures_of(FailureKind::Paste);
            assert!(failures[0].error.contains("Paste permission denied"));
            assert!(failures[0].error.contains("Clipboard unavailable"));
            assert_eq!(failures[0].clipboard_available, Some(false));
            assert!(failures[0].audio_path.is_none());
        }
        assert!(paste.copied_texts.lock().unwrap().is_empty());
        assert!(events.paste_ids.lock().unwrap().is_empty());
        assert_eq!(*events.saved.lock().unwrap(), vec![true]);
        assert_eq!(state.last_transcription.lock().await.as_ref().unwrap().text, "Recoverable words.");
    }

    // --- Test 10: Auto-paste disabled -- copy to clipboard only ---
    #[tokio::test]
    #[serial_test::serial]
    async fn test_auto_paste_disabled() {
        let mock_paste = Arc::new(MockPasteBackend::new());

        let settings = Settings {
            auto_paste: false,
            llm_cleanup_enabled: false,
            ..Default::default()
        };

        let state = AppState::new_with_backends(
            Box::new(MockAudioCapture::sine_wave()),
            Box::new(MockAsrEngine::with_text("hello world")),
            None,
            Box::new(SharedMockPaste(mock_paste.clone())),
            settings,
        );
        let events = CollectingEvents::new();

        begin(&state, &events).unwrap();
        assert!(stop(&state, &events).await);

        // Assert no paste (paste_text was never called)
        assert!(mock_paste.pasted_texts.lock().unwrap().is_empty());

        // Assert text was copied to clipboard instead
        let copied = mock_paste.copied_texts.lock().unwrap();
        assert_eq!(copied[0], "hello world");
        assert!(events.paste_ids.lock().unwrap().iter().all(|(_, clipboard_only)| *clipboard_only));
    }
}
