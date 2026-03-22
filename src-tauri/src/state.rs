use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Mutex as StdMutex;
use tokio::sync::Mutex as TokioMutex;
use crate::asr::engine::AsrEngine;
use crate::audio::capture::AudioCapture;
use crate::llm::engine::LlmEngine;
use crate::models::{AppStateEnum, Settings, Transcription};

pub struct AppState {
    pub current_state: StdMutex<AppStateEnum>,
    pub settings: TokioMutex<Settings>,
    pub last_transcription: TokioMutex<Option<Transcription>>,
    pub is_recording: AtomicBool,
    pub is_model_loaded: AtomicBool,
    // Audio capture — managed by hotkey handlers
    pub audio_capture: StdMutex<AudioCapture>,
    // Audio buffer: samples sent via channel from cpal callback
    pub audio_sender: StdMutex<std::sync::mpsc::Sender<Vec<f32>>>,
    pub audio_receiver: StdMutex<std::sync::mpsc::Receiver<Vec<f32>>>,
    // ASR engine
    pub asr_engine: TokioMutex<Box<dyn AsrEngine>>,
    // LLM engine for transcript cleanup
    pub llm_engine: TokioMutex<Option<LlmEngine>>,
    // Monotonic job ID for stale-result prevention
    pub current_job_id: AtomicU64,
    // Cancel shortcut string — registered only while recording
    pub cancel_shortcut: StdMutex<String>,
    // Recording generation counter — incremented on each new recording so stale
    // auto-stop timers from previous sessions can detect they are obsolete.
    pub recording_generation: AtomicU64,
}

impl AppState {
    pub fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let settings = crate::commands::settings::load_persisted_settings();
        let cancel = settings.cancel_shortcut.clone();
        Self {
            current_state: StdMutex::new(AppStateEnum::Idle),
            settings: TokioMutex::new(settings),
            last_transcription: TokioMutex::new(None),
            is_recording: AtomicBool::new(false),
            is_model_loaded: AtomicBool::new(false),
            audio_capture: StdMutex::new(AudioCapture::new()),
            audio_sender: StdMutex::new(tx),
            audio_receiver: StdMutex::new(rx),
            asr_engine: TokioMutex::new(crate::asr::engine::create_engine()),
            llm_engine: TokioMutex::new(None),
            current_job_id: AtomicU64::new(0),
            cancel_shortcut: StdMutex::new(cancel),
            recording_generation: AtomicU64::new(0),
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
