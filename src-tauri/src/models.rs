use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcription {
    pub id: String,
    pub text: String,
    pub duration_ms: u64,
    pub created_at: DateTime<Utc>,
    pub word_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AppStateEnum {
    Idle,
    Recording,
    Transcribing,
    Pasting,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub push_to_talk_shortcut: String,
    pub toggle_shortcut: String,
    pub cancel_shortcut: String,
    pub show_overlay: bool,
    pub auto_paste: bool,
    pub restore_clipboard: bool,
    pub model_path: String,
    pub language: String,
    pub max_history: usize,
    pub launch_at_login: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            push_to_talk_shortcut: "CommandOrControl+Shift+Space".into(),
            toggle_shortcut: "CommandOrControl+Shift+D".into(),
            cancel_shortcut: "Escape".into(),
            show_overlay: true,
            auto_paste: true,
            restore_clipboard: true,
            model_path: String::new(),
            language: "auto".into(),
            max_history: 500,
            launch_at_login: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelStatus {
    pub downloaded: bool,
    pub loaded: bool,
    pub path: Option<String>,
    pub name: String,
    pub size_bytes: Option<u64>,
}
