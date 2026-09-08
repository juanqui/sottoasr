use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64};
use std::sync::Mutex as StdMutex;
use tokio::sync::Mutex as TokioMutex;
use crate::asr::engine::{AsrEngine, SharedAsrEngine};
use crate::audio::capture::{AudioCapture, AudioCaptureBackend};
use crate::llm::engine::LlmBackend;
use crate::paste::PasteBackend;
use crate::models::{AppStateEnum, LlmCleanupStatus, Settings, Transcription};

/// Per-show session state for the overlay panel. Used to detect whether
/// the panel was moved by the user (dragged) between show and hide, so
/// that `hide_overlay` persists only user-chosen positions — not the
/// auto-computed default that `show_overlay` itself just wrote.
///
/// See docs/specs/2026-04-11-overlay-positioning-multi-monitor-fix.md §5.2.
#[derive(Clone, Copy, Debug)]
pub struct OverlaySession {
    /// The display the overlay was positioned onto.
    pub display_id: u32,
    /// The exact (x, y) the default formula produced for this display,
    /// before any user interaction.
    pub default_origin: (f64, f64),
    /// The exact (x, y) we finally set — either `default_origin` or a
    /// valid restored user position.
    pub applied_origin: (f64, f64),
}

pub struct AppState {
    pub current_state: StdMutex<AppStateEnum>,
    pub settings: TokioMutex<Settings>,
    pub settings_update: TokioMutex<()>,
    pub shortcuts_updating: AtomicBool,
    pub is_exiting: AtomicBool,
    pub settings_load_error: Option<String>,
    pub last_transcription: TokioMutex<Option<Transcription>>,
    pub is_recording: AtomicBool,
    pub is_model_loaded: AtomicBool,
    pub asr_initializing: AtomicBool,
    pub asr_init_error: StdMutex<Option<String>>,
    // Audio capture — managed by hotkey handlers
    pub audio_capture: std::sync::Arc<StdMutex<Box<dyn AudioCaptureBackend>>>,
    pub capture_health: std::sync::Arc<StdMutex<crate::audio::capture::CaptureHealth>>,
    // Audio buffer: samples sent via channel from cpal callback
    pub audio_sender: StdMutex<std::sync::mpsc::Sender<Vec<f32>>>,
    pub audio_receiver: std::sync::Arc<StdMutex<std::sync::mpsc::Receiver<Vec<f32>>>>,
    // ASR engine
    pub asr_engine: SharedAsrEngine,
    pub vocabulary_operation: TokioMutex<()>,
    pub vocabulary_runtime: StdMutex<crate::commands::vocabulary::VocabularyRuntime>,
    pub vocabulary_terms: std::sync::Arc<StdMutex<Vec<String>>>,
    pub recording_vocabulary: StdMutex<Vec<String>>,
    // Serializes resident sidecar ownership across cleanup and lifecycle commands.
    // Lock before llm_engine and hold while a blocking task owns the handle.
    pub llm_operation: TokioMutex<()>,
    // LLM engine for transcript cleanup
    pub llm_engine: TokioMutex<Option<Box<dyn LlmBackend>>>,
    // PID of the currently-running LLM sidecar subprocess, or 0 if none.
    // Captured from Child::id() at spawn time. Used by kill_orphan() to
    // SIGKILL the subprocess on timeout/panic without needing ownership of
    // the Child handle (which is held by the blocking cleanup task).
    // See docs/specs/2026-04-11-llm-cleanup-reliability.md §4.3.
    pub llm_pid: std::sync::Arc<AtomicI32>,
    pub llm_preparing: AtomicBool,
    pub llm_loaded: AtomicBool,
    pub llm_downloading: AtomicBool,
    pub llm_setup_error: TokioMutex<Option<String>>,
    pub llm_preparation_finished: tokio::sync::Notify,
    // Most recent cleanup outcome. Read by the frontend via get_llm_status
    // and updated by run_cleanup() after every recording.
    pub llm_last_status: TokioMutex<LlmCleanupStatus>,
    // Paste backend — abstracts clipboard/paste operations
    pub paste_backend: std::sync::Arc<dyn PasteBackend>,
    // Monotonic job ID for stale-result prevention
    pub current_job_id: AtomicU64,
    // Cancel shortcut strings — registered only while recording
    pub cancel_shortcut: StdMutex<String>,
    pub cancel_shortcut_alt: StdMutex<Option<String>>,
    // Recording generation counter — incremented on each new recording so stale
    // auto-stop timers from previous sessions can detect they are obsolete.
    pub recording_generation: AtomicU64,
    // PID of the frontmost application when recording started.
    // Used to target Cmd+V paste at the correct app (avoids focus race conditions).
    // 0 means no target captured — fall back to HID posting.
    pub target_pid: AtomicI32,
    // Overlay show/hide session state. Set on show, cleared on hide.
    // Used to distinguish user-dragged positions from auto-computed defaults.
    pub overlay_session: StdMutex<Option<OverlaySession>>,
    pub overlay_snapshot: StdMutex<crate::commands::overlay::OverlaySnapshot>,
}

/// Excludes microphone acquisition only while shortcut activation, persistence
/// and possible rollback form one transaction. Cancellation releases it too.
pub(crate) struct ShortcutUpdate<'a>(&'a AtomicBool);
impl Drop for ShortcutUpdate<'_> {
    fn drop(&mut self) { self.0.store(false, std::sync::atomic::Ordering::SeqCst); }
}

impl AppState {
    pub fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let (settings, settings_load_error) = match crate::commands::settings::load_persisted_settings_checked() {
            Ok(settings) => (settings, None),
            Err(error) => {
                log::error!("{error}; saved settings preserved, starting with temporary defaults");
                (Settings::default(), Some(error))
            }
        };
        let vocabulary = settings.vocabulary.clone();
        let cancel = settings.cancel_shortcut.clone();
        let cancel_alt = settings.cancel_shortcut_alt.clone();

        #[cfg(target_os = "macos")]
        let paste: Box<dyn PasteBackend> = Box::new(crate::paste::MacOsPasteBackend);
        #[cfg(not(target_os = "macos"))]
        let paste: Box<dyn PasteBackend> = Box::new(crate::paste::StubPasteBackend);

        Self {
            current_state: StdMutex::new(AppStateEnum::Idle),
            settings: TokioMutex::new(settings),
            settings_update: TokioMutex::new(()),
            shortcuts_updating: AtomicBool::new(false),
            is_exiting: AtomicBool::new(false),
            settings_load_error,
            last_transcription: TokioMutex::new(None),
            is_recording: AtomicBool::new(false),
            asr_initializing: AtomicBool::new(false),
            asr_init_error: StdMutex::new(None),
            is_model_loaded: AtomicBool::new(false),
            audio_capture: std::sync::Arc::new(StdMutex::new(Box::new(AudioCapture::new()))),
            capture_health: Default::default(),
            audio_sender: StdMutex::new(tx),
            audio_receiver: std::sync::Arc::new(StdMutex::new(rx)),
            asr_engine: std::sync::Arc::new(TokioMutex::new(crate::asr::engine::create_engine())),
            vocabulary_operation: TokioMutex::new(()),
            vocabulary_runtime: StdMutex::new(crate::commands::vocabulary::VocabularyRuntime::default()),
            vocabulary_terms: std::sync::Arc::new(StdMutex::new(vocabulary)),
            recording_vocabulary: StdMutex::new(Vec::new()),
            llm_operation: TokioMutex::new(()),
            llm_engine: TokioMutex::new(None),
            llm_pid: std::sync::Arc::new(AtomicI32::new(0)),
            llm_preparing: AtomicBool::new(false),
            llm_loaded: AtomicBool::new(false),
            llm_downloading: AtomicBool::new(false),
            llm_setup_error: TokioMutex::new(None),
            llm_preparation_finished: tokio::sync::Notify::new(),
            llm_last_status: TokioMutex::new(LlmCleanupStatus::Idle),
            paste_backend: paste.into(),
            current_job_id: AtomicU64::new(0),
            cancel_shortcut: StdMutex::new(cancel),
            cancel_shortcut_alt: StdMutex::new(cancel_alt),
            recording_generation: AtomicU64::new(0),
            target_pid: AtomicI32::new(0),
            overlay_session: StdMutex::new(None),
            overlay_snapshot: StdMutex::new(Default::default()),
        }
    }

    /// Construct AppState with injected backends. Used by integration tests.
    pub fn new_with_backends(
        audio: Box<dyn AudioCaptureBackend>,
        asr: Box<dyn AsrEngine>,
        llm: Option<Box<dyn LlmBackend>>,
        paste: Box<dyn PasteBackend>,
        settings: Settings,
    ) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let vocabulary = settings.vocabulary.clone();
        let cancel = settings.cancel_shortcut.clone();
        let cancel_alt = settings.cancel_shortcut_alt.clone();
        Self {
            current_state: StdMutex::new(AppStateEnum::Idle),
            settings: TokioMutex::new(settings),
            settings_update: TokioMutex::new(()),
            shortcuts_updating: AtomicBool::new(false),
            is_exiting: AtomicBool::new(false),
            settings_load_error: None,
            last_transcription: TokioMutex::new(None),
            is_recording: AtomicBool::new(false),
            asr_initializing: AtomicBool::new(false),
            asr_init_error: StdMutex::new(None),
            is_model_loaded: AtomicBool::new(true),
            audio_capture: std::sync::Arc::new(StdMutex::new(audio)),
            capture_health: Default::default(),
            audio_sender: StdMutex::new(tx),
            audio_receiver: std::sync::Arc::new(StdMutex::new(rx)),
            asr_engine: std::sync::Arc::new(TokioMutex::new(asr)),
            vocabulary_operation: TokioMutex::new(()),
            vocabulary_runtime: StdMutex::new(crate::commands::vocabulary::VocabularyRuntime::default()),
            vocabulary_terms: std::sync::Arc::new(StdMutex::new(vocabulary)),
            recording_vocabulary: StdMutex::new(Vec::new()),
            llm_operation: TokioMutex::new(()),
            llm_engine: TokioMutex::new(llm),
            llm_pid: std::sync::Arc::new(AtomicI32::new(0)),
            llm_preparing: AtomicBool::new(false),
            llm_loaded: AtomicBool::new(false),
            llm_downloading: AtomicBool::new(false),
            llm_setup_error: TokioMutex::new(None),
            llm_preparation_finished: tokio::sync::Notify::new(),
            llm_last_status: TokioMutex::new(LlmCleanupStatus::Idle),
            paste_backend: paste.into(),
            current_job_id: AtomicU64::new(0),
            cancel_shortcut: StdMutex::new(cancel),
            cancel_shortcut_alt: StdMutex::new(cancel_alt),
            recording_generation: AtomicU64::new(0),
            target_pid: AtomicI32::new(0),
            overlay_session: StdMutex::new(None),
            overlay_snapshot: StdMutex::new(Default::default()),
        }
    }

    /// An accepted exit and a new microphone acquisition cannot cross.
    pub(crate) fn begin_exit(&self) -> Result<(), String> {
        let current = self.current_state.lock().unwrap_or_else(|error| error.into_inner());
        if *current != AppStateEnum::Idle {
            return Err("Finish recording or wait for transcription to complete before quitting or restarting.".into());
        }
        self.is_exiting.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    pub(crate) fn begin_shortcut_update(&self) -> Result<ShortcutUpdate<'_>, String> {
        let current = self.current_state.lock().unwrap_or_else(|error| error.into_inner());
        if *current != AppStateEnum::Idle {
            return Err("Finish recording before changing keyboard shortcuts".into());
        }
        if self.shortcuts_updating.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return Err("Keyboard shortcuts are already being saved".into());
        }
        Ok(ShortcutUpdate(&self.shortcuts_updating))
    }

    /// Get a new job ID and set it as current.
    pub fn new_job(&self) -> u64 {
        let id = crate::llm::engine::next_job_id();
        self.current_job_id.store(id, std::sync::atomic::Ordering::SeqCst);
        id
    }

    /// Check if the given job ID is still the current one.
    pub fn is_current_job(&self, id: u64) -> bool {
        self.current_job_id.load(std::sync::atomic::Ordering::SeqCst) == id
    }

    /// Claim a recording phase once across concurrent hotkey and IPC handlers.
    pub fn try_transition(&self, expected: AppStateEnum, next: AppStateEnum) -> bool {
        let mut state = self.current_state.lock().unwrap_or_else(|error| error.into_inner());
        if *state != expected { return false; }
        *state = next;
        true
    }

    /// Timers and PTT releases may finish after their recording has ended.
    /// Compare the generation and claim the phase under the same state lock
    /// used when starting a new microphone session.
    pub fn claim_recording_end(&self, generation: Option<u64>) -> bool {
        let mut state = self.current_state.lock().unwrap_or_else(|error| error.into_inner());
        if *state != AppStateEnum::Recording || generation.is_some_and(|generation| {
            self.recording_generation.load(std::sync::atomic::Ordering::SeqCst) != generation
        }) {
            return false;
        }
        *state = AppStateEnum::Transcribing;
        true
    }

    pub fn set_state(&self, new_state: AppStateEnum) {
        if let Ok(mut state) = self.current_state.lock() {
            *state = new_state;
        }
    }

    #[cfg(test)]
    pub fn capture_error(&self) -> Option<String> {
        self.capture_health.lock().unwrap_or_else(|error| error.into_inner()).error.clone()
    }

    pub fn get_state(&self) -> AppStateEnum {
        self.current_state.lock().map(|s| s.clone()).unwrap_or(AppStateEnum::Idle)
    }
}
