use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

fn default_true() -> bool {
    true
}

fn default_open_settings_shortcut() -> String {
    "CommandOrControl+Shift+Comma".into()
}

fn default_llm_cleanup_status() -> LlmCleanupStatus {
    LlmCleanupStatus::Idle
}

/// Outcome of the LLM cleanup step for a single transcription.
///
/// Serialized as an externally-tagged enum so the frontend can discriminate on
/// `kind` and read the corresponding `detail` payload. See
/// `docs/specs/2026-04-11-llm-cleanup-reliability.md` §4.4.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum LlmCleanupStatus {
    /// Cleanup ran successfully. Payload is elapsed time in ms.
    Applied { elapsed_ms: u64 },
    /// Skipped because input was under 5 words.
    SkippedTooShort,
    /// Skipped because `llm_cleanup_enabled=false` in settings.
    Disabled,
    /// Sidecar could not be started or the model could not be loaded.
    Unavailable { reason: String },
    /// Sidecar responded with an error. Raw text was pasted.
    Failed { reason: String },
    /// Cleanup exceeded the outer timeout. The subprocess was killed.
    TimedOut { elapsed_ms: u64 },
    /// Default / no cleanup was attempted this recording.
    #[default]
    Idle,
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
    /// Detailed cleanup outcome (see `LlmCleanupStatus`). Older entries that
    /// lack this field deserialize as `Idle`.
    #[serde(default = "default_llm_cleanup_status")]
    pub llm_cleanup_status: LlmCleanupStatus,
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
    #[serde(default = "default_open_settings_shortcut")]
    pub open_settings_shortcut: String,
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
    #[serde(default = "default_true")]
    pub auto_check_updates: bool,
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
            open_settings_shortcut: default_open_settings_shortcut(),
            show_overlay: true,
            auto_paste: true,
            restore_clipboard: true,
            restore_focus_before_paste: true,
            model_path: String::new(),
            language: "auto".into(),
            max_history: 500,
            launch_at_login: false,
            llm_cleanup_enabled: false,
            auto_check_updates: true,
        }
    }
}

impl PartialEq for Settings {
    fn eq(&self, other: &Self) -> bool {
        self.push_to_talk_shortcut == other.push_to_talk_shortcut
            && self.push_to_talk_shortcut_alt == other.push_to_talk_shortcut_alt
            && self.toggle_shortcut == other.toggle_shortcut
            && self.toggle_shortcut_alt == other.toggle_shortcut_alt
            && self.cancel_shortcut == other.cancel_shortcut
            && self.cancel_shortcut_alt == other.cancel_shortcut_alt
            && self.open_settings_shortcut == other.open_settings_shortcut
            && self.show_overlay == other.show_overlay
            && self.auto_paste == other.auto_paste
            && self.restore_clipboard == other.restore_clipboard
            && self.restore_focus_before_paste == other.restore_focus_before_paste
            && self.model_path == other.model_path
            && self.language == other.language
            && self.max_history == other.max_history
            && self.launch_at_login == other.launch_at_login
            && self.llm_cleanup_enabled == other.llm_cleanup_enabled
            && self.auto_check_updates == other.auto_check_updates
    }
}

impl PartialEq for Transcription {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.text == other.text
            && self.duration_ms == other.duration_ms
            && self.created_at == other.created_at
            && self.word_count == other.word_count
            && self.cancelled == other.cancelled
            && self.raw_text == other.raw_text
            && self.llm_applied == other.llm_applied
            && self.llm_cleanup_status == other.llm_cleanup_status
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
    /// Most recent cleanup outcome — populated from `AppState.llm_last_status`.
    /// Frontend uses this to display a status badge in the settings panel.
    #[serde(default = "default_llm_cleanup_status")]
    pub last_cleanup_status: LlmCleanupStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelStatus {
    pub downloaded: bool,
    pub loaded: bool,
    pub path: Option<String>,
    pub name: String,
    pub size_bytes: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Settings::validate() ---

    #[test]
    fn validate_rejects_empty_push_to_talk() {
        let s = Settings { push_to_talk_shortcut: "".into(), ..Default::default() };
        assert!(s.validate().is_err());
    }

    #[test]
    fn validate_rejects_empty_toggle() {
        let s = Settings { toggle_shortcut: "  ".into(), ..Default::default() };
        assert!(s.validate().is_err());
    }

    #[test]
    fn validate_rejects_empty_cancel() {
        let s = Settings { cancel_shortcut: "".into(), ..Default::default() };
        assert!(s.validate().is_err());
    }

    #[test]
    fn validate_rejects_max_history_below_10() {
        let s = Settings { max_history: 9, ..Default::default() };
        assert!(s.validate().is_err());
    }

    #[test]
    fn validate_rejects_max_history_above_10000() {
        let s = Settings { max_history: 10_001, ..Default::default() };
        assert!(s.validate().is_err());
    }

    #[test]
    fn validate_accepts_max_history_boundary_10() {
        let s = Settings { max_history: 10, ..Default::default() };
        assert!(s.validate().is_ok());
    }

    #[test]
    fn validate_accepts_max_history_boundary_10000() {
        let s = Settings { max_history: 10_000, ..Default::default() };
        assert!(s.validate().is_ok());
    }

    #[test]
    fn validate_rejects_push_to_talk_equals_toggle() {
        let s = Settings {
            push_to_talk_shortcut: "Ctrl+A".into(),
            toggle_shortcut: "Ctrl+A".into(),
            ..Default::default()
        };
        assert!(s.validate().is_err());
    }

    #[test]
    fn validate_rejects_push_to_talk_equals_cancel() {
        let s = Settings {
            push_to_talk_shortcut: "Ctrl+B".into(),
            cancel_shortcut: "Ctrl+B".into(),
            ..Default::default()
        };
        assert!(s.validate().is_err());
    }

    #[test]
    fn validate_rejects_toggle_equals_cancel() {
        let s = Settings {
            toggle_shortcut: "Ctrl+C".into(),
            cancel_shortcut: "Ctrl+C".into(),
            ..Default::default()
        };
        assert!(s.validate().is_err());
    }

    #[test]
    fn validate_does_not_check_alt_shortcuts() {
        // Alt shortcuts are optional alternates — the validate() function
        // does not check them for conflicts.
        let defaults = Settings::default();
        let s = Settings {
            push_to_talk_shortcut_alt: Some(defaults.toggle_shortcut.clone()),
            ..defaults
        };
        assert!(s.validate().is_ok());
    }

    #[test]
    fn validate_accepts_valid_defaults() {
        let s = Settings::default();
        assert!(s.validate().is_ok());
    }

    // --- Settings::default() ---

    #[test]
    fn default_settings_values() {
        let s = Settings::default();
        assert_eq!(s.push_to_talk_shortcut, "CommandOrControl+Shift+Space");
        assert_eq!(s.toggle_shortcut, "CommandOrControl+Shift+D");
        assert_eq!(s.cancel_shortcut, "Escape");
        assert_eq!(s.open_settings_shortcut, "CommandOrControl+Shift+Comma");
        assert!(s.push_to_talk_shortcut_alt.is_none());
        assert!(s.toggle_shortcut_alt.is_none());
        assert!(s.cancel_shortcut_alt.is_none());
        assert!(s.show_overlay);
        assert!(s.auto_paste);
        assert!(s.restore_clipboard);
        assert!(s.restore_focus_before_paste);
        assert_eq!(s.model_path, "");
        assert_eq!(s.language, "auto");
        assert_eq!(s.max_history, 500);
        assert!(!s.launch_at_login);
        assert!(!s.llm_cleanup_enabled);
        assert!(s.auto_check_updates);
    }

    // --- Settings serde round-trip ---

    #[test]
    fn settings_serde_round_trip() {
        let original = Settings::default();
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(original, deserialized);
    }

    #[test]
    fn settings_serde_round_trip_with_custom_values() {
        let original = Settings {
            push_to_talk_shortcut: "Ctrl+Space".into(),
            push_to_talk_shortcut_alt: Some("Alt+Space".into()),
            max_history: 42,
            llm_cleanup_enabled: true,
            auto_check_updates: false,
            ..Default::default()
        };
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(original, deserialized);
    }

    // --- Settings backward compat ---

    #[test]
    fn settings_backward_compat_missing_fields_use_defaults() {
        // Simulate a JSON from an older version that lacks newer fields
        let json = r#"{
            "push_to_talk_shortcut": "Ctrl+Space",
            "toggle_shortcut": "Ctrl+D",
            "cancel_shortcut": "Escape",
            "show_overlay": true,
            "auto_paste": true,
            "restore_clipboard": true,
            "model_path": "",
            "language": "auto",
            "max_history": 200,
            "launch_at_login": false
        }"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        // Fields with serde defaults should be populated
        assert!(s.restore_focus_before_paste); // default_true
        assert!(!s.llm_cleanup_enabled); // default false
        assert!(s.auto_check_updates); // default_true
        assert_eq!(s.open_settings_shortcut, "CommandOrControl+Shift+Comma"); // default_open_settings_shortcut
        assert!(s.push_to_talk_shortcut_alt.is_none());
        assert!(s.toggle_shortcut_alt.is_none());
        assert!(s.cancel_shortcut_alt.is_none());
    }

    // --- AppStateEnum serde ---

    #[test]
    fn app_state_enum_serde_round_trip() {
        let states = vec![
            AppStateEnum::Idle,
            AppStateEnum::Recording,
            AppStateEnum::Transcribing,
            AppStateEnum::CleaningUp,
            AppStateEnum::Pasting,
        ];
        for state in states {
            let json = serde_json::to_string(&state).unwrap();
            let deserialized: AppStateEnum = serde_json::from_str(&json).unwrap();
            assert_eq!(state, deserialized);
        }
    }

    #[test]
    fn app_state_enum_serializes_as_string() {
        let json = serde_json::to_string(&AppStateEnum::Recording).unwrap();
        assert_eq!(json, "\"Recording\"");
    }

    // --- Transcription serde ---

    #[test]
    fn transcription_serde_round_trip() {
        let t = Transcription {
            id: "abc-123".into(),
            text: "hello world".into(),
            duration_ms: 1500,
            created_at: Utc::now(),
            word_count: 2,
            cancelled: false,
            raw_text: Some("hello uh world".into()),
            llm_applied: true,
            llm_cleanup_status: LlmCleanupStatus::Applied { elapsed_ms: 1234 },
        };
        let json = serde_json::to_string(&t).unwrap();
        let deserialized: Transcription = serde_json::from_str(&json).unwrap();
        assert_eq!(t, deserialized);
    }

    #[test]
    fn transcription_serde_missing_optional_fields() {
        let json = r#"{
            "id": "test-1",
            "text": "hello",
            "duration_ms": 100,
            "created_at": "2024-01-01T00:00:00Z",
            "word_count": 1
        }"#;
        let t: Transcription = serde_json::from_str(json).unwrap();
        assert!(!t.cancelled);
        assert!(t.raw_text.is_none());
        assert!(!t.llm_applied);
        assert_eq!(t.llm_cleanup_status, LlmCleanupStatus::Idle);
    }

    #[test]
    fn transcription_skips_none_raw_text_in_serialization() {
        let t = Transcription {
            id: "test-2".into(),
            text: "hello".into(),
            duration_ms: 100,
            created_at: Utc::now(),
            word_count: 1,
            cancelled: false,
            raw_text: None,
            llm_applied: false,
            llm_cleanup_status: LlmCleanupStatus::Idle,
        };
        let json = serde_json::to_string(&t).unwrap();
        assert!(!json.contains("raw_text"));
    }

    #[test]
    fn llm_cleanup_status_serializes_applied_with_elapsed() {
        let s = LlmCleanupStatus::Applied { elapsed_ms: 1500 };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, r#"{"kind":"applied","detail":{"elapsed_ms":1500}}"#);
    }

    #[test]
    fn llm_cleanup_status_serializes_unit_variant_as_kind_only() {
        let s = LlmCleanupStatus::SkippedTooShort;
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, r#"{"kind":"skipped_too_short"}"#);
    }

    #[test]
    fn llm_cleanup_status_serializes_failed_with_reason() {
        let s = LlmCleanupStatus::Failed { reason: "broken pipe".into() };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains(r#""kind":"failed""#));
        assert!(json.contains(r#""reason":"broken pipe""#));
    }

    #[test]
    fn llm_cleanup_status_round_trip() {
        let statuses = vec![
            LlmCleanupStatus::Applied { elapsed_ms: 1500 },
            LlmCleanupStatus::SkippedTooShort,
            LlmCleanupStatus::Disabled,
            LlmCleanupStatus::Unavailable { reason: "no sidecar".into() },
            LlmCleanupStatus::Failed { reason: "timeout".into() },
            LlmCleanupStatus::TimedOut { elapsed_ms: 300_000 },
            LlmCleanupStatus::Idle,
        ];
        for s in statuses {
            let json = serde_json::to_string(&s).unwrap();
            let deserialized: LlmCleanupStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(s, deserialized);
        }
    }
}
