use std::sync::atomic::AtomicBool;
use std::sync::Mutex as StdMutex;
use tokio::sync::Mutex as TokioMutex;
use crate::asr::engine::AsrEngine;
use crate::audio::capture::AudioCapture;
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
}

impl AppState {
    pub fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self {
            current_state: StdMutex::new(AppStateEnum::Idle),
            settings: TokioMutex::new(Settings::default()),
            last_transcription: TokioMutex::new(None),
            is_recording: AtomicBool::new(false),
            is_model_loaded: AtomicBool::new(false),
            audio_capture: StdMutex::new(AudioCapture::new()),
            audio_sender: StdMutex::new(tx),
            audio_receiver: StdMutex::new(rx),
            asr_engine: TokioMutex::new(crate::asr::engine::create_engine()),
        }
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
