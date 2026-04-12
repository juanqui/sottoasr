use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64};
use std::sync::Mutex as StdMutex;
use tokio::sync::Mutex as TokioMutex;
use crate::asr::engine::AsrEngine;
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
    pub last_transcription: TokioMutex<Option<Transcription>>,
    pub is_recording: AtomicBool,
    pub is_model_loaded: AtomicBool,
    // Audio capture — managed by hotkey handlers
    pub audio_capture: StdMutex<Box<dyn AudioCaptureBackend>>,
    // Audio buffer: samples sent via channel from cpal callback
    pub audio_sender: StdMutex<std::sync::mpsc::Sender<Vec<f32>>>,
    pub audio_receiver: StdMutex<std::sync::mpsc::Receiver<Vec<f32>>>,
    // ASR engine
    pub asr_engine: TokioMutex<Box<dyn AsrEngine>>,
    // LLM engine for transcript cleanup
    pub llm_engine: TokioMutex<Option<Box<dyn LlmBackend>>>,
    // PID of the currently-running LLM sidecar subprocess, or 0 if none.
    // Captured from Child::id() at spawn time. Used by kill_orphan() to
    // SIGKILL the subprocess on timeout/panic without needing ownership of
    // the Child handle (which is held by the blocking cleanup task).
    // See docs/specs/2026-04-11-llm-cleanup-reliability.md §4.3.
    pub llm_pid: AtomicI32,
    // Most recent cleanup outcome. Read by the frontend via get_llm_status
    // and updated by run_cleanup() after every recording.
    pub llm_last_status: TokioMutex<LlmCleanupStatus>,
    // Paste backend — abstracts clipboard/paste operations
    pub paste_backend: Box<dyn PasteBackend>,
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
}

impl AppState {
    pub fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let settings = crate::commands::settings::load_persisted_settings();
        let cancel = settings.cancel_shortcut.clone();
        let cancel_alt = settings.cancel_shortcut_alt.clone();

        #[cfg(target_os = "macos")]
        let paste: Box<dyn PasteBackend> = Box::new(crate::paste::MacOsPasteBackend);
        #[cfg(not(target_os = "macos"))]
        let paste: Box<dyn PasteBackend> = Box::new(crate::paste::StubPasteBackend);

        Self {
            current_state: StdMutex::new(AppStateEnum::Idle),
            settings: TokioMutex::new(settings),
            last_transcription: TokioMutex::new(None),
            is_recording: AtomicBool::new(false),
            is_model_loaded: AtomicBool::new(false),
            audio_capture: StdMutex::new(Box::new(AudioCapture::new())),
            audio_sender: StdMutex::new(tx),
            audio_receiver: StdMutex::new(rx),
            asr_engine: TokioMutex::new(crate::asr::engine::create_engine()),
            llm_engine: TokioMutex::new(None),
            llm_pid: AtomicI32::new(0),
            llm_last_status: TokioMutex::new(LlmCleanupStatus::Idle),
            paste_backend: paste,
            current_job_id: AtomicU64::new(0),
            cancel_shortcut: StdMutex::new(cancel),
            cancel_shortcut_alt: StdMutex::new(cancel_alt),
            recording_generation: AtomicU64::new(0),
            target_pid: AtomicI32::new(0),
            overlay_session: StdMutex::new(None),
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
        let cancel = settings.cancel_shortcut.clone();
        let cancel_alt = settings.cancel_shortcut_alt.clone();
        Self {
            current_state: StdMutex::new(AppStateEnum::Idle),
            settings: TokioMutex::new(settings),
            last_transcription: TokioMutex::new(None),
            is_recording: AtomicBool::new(false),
            is_model_loaded: AtomicBool::new(true),
            audio_capture: StdMutex::new(audio),
            audio_sender: StdMutex::new(tx),
            audio_receiver: StdMutex::new(rx),
            asr_engine: TokioMutex::new(asr),
            llm_engine: TokioMutex::new(llm),
            llm_pid: AtomicI32::new(0),
            llm_last_status: TokioMutex::new(LlmCleanupStatus::Idle),
            paste_backend: paste,
            current_job_id: AtomicU64::new(0),
            cancel_shortcut: StdMutex::new(cancel),
            cancel_shortcut_alt: StdMutex::new(cancel_alt),
            recording_generation: AtomicU64::new(0),
            target_pid: AtomicI32::new(0),
            overlay_session: StdMutex::new(None),
        }
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

    pub fn set_state(&self, new_state: AppStateEnum) {
        if let Ok(mut state) = self.current_state.lock() {
            *state = new_state;
        }
    }

    pub fn get_state(&self) -> AppStateEnum {
        self.current_state.lock().map(|s| s.clone()).unwrap_or(AppStateEnum::Idle)
    }
}
