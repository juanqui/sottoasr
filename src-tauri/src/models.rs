use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcription {
    pub id: String,
    /// The final text (cleaned if LLM was used, raw otherwise)
    pub text: String,
    pub duration_ms: u64,
    pub created_at: DateTime<Utc>,
    pub word_count: usize,
    #[serde(default)]
    pub cancelled: bool,
    /// Raw ASR output before LLM cleanup (None if LLM was not used)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_text: Option<String>,
    /// Whether LLM cleanup was applied to this transcription
    #[serde(default)]
    pub llm_applied: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AppStateEnum {
    Idle,
    Recording,
    Transcribing,
    CleaningUp,
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
    #[serde(default)]
    pub llm_cleanup_enabled: bool,
    #[serde(default)]
    pub llm_markdown_mode: bool,
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
            llm_cleanup_enabled: false,
            llm_markdown_mode: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmStatus {
    pub available: bool,
    pub unavailable_reason: Option<String>,
    pub downloaded: bool,
    pub downloading: bool,
    pub loaded: bool,
    pub model_name: String,
    pub model_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelStatus {
    pub downloaded: bool,
    pub loaded: bool,
    pub path: Option<String>,
    pub name: String,
    pub size_bytes: Option<u64>,
}
