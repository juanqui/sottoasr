use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

fn default_true() -> bool {
    true
}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push_to_talk_shortcut_alt: Option<String>,
    pub toggle_shortcut: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toggle_shortcut_alt: Option<String>,
    pub cancel_shortcut: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel_shortcut_alt: Option<String>,
    pub show_overlay: bool,
    pub auto_paste: bool,
    pub restore_clipboard: bool,
    #[serde(default = "default_true")]
    pub restore_focus_before_paste: bool,
    pub model_path: String,
    pub language: String,
    pub max_history: usize,
    pub launch_at_login: bool,
    #[serde(default)]
    pub llm_cleanup_enabled: bool,
}

impl Settings {
    pub fn validate(&self) -> Result<(), String> {
        if self.push_to_talk_shortcut.trim().is_empty() {
            return Err("Push-to-talk shortcut cannot be empty".into());
        }
        if self.toggle_shortcut.trim().is_empty() {
            return Err("Toggle shortcut cannot be empty".into());
        }
        if self.cancel_shortcut.trim().is_empty() {
            return Err("Cancel shortcut cannot be empty".into());
        }
        if self.max_history < 10 || self.max_history > 10_000 {
            return Err("max_history must be between 10 and 10,000".into());
        }
        // Check for shortcut conflicts
        if self.push_to_talk_shortcut == self.toggle_shortcut {
            return Err("Push-to-talk and toggle shortcuts cannot be the same".into());
        }
        if self.push_to_talk_shortcut == self.cancel_shortcut {
            return Err("Push-to-talk and cancel shortcuts cannot be the same".into());
        }
        if self.toggle_shortcut == self.cancel_shortcut {
            return Err("Toggle and cancel shortcuts cannot be the same".into());
        }
        Ok(())
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            push_to_talk_shortcut: "CommandOrControl+Shift+Space".into(),
            push_to_talk_shortcut_alt: None,
            toggle_shortcut: "CommandOrControl+Shift+D".into(),
            toggle_shortcut_alt: None,
            cancel_shortcut: "Escape".into(),
            cancel_shortcut_alt: None,
            show_overlay: true,
            auto_paste: true,
            restore_clipboard: true,
            restore_focus_before_paste: true,
            model_path: String::new(),
            language: "auto".into(),
            max_history: 500,
            launch_at_login: false,
            llm_cleanup_enabled: false,
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
    #[serde(default)]
    pub update_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelStatus {
    pub downloaded: bool,
    pub loaded: bool,
    pub path: Option<String>,
    pub name: String,
    pub size_bytes: Option<u64>,
}
