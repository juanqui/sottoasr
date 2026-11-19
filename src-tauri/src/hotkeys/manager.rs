use std::sync::atomic::Ordering;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_nspanel::{tauri_panel, ManagerExt, WebviewWindowExt as _};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use crate::audio::cue::{self, Cue};
use crate::models::{AppStateEnum, LlmCleanupStatus, Settings};
use crate::pipeline::{Failure, FailureKind, Linger, Outcome, PipelineEvents, StopJob};
use crate::state::{AppState, OverlaySession};

// Ordinary recording stays non-activating. Recovery controls can temporarily
// accept keyboard focus without turning the app into a regular Dock app.
pub(crate) static OVERLAY_ACCEPTS_KEY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
tauri_panel! {
    panel!(OverlayPanel {
        config: {
            can_become_key_window: crate::hotkeys::manager::OVERLAY_ACCEPTS_KEY.load(std::sync::atomic::Ordering::SeqCst),
            is_floating_panel: true
        }
    })
}

/// Reasons are logged at startup, so they never name the keys.
const REGISTER_FAILED: &str = "another app may be using it";
const PTT_UNSUPPORTED: &str = "push-to-talk cannot watch that key on this Mac";

/// Resolve parser aliases to the same physical key used for release polling.
/// Unsupported keys must fail validation before a recording can become stuck.
pub(crate) fn ptt_virtual_key(shortcut: &str) -> Result<u16, String> {
    let parsed = shortcut.parse::<Shortcut>()
        .map_err(|error| format!("Push-to-talk shortcut is invalid: {error}"))?;
    ptt_key(&parsed)
}

fn ptt_key(shortcut: &Shortcut) -> Result<u16, String> {
    crate::commands::keycapture::tauri_key_to_vk(&shortcut.key.to_string())
        .ok_or_else(|| format!("Push-to-talk does not support the {} key on this Mac", shortcut.key))
}

/// What a configured shortcut does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShortcutRole {
    PushToTalk,
    Toggle,
    /// Remembered at registration, active only while recording.
    Cancel,
    OpenSettings,
}

/// One configured shortcut and whether it can be registered.
#[derive(Debug)]
pub(crate) struct PlannedShortcut {
    pub label: &'static str,
    pub role: ShortcutRole,
    /// The parsed shortcut, or why it is skipped.
    pub shortcut: Result<Shortcut, String>,
}

/// Decide for each saved shortcut whether it can be registered. An empty
/// required shortcut, an unreadable one, an unsupported push-to-talk key and
/// a duplicate of an earlier entry are skipped with a reason; the others stay.
pub(crate) fn plan_shortcuts(settings: &Settings) -> Vec<PlannedShortcut> {
    use std::collections::hash_map::Entry;
    let configured = [
        ("Push-to-talk", ShortcutRole::PushToTalk, Some(settings.push_to_talk_shortcut.as_str()), true),
        ("Alternate push-to-talk", ShortcutRole::PushToTalk, settings.push_to_talk_shortcut_alt.as_deref(), false),
        ("Toggle recording", ShortcutRole::Toggle, Some(settings.toggle_shortcut.as_str()), true),
        ("Alternate toggle", ShortcutRole::Toggle, settings.toggle_shortcut_alt.as_deref(), false),
        ("Cancel recording", ShortcutRole::Cancel, Some(settings.cancel_shortcut.as_str()), true),
        ("Alternate cancel", ShortcutRole::Cancel, settings.cancel_shortcut_alt.as_deref(), false),
        ("Open Settings", ShortcutRole::OpenSettings, Some(settings.open_settings_shortcut.as_str()), false),
    ];
    let mut seen = std::collections::HashMap::new();
    let mut plan = Vec::with_capacity(configured.len());
    for (label, role, text, required) in configured {
        let text = text.map(str::trim).unwrap_or_default();
        if text.is_empty() {
            if required {
                plan.push(PlannedShortcut { label, role, shortcut: Err("empty".into()) });
            }
            continue;
        }
        let shortcut = parse_planned(text, role).and_then(|shortcut| match seen.entry(shortcut) {
            Entry::Occupied(previous) => Err(format!("same as {}", previous.get())),
            Entry::Vacant(slot) => {
                slot.insert(label);
                Ok(shortcut)
            }
        });
        plan.push(PlannedShortcut { label, role, shortcut });
    }
    plan
}

fn parse_planned(text: &str, role: ShortcutRole) -> Result<Shortcut, String> {
    let shortcut = text.parse::<Shortcut>().map_err(|_| "cannot be read".to_string())?;
    if role == ShortcutRole::PushToTalk && ptt_key(&shortcut).is_err() {
        return Err(PTT_UNSUPPORTED.into());
    }
    Ok(shortcut)
}

#[cfg(test)]
mod shortcut_tests {
    use super::{apply_shortcut_plan, plan_shortcuts, ptt_virtual_key, ShortcutRole, REGISTER_FAILED};

    #[test]
    fn parsed_aliases_share_the_physical_release_key_and_unsupported_keys_fail() {
        for shortcut in ["Ctrl+A", "Control+KeyA", "ctrl+a"] {
            assert_eq!(ptt_virtual_key(shortcut).unwrap(), 0x00);
        }
        assert_eq!(ptt_virtual_key("Ctrl+Space").unwrap(), 0x31);
        assert!(ptt_virtual_key("Ctrl+F24").is_err());
        let settings = crate::models::Settings {
            push_to_talk_shortcut: "Ctrl+F24".into(), ..Default::default()
        };
        assert!(settings.validate().is_err());
    }

    #[test]
    fn lenient_plan_registers_valid_shortcuts_beside_invalid_ones() {
        let settings = crate::models::Settings {
            toggle_shortcut: "Ctrl+NotAKey".into(),
            cancel_shortcut: "".into(),
            // Same physical shortcut as the default push-to-talk.
            toggle_shortcut_alt: Some("CmdOrCtrl+Shift+Space".into()),
            push_to_talk_shortcut_alt: Some("Ctrl+F24".into()),
            ..Default::default()
        };
        // Strict validation would have refused every shortcut at startup.
        assert!(settings.validate().is_err());
        let plan = plan_shortcuts(&settings);
        let usable = |label: &str| plan.iter().find(|entry| entry.label == label).map(|entry| entry.shortcut.is_ok());
        assert_eq!(usable("Push-to-talk"), Some(true));
        assert_eq!(usable("Open Settings"), Some(true));
        assert_eq!(usable("Toggle recording"), Some(false));
        assert_eq!(usable("Cancel recording"), Some(false));
        assert_eq!(usable("Alternate toggle"), Some(false));
        assert_eq!(usable("Alternate push-to-talk"), Some(false));
        assert_eq!(usable("Alternate cancel"), None, "an unset alternate is not reported");
        let open_settings = plan.iter().find(|entry| entry.label == "Open Settings").unwrap();
        assert_eq!(open_settings.role, ShortcutRole::OpenSettings);
        // Reasons are logged, so they must not repeat the configured keys.
        for entry in &plan {
            if let Err(reason) = &entry.shortcut {
                assert!(!reason.contains("NotAKey") && !reason.contains("F24"), "{reason}");
            }
        }
    }

    #[test]
    fn one_failed_registration_does_not_stop_the_rest_and_cancel_shortcuts_are_kept() {
        let settings = crate::models::Settings {
            cancel_shortcut_alt: Some("Alt+Escape".into()),
            ..Default::default()
        };
        let cancel = std::sync::Mutex::new(Vec::new());
        let mut registered = Vec::new();
        let skipped = apply_shortcut_plan(plan_shortcuts(&settings), &cancel, |role, _shortcut| {
            assert_ne!(role, ShortcutRole::Cancel, "cancel shortcuts register only while recording");
            if role == ShortcutRole::Toggle {
                return Err(REGISTER_FAILED.into());
            }
            registered.push(role);
            Ok(())
        });
        assert_eq!(skipped, vec![format!("Toggle recording ({REGISTER_FAILED})")]);
        assert_eq!(registered, vec![ShortcutRole::PushToTalk, ShortcutRole::OpenSettings]);
        assert_eq!(cancel.lock().unwrap().len(), 2);
    }
}

/// Register `plan` through `register`, which receives every entry except the
/// cancel shortcuts; those are stored in `cancel` and registered only while
/// recording. One failure does not stop the rest. Returns one message per
/// shortcut that is not active.
fn apply_shortcut_plan(
    plan: Vec<PlannedShortcut>,
    cancel: &std::sync::Mutex<Vec<Shortcut>>,
    mut register: impl FnMut(ShortcutRole, Shortcut) -> Result<(), String>,
) -> Vec<String> {
    let mut cancel_shortcuts = Vec::new();
    let mut skipped = Vec::new();
    for entry in plan {
        let registered = entry.shortcut.and_then(|shortcut| match entry.role {
            ShortcutRole::Cancel => {
                cancel_shortcuts.push(shortcut);
                Ok(())
            }
            role => register(role, shortcut),
        });
        if let Err(reason) = registered {
            skipped.push(format!("{} ({reason})", entry.label));
        }
    }
    *cancel.lock().unwrap_or_else(|error| error.into_inner()) = cancel_shortcuts;
    skipped
}

/// Replace every global registration with `plan`. The error says whether the
/// other shortcuts were registered or none were, so callers can report a
/// rollback truthfully.
fn register_plan(app: &AppHandle, plan: Vec<PlannedShortcut>) -> Result<(), String> {
    app.global_shortcut().unregister_all()
        .map_err(|_| "no shortcut was registered: the existing ones could not be cleared".to_string())?;
    let state = app.state::<AppState>();
    let skipped = apply_shortcut_plan(plan, &state.cancel_shortcuts, |role, shortcut| match role {
        ShortcutRole::PushToTalk => match ptt_key(&shortcut) {
            Ok(vk) => register_ptt(app, shortcut, vk),
            Err(_) => Err(PTT_UNSUPPORTED.into()),
        },
        ShortcutRole::Toggle => register_toggle(app, shortcut),
        ShortcutRole::OpenSettings => register_open_settings(app, shortcut),
        // Kept by apply_shortcut_plan for the recording; never registered here.
        ShortcutRole::Cancel => Ok(()),
    });
    if skipped.is_empty() {
        log::info!("Keyboard shortcuts registered");
        Ok(())
    } else {
        Err(format!("every other shortcut was registered; skipped: {}", skipped.join("; ")))
    }
}

/// Startup registration from the in-memory settings, which are the defaults
/// when the saved file could not be read. Each shortcut registers on its own
/// and settings are not validated, so one bad entry (or an invalid dictionary
/// row) cannot disable the other hotkeys. The error names the skipped
/// shortcuts. Runs on the main thread, outside the async runtime.
pub fn setup_hotkeys(app: &AppHandle) -> Result<(), String> {
    let settings = app.state::<AppState>().settings.blocking_lock().clone();
    register_plan(app, plan_shortcuts(&settings))
}

/// Re-register all shortcuts from `settings` (Settings save and its
/// rollback). Every usable shortcut is registered, so a rollback restores
/// what it can; the error names the ones that are not active.
/// The cancel shortcut is NOT registered globally here — it is only
/// registered while recording is active (see register_cancel_shortcut).
pub fn register_shortcuts(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    register_plan(app, plan_shortcuts(settings))
}

fn register_ptt(app: &AppHandle, shortcut: Shortcut, ptt_vk: u16) -> Result<(), String> {
    let app_handle = app.clone();
    app.global_shortcut().on_shortcut(shortcut, move |_app, _shortcut, event| {
        if event.state != ShortcutState::Pressed {
            return;
        }
        let app = app_handle.clone();
        tauri::async_runtime::spawn(async move {
            // Key repeat, or a second binding pressed during this recording.
            if app.state::<AppState>().get_state() == AppStateEnum::Recording {
                return;
            }
            let Ok(generation) = handle_start_recording(&app) else { return; };

            // Poll for key release via CGEventSourceKeyState
            #[cfg(target_os = "macos")]
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                unsafe {
                    extern "C" {
                        fn CGEventSourceKeyState(stateID: u32, key: u16) -> bool;
                    }
                    let deadline = std::time::Instant::now() + Duration::from_secs(crate::pipeline::MAX_RECORDING_SECS);
                    loop {
                        std::thread::sleep(Duration::from_millis(33));
                        let state: tauri::State<'_, AppState> = app.state();
                        if state.recording_generation.load(Ordering::SeqCst) != generation
                            || state.get_state() != AppStateEnum::Recording {
                            return;
                        }
                        if std::time::Instant::now() > deadline {
                            log::warn!("PTT key release polling reached the {}s recording limit", crate::pipeline::MAX_RECORDING_SECS);
                            break;
                        }
                        if !CGEventSourceKeyState(0, ptt_vk) {
                            break;
                        }
                    }
                }
                if app.state::<AppState>().get_state() == AppStateEnum::Recording {
                    tauri::async_runtime::spawn(async move {
                        handle_stop_recording(&app, Some(generation)).await;
                    });
                }
            });
        });
    }).map_err(|_| REGISTER_FAILED.to_string())
}

fn register_toggle(app: &AppHandle, shortcut: Shortcut) -> Result<(), String> {
    let app_handle = app.clone();
    app.global_shortcut().on_shortcut(shortcut, move |_app, _shortcut, event| {
        if event.state != ShortcutState::Pressed {
            return;
        }
        let app = app_handle.clone();
        tauri::async_runtime::spawn(async move {
            if app.state::<AppState>().get_state() == AppStateEnum::Recording {
                handle_stop_recording(&app, None).await;
            } else {
                // Idle starts; a dictation still processing reports busy.
                let _ = handle_start_recording(&app);
            }
        });
    }).map_err(|_| REGISTER_FAILED.to_string())
}

fn register_open_settings(app: &AppHandle, shortcut: Shortcut) -> Result<(), String> {
    let app_handle = app.clone();
    app.global_shortcut().on_shortcut(shortcut, move |_app, _shortcut, event| {
        if event.state == ShortcutState::Pressed {
            log::info!("Open-settings hotkey pressed");
            crate::tray::menu::open_or_focus_window(
                &app_handle,
                "settings",
                "settings.html",
                "SottoASR \u{2014} Settings",
                640.0,
                760.0,
            );
        }
    }).map_err(|_| REGISTER_FAILED.to_string())
}

/// Register the cancel shortcut(s) globally. Called when recording starts.
pub fn register_cancel_shortcut(app: &AppHandle) {
    let shortcuts = app.state::<AppState>().cancel_shortcuts.lock().unwrap_or_else(|error| error.into_inner()).clone();
    for shortcut in shortcuts {
        let app_handle = app.clone();
        let result = app.global_shortcut().on_shortcut(shortcut, move |_app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                let app = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    if app.state::<AppState>().get_state() == AppStateEnum::Recording {
                        handle_cancel_recording(&app).await;
                    }
                });
            }
        });
        if result.is_err() {
            log::error!("Could not register a cancel shortcut for this recording");
        }
    }
}

/// Unregister the cancel shortcut(s). Called when recording stops or is cancelled.
pub fn unregister_cancel_shortcut(app: &AppHandle) {
    let shortcuts = app.state::<AppState>().cancel_shortcuts.lock().unwrap_or_else(|error| error.into_inner()).clone();
    for shortcut in shortcuts {
        if app.global_shortcut().unregister(shortcut).is_err() {
            log::warn!("Could not unregister a cancel shortcut after recording");
        }
    }
}

/// Start recording from a shortcut or IPC request. While a dictation is still
/// processing, this reports busy instead.
pub fn handle_start_recording(app: &AppHandle) -> Result<u64, String> {
    let state = app.state::<AppState>();
    let level_app = app.clone();
    let (error_sender, mut error_receiver) = tokio::sync::mpsc::unbounded_channel();
    let generation = crate::pipeline::start(
        &state,
        &TauriEvents::new(app),
        Box::new(move |level| {
            let _ = level_app.emit("audio-level", serde_json::json!({ "level": level }));
        }),
        Box::new(move |generation, _error| {
            let _ = error_sender.send(generation);
        }),
    )?;

    // Publish the start events before consuming an early device error.
    // The callback only enqueues once. The stream's callbacks and the
    // recording writer share its sender, so the channel closes once Stop
    // drops the stream and the writer finishes; ordinary recordings do not
    // leave a waiting task.
    let app_for_error = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(generation) = error_receiver.recv().await {
            handle_stop_recording(&app_for_error, Some(generation)).await;
        }
    });

    let app_for_timer = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app_for_timer.state::<AppState>();
        let events = TauriEvents::new(&app_for_timer);
        if crate::pipeline::wait_for_recording_limit(&state, &events, generation).await {
            handle_stop_recording(&app_for_timer, Some(generation)).await;
        }
    });

    // Speculative cleanup-sidecar prewarm: page the swapped-out MLX weights
    // back in during the recording window, so the stop-path cleanup starts
    // hot. It is counted HERE, synchronously, so a stop that races this
    // spawn still finds the handoff gate; prewarm_sidecar releases it.
    crate::llm::cleanup::begin_prewarm(&state);
    let app_for_prewarm = app.clone();
    tauri::async_runtime::spawn(async move {
        crate::llm::cleanup::prewarm_sidecar(&app_for_prewarm.state::<AppState>()).await;
    });
    Ok(generation)
}

/// Stop the current recording, or only `generation` when given, then
/// transcribe and deliver in the background.
pub async fn handle_stop_recording(app: &AppHandle, generation: Option<u64>) {
    let state = app.state::<AppState>();
    let events = TauriEvents::new(app);
    if let Some(job) = crate::pipeline::claim_and_finish(&state, &events, generation).await {
        spawn_completion(app, job);
        log::info!("Recording stopped, transcription queued");
    }
}

/// Cancel recording: nothing is pasted; the transcript is saved as cancelled.
pub async fn handle_cancel_recording(app: &AppHandle) {
    let state = app.state::<AppState>();
    let events = TauriEvents::new(app);
    if let Some(job) = crate::pipeline::cancel(&state, &events).await {
        spawn_completion(app, job);
        log::info!("Recording cancelled, transcription queued");
    }
}

fn spawn_completion(app: &AppHandle, job: StopJob) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let events = TauriEvents::new(&app);
        crate::pipeline::complete(&state, &events, job).await;
    });
}

/// Production `PipelineEvents`: Tauri events, the overlay, sound cues and
/// the cancel shortcut. Event names and payloads are the ones the frontend reads.
pub(crate) struct TauriEvents {
    app: AppHandle,
}

impl TauriEvents {
    pub(crate) fn new(app: &AppHandle) -> Self {
        Self { app: app.clone() }
    }
}

fn failure_event(kind: FailureKind) -> &'static str {
    match kind {
        FailureKind::Recording => "recording-error",
        FailureKind::Transcription => "transcription-error",
        FailureKind::Paste => "paste-error",
        FailureKind::HistorySave => "history-save-error",
    }
}

/// How long the cleanup badge stays visible before the overlay hides.
/// Failures stay longer so the message can be read; outcomes with nothing
/// worth interrupting the flow for get no dwell.
fn badge_dwell(status: &LlmCleanupStatus) -> Duration {
    match status {
        LlmCleanupStatus::Suggested { .. } | LlmCleanupStatus::Applied { .. } => Duration::from_millis(800),
        LlmCleanupStatus::SkippedTooShort
        | LlmCleanupStatus::SkippedNoCandidates
        | LlmCleanupStatus::NoChanges
        | LlmCleanupStatus::Disabled
        | LlmCleanupStatus::Idle => Duration::ZERO,
        LlmCleanupStatus::Unavailable { .. }
        | LlmCleanupStatus::Failed { .. }
        | LlmCleanupStatus::Rejected { .. }
        | LlmCleanupStatus::TimedOut { .. } => Duration::from_millis(2000),
    }
}

/// What the overlay does when a dictation ends: `None` keeps it, because a
/// reported failure stays until the user dismisses it; otherwise it hides
/// after the returned dwell, which keeps the cleanup badge readable.
fn overlay_linger(outcome: &Outcome, show_overlay: bool) -> Option<Duration> {
    match outcome {
        Outcome::Delivered(status) if show_overlay => Some(badge_dwell(status)),
        Outcome::Delivered(_) | Outcome::Dismissed => Some(Duration::ZERO),
        Outcome::Failed | Outcome::Stale => None,
    }
}

#[cfg(test)]
mod adapter_tests {
    use super::{overlay_linger, LlmCleanupStatus, Outcome};
    use std::time::Duration;

    #[test]
    fn failures_stay_and_cleanup_badges_linger_before_the_hide() {
        let applied = Outcome::Delivered(LlmCleanupStatus::Applied { elapsed_ms: 5 });
        assert_eq!(overlay_linger(&applied, true), Some(Duration::from_millis(800)));
        let timed_out = Outcome::Delivered(LlmCleanupStatus::TimedOut { elapsed_ms: 5 });
        assert_eq!(overlay_linger(&timed_out, true), Some(Duration::from_millis(2000)));
        assert_eq!(overlay_linger(&Outcome::Delivered(LlmCleanupStatus::Disabled), true), Some(Duration::ZERO));
        assert_eq!(overlay_linger(&applied, false), Some(Duration::ZERO), "no badge without the pill");
        assert_eq!(overlay_linger(&Outcome::Dismissed, true), Some(Duration::ZERO));
        assert_eq!(overlay_linger(&Outcome::Failed, true), None);
        assert_eq!(overlay_linger(&Outcome::Stale, true), None);
    }
}

impl PipelineEvents for TauriEvents {
    fn phase(&self, state: AppStateEnum) {
        crate::commands::overlay::publish_state(&self.app, state);
    }

    fn busy(&self, state: AppStateEnum) {
        let _ = self.app.emit("hotkey-busy", serde_json::json!({ "state": state }));
        cue::play(&self.app, Cue::Busy);
    }

    fn recording_started(&self, generation: u64) {
        let _ = self.app.emit("recording-started", serde_json::json!({ "generation": generation }));
        register_cancel_shortcut(&self.app);
        show_overlay(&self.app);
        cue::play(&self.app, Cue::Start);
    }

    fn recording_stopped(&self, _generation: u64) {
        unregister_cancel_shortcut(&self.app);
        let _ = self.app.emit("recording-stopped", ());
        cue::play(&self.app, Cue::Stop);
    }

    fn recording_cancelled(&self, _generation: u64) {
        unregister_cancel_shortcut(&self.app);
        hide_overlay(&self.app);
        let _ = self.app.emit("recording-cancelled", ());
        cue::play(&self.app, Cue::Cancel);
    }

    fn time_warning(&self, remaining_secs: u64) {
        let _ = self.app.emit("recording-time-warning", serde_json::json!({ "remaining_secs": remaining_secs }));
    }

    fn failure(&self, failure: Failure) {
        let mut payload = serde_json::json!({ "error": failure.error });
        if let Some(path) = failure.audio_path {
            payload["audio_path"] = serde_json::json!(path);
        }
        if let Some(available) = failure.clipboard_available {
            payload["clipboard_available"] = serde_json::json!(available);
        }
        show_recording_error(&self.app, failure_event(failure.kind), failure.generation, payload);
        cue::play(&self.app, Cue::Error);
    }

    fn cleanup_status(&self, status: &LlmCleanupStatus) {
        let _ = self.app.emit("llm-cleanup-status", status);
    }

    fn paste_complete(&self, id: &str, clipboard_only: bool) {
        let payload = if clipboard_only {
            serde_json::json!({ "id": id, "clipboard_only": true })
        } else {
            serde_json::json!({ "id": id })
        };
        let _ = self.app.emit("paste-complete", payload);
    }

    fn transcription_saved(&self, transcription: &crate::models::Transcription, saved: &Result<Vec<String>, String>) {
        crate::commands::transcription::emit_transcription(&self.app, transcription, saved);
    }

    fn history_error(&self, error: &str) {
        let _ = self.app.emit("history-error", error);
    }

    fn recordings_changed(&self) {
        let _ = self.app.emit("recovery-recordings-changed", ());
    }

    fn finished(&self, generation: u64, outcome: Outcome) -> Linger<'_> {
        let show_overlay = self.app.state::<AppState>().show_overlay.load(Ordering::SeqCst);
        let linger = overlay_linger(&outcome, show_overlay);
        Box::pin(async move {
            let Some(dwell) = linger else { return };
            // The text is already delivered; this only delays the hide so
            // the cleanup badge can be read.
            if !dwell.is_zero() {
                tokio::time::sleep(dwell).await;
            }
            hide_overlay_for(&self.app, generation, None);
        })
    }
}

/// Logical dimensions of the overlay pill window.
const OVERLAY_WIDTH: f64 = 300.0;
const OVERLAY_HEIGHT: f64 = 110.0;

/// Sub-point tolerance for "did the user drag the overlay?" detection.
/// Hide-time frame positions are compared against the value we set at
/// show-time; anything above this epsilon counts as a user drag and gets
/// persisted. See docs/specs/2026-04-11-overlay-positioning-multi-monitor-fix.md §5.7.
const DRAG_EPSILON: f64 = 0.5;

/// Pre-create the overlay panel at startup so that the first recording
/// doesn't steal focus. WebviewWindowBuilder::build() activates the app
/// on macOS (Tauri bug #9065), but this only happens on initial creation.
/// By creating the panel early (hidden), the first recording can just
/// show the existing non-activating NSPanel.
pub fn precreate_overlay(app: &AppHandle) {
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        // Don't create if it already exists
        if app.get_webview_panel("overlay").is_ok() {
            return;
        }

        let window = match tauri::webview::WebviewWindowBuilder::new(
            &app,
            "overlay",
            tauri::WebviewUrl::App("overlay.html".into()),
        )
        .title("")
        .inner_size(OVERLAY_WIDTH, OVERLAY_HEIGHT)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .visible(false)
        .focused(false)
        // The card never becomes key on its own, so its first click must
        // reach the webview instead of only making the panel key.
        .accept_first_mouse(true)
        .build()
        {
            Ok(w) => w,
            Err(e) => {
                log::error!("Failed to pre-create overlay window: {}", e);
                return;
            }
        };

        match window.to_panel::<OverlayPanel>() {
            Ok(panel) => {
                configure_overlay_panel(&panel);

                // Park the hidden panel at a safe default (bottom-center
                // of the primary display). The first real show_overlay
                // will overwrite this, but parking the panel now
                // guarantees that if something skips positioning it
                // still appears on a sane screen. AppState.overlay_session
                // is intentionally left as None — precreation does not
                // open a user-visible session.
                let screens = get_native_screens();
                if let Some(primary) = screens.first() {
                    let _ = position_overlay_native(panel.as_panel(), primary);
                }

                // Do NOT show — leave hidden until first recording
                log::info!("Overlay panel pre-created (hidden)");
            }
            Err(e) => {
                log::error!("Failed to convert overlay to panel during pre-creation: {}", e);
            }
        }
    });
}

/// Show the floating overlay pill panel. Creates it if it doesn't exist.
/// Panel operations (show/hide/create) must run on the main thread for NSPanel.
///
/// Positioning uses native macOS APIs (NSScreen, CGWindowList, setFrameOrigin:)
/// to bypass Tauri's buggy multi-monitor coordinate handling.
///
/// Ordering matters: we position the panel *while it is hidden* and only
/// then call `panel.show()`. Calling `setFrameOrigin:` on a visible
/// NSPanel whose frame lives on a different NSScreen is a known-flaky
/// pattern — see
/// docs/specs/2026-04-11-overlay-positioning-multi-monitor-fix.md §3
/// Defect C. If the panel is already visible (second and later
/// recordings) we `orderOut:` it first, so the transport is invisible.
fn show_overlay(app: &AppHandle) {
    if app.state::<AppState>().show_overlay.load(Ordering::SeqCst) {
        show_overlay_panel(app, false);
    } else {
        hide_overlay(app);
    }
}

/// Shared NSPanel setup. The non-activating mask keeps a click on the overlay
/// from activating SottoASR, which would make it the next paste target. An
/// error card never takes keyboard focus by itself, so typing keeps going to
/// the user's app; `OVERLAY_ACCEPTS_KEY` lets a click on the card make the
/// panel key without activating the app.
fn configure_overlay_panel(panel: &tauri_nspanel::PanelHandle<tauri::Wry>) {
    use tauri_nspanel::objc2_app_kit::{NSWindowCollectionBehavior, NSWindowStyleMask};
    panel.set_transparent(true);
    panel.set_has_shadow(false);
    panel.set_hides_on_deactivate(false);
    panel.set_style_mask(panel.as_panel().styleMask() | NSWindowStyleMask::NonactivatingPanel);
    // Visible on all macOS Spaces (like Cmd+Shift+5 screenshot toolbar)
    panel.set_collection_behavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    // Synchronously clear backgrounds on ALL views in the hierarchy.
    clear_all_backgrounds(panel.as_panel());
}

fn show_recording_error(app: &AppHandle, event: &str, generation: u64, payload: serde_json::Value) {
    if crate::commands::overlay::publish_error(app, event, generation, payload) {
        show_error_overlay(app);
    }
}

/// Failures must remain accessible even when the normal recording pill is disabled.
pub fn show_error_overlay(app: &AppHandle) {
    show_overlay_panel(app, true);
}

fn show_overlay_panel(app: &AppHandle, interactive: bool) {
    let generation = app.state::<AppState>().recording_generation.load(Ordering::SeqCst);
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        let state = app.state::<AppState>();
        if state.recording_generation.load(Ordering::SeqCst) != generation { return; }
        if interactive && state.overlay_snapshot.lock().unwrap_or_else(|error| error.into_inner()).error.is_none() { return; }
        OVERLAY_ACCEPTS_KEY.store(interactive, Ordering::SeqCst);
        // Determine which screen to show the overlay on.
        // Uses the focused app's window first, then mouse cursor, then primary.
        let state = app.state::<AppState>();
        let target_pid = state.target_pid.load(Ordering::SeqCst);
        let target_screen = select_target_screen(target_pid);

        // Try to show an existing panel first
        if let Ok(panel) = app.get_webview_panel("overlay") {
            if !interactive { panel.resign_key_window(); }
            // 1. Hide first if currently visible so that setFrameOrigin
            //    does not have to perform a cross-display transport on a
            //    visible window.
            if panel_is_visible(panel.as_panel()) {
                unsafe {
                    let nil: Option<&tauri_nspanel::objc2_foundation::NSObject> = None;
                    let _: () = tauri_nspanel::objc2::msg_send![
                        panel.as_panel(), orderOut: nil
                    ];
                }
            }

            // 2. Position while hidden. Record the session state so that
            //    hide_overlay can tell user drags from auto-defaults.
            if let Some(ref screen) = target_screen {
                let session = position_overlay_native(panel.as_panel(), screen);
                *state.overlay_session.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(session);
            }

            // 3. Show.
            panel.show();

            // 4. Re-apply floating level — required to fix Tauri issue
            //    #13530 where the setting is lost after hide/show.
            use tauri_nspanel::PanelLevel;
            panel.set_level(PanelLevel::Floating.into());
            panel.set_floating_panel(true);
            panel.order_front_regardless();

            // 5. Verify the panel actually landed on the target screen;
            //    re-apply once if the window server moved it.
            if let Some(ref screen) = target_screen {
                if let Some(session) = *state
                    .overlay_session
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                {
                    verify_and_fix_overlay_frame(panel.as_panel(), screen, session);
                }
            }

            log::info!("Overlay shown (existing panel, floating level reapplied)");
            return;
        }

        // Fallback: check for a regular webview window (handles case where panel
        // conversion failed in the past and we have a plain window instead).
        if let Some(window) = app.get_webview_window("overlay") {
            let _ = window.show();
            // Never focus it: a key overlay would take the user's typing.
            // Best-effort positioning via Tauri for non-panel fallback
            log::info!("Overlay shown (existing window fallback)");
            return;
        }

        // Create the overlay as a regular WebviewWindow first (proven to work),
        // then convert it to an NSPanel for proper transparency.
        let window = match tauri::webview::WebviewWindowBuilder::new(
            &app,
            "overlay",
            tauri::WebviewUrl::App("overlay.html".into()),
        )
        .title("")
        .inner_size(OVERLAY_WIDTH, OVERLAY_HEIGHT)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .visible(false)
        .focused(false)
        .accept_first_mouse(true)
        .build()
        {
            Ok(w) => w,
            Err(e) => {
                log::error!("Failed to create overlay window: {}", e);
                return;
            }
        };

        // Convert to NSPanel for true transparency.
        match window.to_panel::<OverlayPanel>() {
            Ok(panel) => {
                configure_overlay_panel(&panel);

                // Position BEFORE showing; record session state.
                if let Some(ref screen) = target_screen {
                    let session = position_overlay_native(panel.as_panel(), screen);
                    *state.overlay_session.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some(session);
                }

                panel.show();

                // Verify post-show placement.
                if let Some(ref screen) = target_screen {
                    if let Some(session) = *state
                        .overlay_session
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                    {
                        verify_and_fix_overlay_frame(panel.as_panel(), screen, session);
                    }
                }

                log::info!("Overlay panel created and shown");
            }
            Err(e) => {
                log::error!("Failed to convert overlay to panel: {}, showing as window", e);
                let _ = window.show();
            }
        }
    });
}

/// Hide the overlay panel and reset its state for the next recording.
///
/// Persistence policy: we save the overlay's frame *only* if the user
/// actually dragged it away from the position we placed it at in
/// `show_overlay`. An auto-computed default that was never touched must
/// not be persisted, because the display arrangement might change before
/// the next show and the stored absolute coordinate would then correspond
/// to the wrong visual spot on the display it was keyed against.
/// See docs/specs/2026-04-11-overlay-positioning-multi-monitor-fix.md §5.7.
pub(crate) fn hide_overlay(app: &AppHandle) {
    hide_overlay_if_revision(app, None);
}

pub(crate) fn hide_overlay_if_revision(app: &AppHandle, revision: Option<u64>) {
    let generation = app.state::<AppState>().recording_generation.load(Ordering::SeqCst);
    hide_overlay_for(app, generation, revision);
}

/// Hide only while `generation` is still the current recording, so a late
/// hide cannot close a newer recording's overlay.
fn hide_overlay_for(app: &AppHandle, generation: u64, revision: Option<u64>) {
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        if !crate::commands::overlay::clear_overlay(&app, generation, revision) { return; }
        OVERLAY_ACCEPTS_KEY.store(false, Ordering::SeqCst);
        if let Ok(panel) = app.get_webview_panel("overlay") {
            panel.resign_key_window();
            let panel_ref = panel.as_panel();
            let frame: tauri_nspanel::objc2_foundation::NSRect = unsafe {
                tauri_nspanel::objc2::msg_send![panel_ref, frame]
            };

            let state = app.state::<AppState>();
            let session = state
                .overlay_session
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();

            if let Some(sess) = session {
                let dx = (frame.origin.x - sess.applied_origin.0).abs();
                let dy = (frame.origin.y - sess.applied_origin.1).abs();
                let moved = dx > DRAG_EPSILON || dy > DRAG_EPSILON;

                if moved {
                    // User dragged. Persist against whichever display
                    // now contains the overlay's center — matches the
                    // macOS "majority geometry" rule for spanning windows.
                    let center_x = frame.origin.x + frame.size.width / 2.0;
                    let center_y = frame.origin.y + frame.size.height / 2.0;
                    let screens = get_native_screens();
                    if let Some(idx) =
                        screen_containing_point(&screens, center_x, center_y)
                    {
                        save_panel_position(panel_ref, screens[idx].display_id);
                    } else {
                        log::info!(
                            "Overlay was dragged off all screens; cannot persist \
                             ({:.0},{:.0})",
                            frame.origin.x, frame.origin.y
                        );
                    }
                } else {
                    log::info!(
                        "Overlay was not moved during session — not persisting \
                         (default {:.0},{:.0} for display {})",
                        sess.default_origin.0, sess.default_origin.1, sess.display_id
                    );
                }
            } else {
                log::info!("No overlay session — skipping persistence on hide");
            }

            panel.hide();
            log::info!("Overlay hidden");
        } else if let Some(window) = app.get_webview_window("overlay") {
            let _ = window.hide();
            log::info!("Overlay hidden (window fallback)");
        }
    });
}

/// Synchronously clear background drawing on all views in the NSPanel hierarchy.
/// This walks the content view tree and sets every view + the WKWebView to transparent.
fn clear_all_backgrounds(panel: &tauri_nspanel::NSPanel) {
    use tauri_nspanel::{objc2, objc2_app_kit, objc2_foundation};

    unsafe {
        // 1. Clear the content view background
        let content_view: objc2::rc::Retained<objc2_app_kit::NSView> =
            objc2::msg_send![panel, contentView];
        let _: () = objc2::msg_send![&*content_view, setWantsLayer: true];
        if let Some(layer) = content_view.layer() {
            let clear: objc2::rc::Retained<objc2_foundation::NSObject> =
                objc2::msg_send![objc2::class!(NSColor), clearColor];
            let _: () = objc2::msg_send![&*layer, setBackgroundColor:
                { let cg: *const std::ffi::c_void = objc2::msg_send![&*clear, CGColor]; cg }];
        }

        // 2. Walk all subviews and clear backgrounds / find WKWebView
        let subviews: objc2::rc::Retained<objc2_foundation::NSArray<objc2_app_kit::NSView>> =
            objc2::msg_send![&*content_view, subviews];
        let count: usize = subviews.count();

        for i in 0..count {
            let view: objc2::rc::Retained<objc2_app_kit::NSView> =
                objc2::msg_send![&*subviews, objectAtIndex: i];

            // Check if this is a WKWebView by class name
            let cls: *const objc2::runtime::AnyClass = objc2::msg_send![&*view, class];
            let cls_name = std::ffi::CStr::from_ptr((*cls).name().to_bytes_with_nul().as_ptr() as *const _);
            let cls_str = cls_name.to_str().unwrap_or("");

            if cls_str.contains("WKWebView") || cls_str.contains("WebView") {
                // Set drawsBackground = false on the WKWebView
                let key = objc2_foundation::NSString::from_str("drawsBackground");
                let no: objc2::rc::Retained<objc2_foundation::NSNumber> =
                    objc2::msg_send![objc2::class!(NSNumber), numberWithBool: false];
                let _: () = objc2::msg_send![&*view, setValue: &*no, forKey: &*key];
                log::info!("Found {} — set drawsBackground=false", cls_str);
            }

            // Make every view non-opaque with clear layer background
            let _: () = objc2::msg_send![&*view, setWantsLayer: true];
            if let Some(layer) = view.layer() {
                let clear: objc2::rc::Retained<objc2_foundation::NSObject> =
                    objc2::msg_send![objc2::class!(NSColor), clearColor];
                let _: () = objc2::msg_send![&*layer, setBackgroundColor:
                    { let cg: *const std::ffi::c_void = objc2::msg_send![&*clear, CGColor]; cg }];
            }
        }
        log::info!("Cleared backgrounds on {} subviews", count);
    }
}

// ---------------------------------------------------------------------------
// Native multi-monitor overlay positioning
//
// Bypasses Tauri's buggy monitor/positioning APIs (see tauri#10980, #7890,
// #14825) by using macOS native APIs directly:
//   - NSScreen.screens        → screen enumeration
//   - CGWindowListCopyWindowInfo → focused-app window bounds
//   - NSEvent.mouseLocation   → cursor position
//   - NSPanel.setFrameOrigin: → window placement
//
// All coordinates stay in Cocoa's coordinate system (logical points, origin
// at bottom-left of primary display) to avoid cross-system conversion bugs.
//
// The overlay is draggable. User-chosen positions are remembered per monitor
// (keyed by CGDirectDisplayID) and restored on subsequent recordings.
// ---------------------------------------------------------------------------

/// Current persistence schema version. Bumped to 2 so that the loader can
/// reject any entry written by a pre-fix build — those entries were
/// auto-computed defaults that `hide_overlay` should never have persisted.
/// See docs/specs/2026-04-11-overlay-positioning-multi-monitor-fix.md §5.1.
const OVERLAY_POSITION_SCHEMA: u32 = 2;

/// Schema value inferred for entries that predate the schema field. Old
/// entries deserialize with this value and are rejected at load.
fn legacy_schema_version() -> u32 { 1 }

/// Saved overlay position for a specific monitor. Only user-dragged
/// positions are persisted; auto-computed defaults are not.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct SavedOverlayPosition {
    x: f64,
    y: f64,
    /// Schema version. Writers always set this to `OVERLAY_POSITION_SCHEMA`.
    #[serde(default = "legacy_schema_version")]
    schema: u32,
}

/// Map from display_id (as string) to saved position.
type OverlayPositions = std::collections::HashMap<String, SavedOverlayPosition>;

fn overlay_positions_path() -> Option<std::path::PathBuf> {
    let data_dir = dirs::data_dir()?;
    let app_dir = data_dir.join("com.sottoasr.app");
    std::fs::create_dir_all(&app_dir).ok()?;
    Some(app_dir.join("overlay_positions.json"))
}

fn load_overlay_positions() -> OverlayPositions {
    let path = match overlay_positions_path() {
        Some(p) if p.exists() => p,
        _ => return OverlayPositions::new(),
    };
    let mut positions: OverlayPositions = match std::fs::read_to_string(&path) {
        Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
        Err(_) => OverlayPositions::new(),
    };
    let before = positions.len();
    positions.retain(|_, v| v.schema >= OVERLAY_POSITION_SCHEMA);
    let dropped = before - positions.len();
    if dropped > 0 {
        log::info!(
            "Dropped {} legacy overlay-position entries (schema < {})",
            dropped, OVERLAY_POSITION_SCHEMA
        );
    }
    positions
}

fn save_overlay_positions(positions: &OverlayPositions) {
    if let Some(path) = overlay_positions_path() {
        if let Ok(data) = serde_json::to_string_pretty(positions) {
            let _ = std::fs::write(&path, data);
        }
    }
}

/// Save the current panel position for a specific display. This is the
/// *only* write site; every entry it creates carries
/// `OVERLAY_POSITION_SCHEMA` so that the loader accepts it.
fn save_panel_position(panel: &tauri_nspanel::objc2_app_kit::NSPanel, display_id: u32) {
    // Build-time guard: bumping the schema should force a loader update.
    debug_assert_eq!(
        OVERLAY_POSITION_SCHEMA, 2,
        "bump OVERLAY_POSITION_SCHEMA and extend the load filter together"
    );
    let frame: tauri_nspanel::objc2_foundation::NSRect = unsafe {
        tauri_nspanel::objc2::msg_send![panel, frame]
    };
    let mut positions = load_overlay_positions();
    positions.insert(display_id.to_string(), SavedOverlayPosition {
        x: frame.origin.x,
        y: frame.origin.y,
        schema: OVERLAY_POSITION_SCHEMA,
    });
    save_overlay_positions(&positions);
    log::info!(
        "Saved overlay position ({:.0}, {:.0}) for display {} (schema {})",
        frame.origin.x, frame.origin.y, display_id, OVERLAY_POSITION_SCHEMA
    );
}

/// Look up a saved position for this display. Returns `None` if the saved
/// point does not lie fully inside the current `visibleFrame` — i.e. the
/// display arrangement has changed since the position was persisted. A
/// saved entry that would need clamping is, by definition, stale; we
/// discard it rather than pin it to a visible edge.
fn get_saved_position(
    display_id: u32,
    visible: &tauri_nspanel::objc2_foundation::NSRect,
) -> Option<(f64, f64)> {
    let positions = load_overlay_positions();
    let saved = positions.get(&display_id.to_string())?;

    let fits_x = saved.x >= visible.origin.x
        && saved.x + OVERLAY_WIDTH <= visible.origin.x + visible.size.width;
    let fits_y = saved.y >= visible.origin.y
        && saved.y + OVERLAY_HEIGHT <= visible.origin.y + visible.size.height;

    if !fits_x || !fits_y {
        log::info!(
            "Discarding stale saved position ({:.0},{:.0}) for display {} — \
             does not fit current visibleFrame ({:.0},{:.0} {:.0}x{:.0})",
            saved.x, saved.y, display_id,
            visible.origin.x, visible.origin.y, visible.size.width, visible.size.height
        );
        return None;
    }

    log::info!(
        "Restored overlay position ({:.0},{:.0}) for display {}",
        saved.x, saved.y, display_id
    );
    Some((saved.x, saved.y))
}

/// Native screen info in Cocoa coordinates (logical points, origin bottom-left).
#[derive(Clone, Copy)]
struct NativeScreen {
    frame: tauri_nspanel::objc2_foundation::NSRect,
    visible_frame: tauri_nspanel::objc2_foundation::NSRect,
    scale_factor: f64,
    /// CGDirectDisplayID — unique, stable identifier for a physical monitor.
    display_id: u32,
}

/// Get all connected screens via [NSScreen screens].
/// Returns frames in Cocoa coordinates (points, origin at bottom-left of primary).
fn get_native_screens() -> Vec<NativeScreen> {
    unsafe {
        let screens: *const tauri_nspanel::objc2_foundation::NSArray<
            tauri_nspanel::objc2_app_kit::NSScreen,
        > = tauri_nspanel::objc2::msg_send![
            tauri_nspanel::objc2::class!(NSScreen),
            screens
        ];
        let count: usize = tauri_nspanel::objc2::msg_send![&*screens, count];
        let mut result = Vec::with_capacity(count);
        for i in 0..count {
            let screen: *const tauri_nspanel::objc2_app_kit::NSScreen =
                tauri_nspanel::objc2::msg_send![&*screens, objectAtIndex: i];
            let frame: tauri_nspanel::objc2_foundation::NSRect =
                tauri_nspanel::objc2::msg_send![screen, frame];
            let visible: tauri_nspanel::objc2_foundation::NSRect =
                tauri_nspanel::objc2::msg_send![screen, visibleFrame];
            let scale: f64 =
                tauri_nspanel::objc2::msg_send![screen, backingScaleFactor];
            // Extract CGDirectDisplayID from NSScreen.deviceDescription[@"NSScreenNumber"]
            let desc: *const tauri_nspanel::objc2_foundation::NSDictionary<
                tauri_nspanel::objc2_foundation::NSString,
                tauri_nspanel::objc2_foundation::NSObject,
            > = tauri_nspanel::objc2::msg_send![screen, deviceDescription];
            let key = tauri_nspanel::objc2_foundation::NSString::from_str("NSScreenNumber");
            let num_obj: *const tauri_nspanel::objc2_foundation::NSObject =
                tauri_nspanel::objc2::msg_send![&*desc, objectForKey: &*key];
            let display_id: u32 = if !num_obj.is_null() {
                tauri_nspanel::objc2::msg_send![num_obj, unsignedIntValue]
            } else {
                0
            };
            result.push(NativeScreen { frame, visible_frame: visible, scale_factor: scale, display_id });
        }
        result
    }
}

/// Get the mouse location in Cocoa coordinates (points, origin bottom-left).
fn get_mouse_location_cocoa() -> tauri_nspanel::objc2_foundation::NSPoint {
    unsafe {
        tauri_nspanel::objc2::msg_send![
            tauri_nspanel::objc2::class!(NSEvent),
            mouseLocation
        ]
    }
}

/// Get the bounds of the frontmost on-screen window belonging to `pid`.
/// Returns (x, y, width, height) in Quartz coordinates (origin top-left of primary).
fn get_frontmost_window_bounds(pid: i32) -> Option<(f64, f64, f64, f64)> {
    use core_graphics::window::*;
    use core_graphics::geometry::CGRect;
    use core_foundation::base::TCFType;

    extern "C" {
        fn CGRectMakeWithDictionaryRepresentation(
            dict: core_foundation::dictionary::CFDictionaryRef,
            rect: *mut CGRect,
        ) -> bool;
    }

    let windows = copy_window_info(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        kCGNullWindowID,
    )?;

    let arr_ref = windows.as_concrete_TypeRef();
    let len = unsafe { core_foundation::array::CFArrayGetCount(arr_ref) };

    for i in 0..len {
        unsafe {
            let dict_ptr = core_foundation::array::CFArrayGetValueAtIndex(arr_ref, i)
                as core_foundation::dictionary::CFDictionaryRef;
            if dict_ptr.is_null() { continue; }

            // Helper: read a numeric value from the window-info dictionary.
            let get_i32 = |key: core_foundation::string::CFStringRef| -> i32 {
                let mut value: *const std::ffi::c_void = std::ptr::null();
                if core_foundation::dictionary::CFDictionaryGetValueIfPresent(
                    dict_ptr, key as *const std::ffi::c_void, &mut value,
                ) != 0 && !value.is_null() {
                    let num = core_foundation::number::CFNumber::wrap_under_get_rule(
                        value as core_foundation::number::CFNumberRef,
                    );
                    num.to_i32().unwrap_or(0)
                } else {
                    0
                }
            };

            let owner_pid = get_i32(kCGWindowOwnerPID);
            let layer = get_i32(kCGWindowLayer);

            if owner_pid == pid && layer == 0 {
                let mut bounds_val: *const std::ffi::c_void = std::ptr::null();
                if core_foundation::dictionary::CFDictionaryGetValueIfPresent(
                    dict_ptr, kCGWindowBounds as *const std::ffi::c_void, &mut bounds_val,
                ) != 0 && !bounds_val.is_null() {
                    let mut rect = CGRect::new(
                        &core_graphics::geometry::CGPoint::new(0.0, 0.0),
                        &core_graphics::geometry::CGSize::new(0.0, 0.0),
                    );
                    if CGRectMakeWithDictionaryRepresentation(
                        bounds_val as core_foundation::dictionary::CFDictionaryRef,
                        &mut rect,
                    ) {
                        return Some((
                            rect.origin.x, rect.origin.y,
                            rect.size.width, rect.size.height,
                        ));
                    }
                }
            }
        }
    }
    None
}

/// Find which screen contains the given point (Cocoa coordinates).
fn screen_containing_point(screens: &[NativeScreen], x: f64, y: f64) -> Option<usize> {
    screens.iter().position(|s| {
        x >= s.frame.origin.x
            && x < s.frame.origin.x + s.frame.size.width
            && y >= s.frame.origin.y
            && y < s.frame.origin.y + s.frame.size.height
    })
}

/// Select the screen where the overlay should appear.
///
/// Priority: 1) screen with focused app's window  2) mouse cursor  3) primary
fn select_target_screen(target_pid: i32) -> Option<NativeScreen> {
    let screens = get_native_screens();
    if screens.is_empty() {
        log::error!("No screens detected — cannot position overlay");
        return None;
    }

    log_screen_configuration(&screens);

    // 1. Try the screen containing the focused app's frontmost window
    if target_pid > 0 {
        if let Some((bx, by, bw, bh)) = get_frontmost_window_bounds(target_pid) {
            let center_x = bx + bw / 2.0;
            let center_y_quartz = by + bh / 2.0;
            // Convert Quartz Y (top-left origin) → Cocoa Y (bottom-left origin)
            let primary_h = screens[0].frame.size.height;
            let center_y_cocoa = primary_h - center_y_quartz;
            if let Some(idx) = screen_containing_point(&screens, center_x, center_y_cocoa) {
                log::info!(
                    "Overlay target: screen {} (focused app PID {}, window center {:.0},{:.0})",
                    idx, target_pid, center_x, center_y_cocoa
                );
                return Some(screens[idx]);
            }
            log::warn!(
                "Focused app PID {} window center ({:.0},{:.0}) not on any screen",
                target_pid, center_x, center_y_cocoa
            );
        } else {
            log::info!("No on-screen windows found for PID {}", target_pid);
        }
    }

    // 2. Fall back to the screen under the mouse cursor
    let mouse = get_mouse_location_cocoa();
    if let Some(idx) = screen_containing_point(&screens, mouse.x, mouse.y) {
        log::info!("Overlay target: screen {} (mouse cursor fallback at {:.0},{:.0})", idx, mouse.x, mouse.y);
        return Some(screens[idx]);
    }

    // 3. Last resort: primary screen (screens[0] = menu-bar screen)
    log::info!("Overlay target: primary screen (final fallback)");
    Some(screens[0])
}

/// Position the overlay on the target screen using native Cocoa APIs.
///
/// Always computes the default bottom-center position; if the user has a
/// valid saved position for this display, uses that instead. Returns an
/// `OverlaySession` describing both origins so the caller can record it
/// on `AppState.overlay_session` and later detect whether the user
/// dragged the panel.
fn position_overlay_native(
    panel: &tauri_nspanel::objc2_app_kit::NSPanel,
    target: &NativeScreen,
) -> OverlaySession {
    let vis = &target.visible_frame;
    let margin_bottom: f64 = 8.0;

    // Default: centered horizontally, just above the Dock.
    let default_origin = (
        vis.origin.x + (vis.size.width - OVERLAY_WIDTH) / 2.0,
        vis.origin.y + margin_bottom,
    );

    let (x, y) = match get_saved_position(target.display_id, vis) {
        Some(saved) => saved,
        None        => default_origin,
    };

    unsafe {
        let origin = tauri_nspanel::objc2_foundation::NSPoint { x, y };
        let _: () = tauri_nspanel::objc2::msg_send![panel, setFrameOrigin: origin];
    }

    log::info!(
        "Overlay positioned at ({:.0},{:.0}) on display {} — default=({:.0},{:.0}) visible=({:.0},{:.0} {:.0}x{:.0})",
        x, y, target.display_id,
        default_origin.0, default_origin.1,
        vis.origin.x, vis.origin.y, vis.size.width, vis.size.height,
    );

    OverlaySession {
        display_id: target.display_id,
        default_origin,
        applied_origin: (x, y),
    }
}

/// True if the NSPanel's `isVisible` flag is set.
fn panel_is_visible(panel: &tauri_nspanel::objc2_app_kit::NSPanel) -> bool {
    unsafe {
        let visible: tauri_nspanel::objc2::runtime::Bool =
            tauri_nspanel::objc2::msg_send![panel, isVisible];
        visible.as_bool()
    }
}

/// Read the panel's actual frame after show and check it lies on the
/// target screen. If not (can happen when the window server decides to
/// re-assign the window to the "majority" display), re-apply once.
///
/// Checked against `target.frame` (not `visible_frame`) on purpose: the
/// goal is "did the panel land on the correct *screen*", not "is every
/// pixel inside the Dock-excluded area". A user who later drags the
/// panel into the Dock zone should still have their position honored.
fn verify_and_fix_overlay_frame(
    panel: &tauri_nspanel::objc2_app_kit::NSPanel,
    target: &NativeScreen,
    session: OverlaySession,
) {
    let frame: tauri_nspanel::objc2_foundation::NSRect = unsafe {
        tauri_nspanel::objc2::msg_send![panel, frame]
    };
    let center_x = frame.origin.x + frame.size.width / 2.0;
    let center_y = frame.origin.y + frame.size.height / 2.0;

    let tf = &target.frame;
    let inside = center_x >= tf.origin.x
        && center_x <  tf.origin.x + tf.size.width
        && center_y >= tf.origin.y
        && center_y <  tf.origin.y + tf.size.height;

    if inside {
        return;
    }

    log::warn!(
        "Overlay frame landed off-target after show (center {:.0},{:.0}, \
         target display {} frame {:.0},{:.0} {:.0}x{:.0}) — re-applying",
        center_x, center_y, target.display_id,
        tf.origin.x, tf.origin.y, tf.size.width, tf.size.height,
    );

    // Re-apply using the session's applied_origin rather than recomputing
    // so that we do not "demote" a valid user position to the default.
    unsafe {
        let origin = tauri_nspanel::objc2_foundation::NSPoint {
            x: session.applied_origin.0,
            y: session.applied_origin.1,
        };
        let _: () = tauri_nspanel::objc2::msg_send![panel, setFrameOrigin: origin];
    }
}

/// Log the current screen configuration for diagnostics.
fn log_screen_configuration(screens: &[NativeScreen]) {
    log::info!("=== Screen Configuration ({} screens) ===", screens.len());
    for (i, s) in screens.iter().enumerate() {
        log::info!(
            "  Screen {}: display={} frame=({:.0},{:.0} {:.0}x{:.0}) visible=({:.0},{:.0} {:.0}x{:.0}) scale={}",
            i, s.display_id,
            s.frame.origin.x, s.frame.origin.y,
            s.frame.size.width, s.frame.size.height,
            s.visible_frame.origin.x, s.visible_frame.origin.y,
            s.visible_frame.size.width, s.visible_frame.size.height,
            s.scale_factor,
        );
    }
}
