use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Emitter, Manager, State};
use crate::asr::engine::{with_engine, AsrEngine};
use crate::asr::model;
use crate::models::{AppStateEnum, ModelStatus};
use crate::state::AppState;

/// Set while a model preparation (download and verification) runs; only one
/// runs at a time. Separate from `asr_initializing`, which covers loading
/// only, so a loaded model keeps serving dictation while files download.
static PREPARING: AtomicBool = AtomicBool::new(false);

/// Holds the single preparation slot until dropped.
struct PreparationClaim;

impl PreparationClaim {
    fn take() -> Result<Self, String> {
        PREPARING.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map(|_| Self)
            .map_err(|_| "Speech recognition is already loading".to_owned())
    }
}

impl Drop for PreparationClaim {
    fn drop(&mut self) {
        PREPARING.store(false, Ordering::SeqCst);
    }
}

/// Set while the current speech model downloads behind a loaded previous one.
static UPGRADING: AtomicBool = AtomicBool::new(false);

/// Set while a confirmed deletion of the previous speech model holds the
/// preparation slot. Like the upgrade's download, it loads nothing.
static DELETING_PREVIOUS: AtomicBool = AtomicBool::new(false);

/// How long a finished background upgrade waits for a dictation to end
/// before it leaves the switch to the next start.
const UPGRADE_IDLE_WAIT: std::time::Duration = std::time::Duration::from_secs(600);

/// How loading moves between the current and the previous speech model. The
/// commands use `MODELS`; tests supply their own, so they read no model folder.
#[derive(Clone, Copy)]
struct Models {
    /// Makes the current model the one the engine loads next.
    use_current: fn(),
    /// After a failed load: selects the previous model when it can serve.
    fall_back: fn(&str) -> bool,
}

const MODELS: Models = Models {
    use_current: model::use_current_speech_model,
    fall_back: model::fall_back_to_previous_speech_model,
};

/// The name of the installed speech engine.
pub(crate) fn backend_name(state: &AppState) -> &'static str {
    if state.cloud_speech() { crate::asr::aws_transcribe::BACKEND_NAME } else { model::backend_name() }
}

/// Get the current ASR backend info. A cloud engine needs no model, so it
/// reports one as available and onboarding downloads nothing.
#[tauri::command]
pub async fn get_asr_backend(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "backend": backend_name(&state),
        "model_available": state.cloud_speech() || model::is_model_available(),
    }))
}

/// Get detailed model status.
#[tauri::command]
pub async fn get_model_status(state: State<'_, AppState>) -> Result<ModelStatus, String> {
    if state.cloud_speech() {
        let options = state.settings.lock().await.aws_transcribe.clone();
        return Ok(cloud_status(&options, crate::asr::aws_profiles::list().await));
    }
    let mut status = model::get_model_status();
    let upgrading = UPGRADING.load(Ordering::SeqCst);
    status.loaded = state.is_model_loaded.load(std::sync::atomic::Ordering::SeqCst);
    // A background upgrade and a deletion are not "loading": a model serves.
    status.initializing = state.asr_initializing.load(Ordering::SeqCst)
        || (PREPARING.load(Ordering::SeqCst) && !upgrading && !DELETING_PREVIOUS.load(Ordering::SeqCst));
    status.error = state.asr_init_error.lock().unwrap_or_else(|e| e.into_inner()).clone();
    status.notice = model::speech_notice(upgrading);
    // The previous model is unused only while the current one is in memory.
    if !status.loaded || status.initializing {
        status.previous_model = None;
    }
    Ok(status)
}

/// The status of a cloud engine: no model on disk or in memory matters. The
/// notice names where the audio goes; a profile that cannot be used is the
/// error.
fn cloud_status(
    options: &crate::models::AwsTranscribeSettings,
    profiles: Result<Vec<crate::asr::aws_profiles::AwsProfile>, String>,
) -> ModelStatus {
    let described = profiles.and_then(|profiles| crate::asr::aws_transcribe::describe(options, &profiles));
    let (notice, error) = match described {
        Ok(notice) => (Some(notice), None),
        Err(error) => (None, Some(error)),
    };
    ModelStatus {
        initializing: false,
        error,
        downloaded: true,
        loaded: true,
        path: None,
        name: crate::asr::aws_transcribe::BACKEND_NAME.into(),
        size_bytes: None,
        integrity_warning: None,
        notice,
        previous_model: None,
        engine: crate::models::SpeechEngine::AwsTranscribe,
    }
}

/// What a model operation tells the windows. The commands pass the app
/// handle; tests pass a silent stand-in and so run the commands' sequence.
trait SetupNotices: Sync {
    fn started(&self);
    /// Starts `model-download-progress` reports, aborted when preparation ends.
    fn download_progress(&self) -> Option<tauri::async_runtime::JoinHandle<()>>;
    fn finished(&self, result: Result<(), String>) -> Result<(), String>;
    fn vocabulary_replaced(&self);
}

impl SetupNotices for AppHandle {
    fn started(&self) {
        let _ = self.emit("asr-init-started", serde_json::json!({
            "backend": backend_name(&self.state::<AppState>()),
        }));
    }

    fn download_progress(&self) -> Option<tauri::async_runtime::JoinHandle<()>> {
        report_download_progress(self)
    }

    fn finished(&self, result: Result<(), String>) -> Result<(), String> {
        announce_result(self, result)
    }

    fn vocabulary_replaced(&self) {
        crate::commands::vocabulary::reload_cached(self.clone());
    }
}

/// Initialize the ASR engine.
/// Preparation runs first on a blocking worker without the engine lock. For
/// FluidAudio it chooses the speech model, downloads missing files (about
/// 630 MB on a new installation) at the pinned revision and verifies them, so
/// a download never holds up transcription or quitting. The engine lock
/// covers only loading and Neural Engine compilation. An installation that
/// still has only the previous speech model loads it at once; the current
/// model then follows in the background.
/// For parakeet-rs: loads ONNX model from disk.
/// For a cloud engine: nothing is prepared, downloaded or loaded.
#[tauri::command]
pub async fn init_asr(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    start(&state, &app, model::prepare_speech_model, MODELS).await?;
    if !state.cloud_speech() && model::upgrade_pending() {
        // Nobody waits for this: the previous model serves meanwhile.
        tauri::async_runtime::spawn(async move {
            let state: State<'_, AppState> = app.state();
            upgrade(&state, &app, model::prepare_current_speech_model, MODELS, UPGRADE_IDLE_WAIT).await;
        });
    }
    Ok(())
}

/// Makes the installed engine ready to transcribe. A cloud engine holds no
/// model: no model file is read and nothing is loaded.
async fn start(
    state: &AppState,
    notices: &impl SetupNotices,
    prepare: impl FnOnce() -> Result<(), String> + Send + 'static,
    models: Models,
) -> Result<(), String> {
    if state.cloud_speech() {
        state.is_model_loaded.store(true, Ordering::SeqCst);
        *state.asr_init_error.lock().unwrap_or_else(|e| e.into_inner()) = None;
        return notices.finished(Ok(()));
    }
    initialize(state, notices, prepare, models).await
}

/// User-confirmed replacement of the speech and vocabulary model files that
/// fail the manifest. The failing files are moved aside, downloaded again at
/// the pinned revision and verified; the moved copies are deleted only then
/// and are moved back on failure. Dictation keeps using the loaded model
/// until the engine loads the new files.
#[tauri::command]
pub async fn redownload_speech_model(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    redownload(&state, &app, model::replace_failing_models, MODELS).await
}

/// User-confirmed deletion of the previous speech model, which stays on disk
/// as a fallback after the switch to the current one. Returns the bytes
/// freed. It does not take the engine lock, so dictation is not held up.
#[tauri::command]
pub async fn delete_previous_speech_model(state: State<'_, AppState>) -> Result<u64, String> {
    delete_previous(&state, model::delete_previous_speech_model).await
}

async fn delete_previous(
    state: &AppState,
    delete: impl FnOnce() -> Result<u64, String> + Send + 'static,
) -> Result<u64, String> {
    // Loading, the background upgrade and the re-download hold this slot, and
    // only they select the previous model. None runs during the deletion.
    let claim = PreparationClaim::take()?;
    DELETING_PREVIOUS.store(true, Ordering::SeqCst);
    let deleted = if state.is_model_loaded.load(Ordering::SeqCst) && !state.asr_initializing.load(Ordering::SeqCst) {
        tokio::task::spawn_blocking(delete).await
            .map_err(|error| format!("Deleting the previous speech model stopped: {error}"))
            .and_then(|deleted| deleted)
    } else {
        Err("Speech recognition is not loaded. Load it, then delete the previous model.".into())
    };
    // The slot is free before the flag clears, so no status read sees "loading".
    drop(claim);
    DELETING_PREVIOUS.store(false, Ordering::SeqCst);
    deleted
}

async fn initialize(
    state: &AppState,
    notices: &impl SetupNotices,
    prepare: impl FnOnce() -> Result<(), String> + Send + 'static,
    models: Models,
) -> Result<(), String> {
    let _claim = PreparationClaim::take()?;
    begin(state, notices);
    let prepared = prepare_with_progress(notices, prepare).await;
    let result = load_prepared(state, prepared, move |engine, ()| {
        load_selected(engine, false, models.fall_back)
    }).await;
    notices.finished(result)
}

/// Downloads and verifies the current speech model while the previous one
/// keeps serving dictation, then loads it once no dictation is in progress.
/// A failed download changes nothing and reports nothing: the next
/// `init_asr` tries again.
async fn upgrade(
    state: &AppState,
    notices: &impl SetupNotices,
    prepare: impl FnOnce() -> Result<(), String> + Send + 'static,
    models: Models,
    idle_wait: std::time::Duration,
) {
    let Ok(_claim) = PreparationClaim::take() else { return };
    UPGRADING.store(true, Ordering::SeqCst);
    let prepared = tokio::task::spawn_blocking(prepare).await
        .map_err(|error| format!("Speech model preparation stopped: {error}"))
        .and_then(|prepared| prepared);
    let switch = match &prepared {
        Ok(()) => wait_until_idle(state, idle_wait).await,
        Err(error) => {
            log::warn!("The new speech model is not ready ({error}); the previous one stays in use");
            false
        }
    };
    UPGRADING.store(false, Ordering::SeqCst);
    if !switch {
        if prepared.is_ok() {
            log::info!("The new speech model is verified and loads at the next start");
        }
        return;
    }
    let result = load_prepared(state, prepared, move |engine, ()| {
        (models.use_current)();
        load_selected(engine, true, models.fall_back)
    }).await;
    let _ = notices.finished(result);
}

/// Waits for the app to be Idle, for at most `limit`, so that loading a new
/// model does not refuse a dictation that is in progress.
async fn wait_until_idle(state: &AppState, limit: std::time::Duration) -> bool {
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        if state.get_state() == AppStateEnum::Idle {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

/// Loads the speech model that preparation selected. When that fails and
/// `fall_back` selects the previous model, loads that instead: a model that
/// verifies but does not load must not stop dictation.
fn load_selected(engine: &mut dyn AsrEngine, replace: bool, fall_back: fn(&str) -> bool) -> Result<(), String> {
    let loaded = if replace { engine.reload() } else { engine.init() };
    match loaded {
        Err(error) if fall_back(&error) => engine.reload(),
        loaded => loaded,
    }
}

async fn redownload(
    state: &AppState,
    notices: &impl SetupNotices,
    replace: impl FnOnce() -> Result<model::Replacement, String> + Send + 'static,
    models: Models,
) -> Result<(), String> {
    if state.get_state() != AppStateEnum::Idle {
        return Err("Finish the current dictation, then re-download the speech model.".into());
    }
    let _claim = PreparationClaim::take()?;
    begin(state, notices);
    // Vocabulary preparation must not use its folder while files are moved aside.
    let vocabulary_operation = state.vocabulary_operation.lock().await;
    let mut prepared = prepare_with_progress(notices, replace).await;
    drop(vocabulary_operation);
    let (vocabulary_replaced, vocabulary_error) = match prepared.as_mut() {
        Ok(replacement) => (replacement.vocabulary, replacement.vocabulary_error.take()),
        Err(_) => (false, None),
    };
    let result = load_prepared(state, prepared, move |engine, replacement: model::Replacement| {
        load_selected(engine, replacement.speech, models.fall_back)
    }).await;
    let announced = notices.finished(result);
    if vocabulary_replaced {
        notices.vocabulary_replaced();
    }
    announced?;
    vocabulary_error.map_or(Ok(()), Err)
}

/// Starts an operation. It never marks the engine busy: only loading does.
fn begin(state: &AppState, notices: &impl SetupNotices) {
    *state.asr_init_error.lock().unwrap_or_else(|e| e.into_inner()) = None;
    notices.started();
}

/// Runs preparation on a blocking worker, without the engine lock, and
/// reports download progress meanwhile.
async fn prepare_with_progress<T: Send + 'static>(
    notices: &impl SetupNotices,
    operation: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let progress = notices.download_progress();
    let prepared = tokio::task::spawn_blocking(operation).await
        .map_err(|error| format!("Speech model preparation stopped: {error}"))
        .and_then(|prepared| prepared);
    if let Some(progress) = progress {
        progress.abort();
    }
    prepared
}

/// Emits `model-download-progress` every 500 ms until aborted.
fn report_download_progress(app: &AppHandle) -> Option<tauri::async_runtime::JoinHandle<()>> {
    #[cfg(feature = "asr-fluidaudio")]
    {
        let app = app.clone();
        Some(tauri::async_runtime::spawn(async move {
            let mut ticks = tokio::time::interval(std::time::Duration::from_millis(500));
            loop {
                ticks.tick().await;
                if let Some(progress) = model::speech_download_progress() {
                    let _ = app.emit("model-download-progress", progress);
                }
            }
        }))
    }

    #[cfg(not(feature = "asr-fluidaudio"))]
    {
        let _ = app;
        None
    }
}

/// Loads what preparation produced and settles the shared state. Only this
/// step sets `asr_initializing`, which refuses dictation. When preparation
/// failed, the engine is not touched and a loaded model stays in use with no
/// error reported.
async fn load_prepared<T: Send + 'static>(
    state: &AppState,
    prepared: Result<T, String>,
    load: impl FnOnce(&mut dyn AsrEngine, T) -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    let (result, attempted) = match prepared {
        Ok(value) => {
            state.asr_initializing.store(true, Ordering::SeqCst);
            (with_engine(&state.asr_engine, move |engine| load(engine, value)).await, true)
        }
        Err(error) => (Err(error), false),
    };
    let loaded = match &result {
        Ok(()) => true,
        Err(_) if attempted => false,
        Err(_) => state.is_model_loaded.load(Ordering::SeqCst),
    };
    state.is_model_loaded.store(loaded, Ordering::SeqCst);
    *state.asr_init_error.lock().unwrap_or_else(|e| e.into_inner()) =
        if loaded { None } else { result.as_ref().err().cloned() };
    // Readiness is published before the loading flag clears, so a dictation
    // that checks both never sees a finished load as not loaded.
    state.asr_initializing.store(false, Ordering::SeqCst);
    result
}

/// Tells the windows how the load ended.
fn announce_result(app: &AppHandle, result: Result<(), String>) -> Result<(), String> {
    match result {
        Ok(()) => {
            crate::commands::vocabulary::restore_cached(app.clone());
            let backend = backend_name(&app.state::<AppState>());
            log::info!("ASR engine ready: {backend}");
            app.emit("asr-init-complete", serde_json::json!({ "backend": backend })).map_err(|e| e.to_string())
        }
        Err(e) => {
            log::error!("ASR init failed: {}", e);
            let _ = app.emit("asr-init-error", serde_json::json!({ "error": &e }));
            Err(e)
        }
    }
}

/// Download model files (parakeet-rs backend only).
/// FluidAudio downloads during init_asr's preparation step.
#[tauri::command]
pub async fn download_model(app: AppHandle) -> Result<(), String> {
    #[cfg(feature = "asr-fluidaudio")]
    {
        log::info!("FluidAudio backend: model download is handled by init_asr()");
        let _ = app;
        Ok(())
    }

    #[cfg(all(feature = "asr-parakeet", not(feature = "asr-fluidaudio")))]
    {
        model::download_parakeet_model(app).await
    }

    #[cfg(not(any(feature = "asr-fluidaudio", feature = "asr-parakeet")))]
    {
        let _ = app;
        Err("No ASR backend enabled".into())
    }
}

/// Result of `complete_setup`, mirrored by `SetupResult` in `tauri.ts`.
#[derive(serde::Serialize)]
pub struct SetupResult {
    backend: &'static str,
    microphone_permission: bool,
    accessibility_permission: bool,
    asr_ready: bool,
    model_available: bool,
    /// The speech engine's own error text when `asr_ready` is false.
    error: Option<String>,
}

/// Complete onboarding: check permissions, init ASR, report status.
/// This is the main entry point called from the onboarding UI.
#[tauri::command]
pub async fn complete_setup(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<SetupResult, String> {
    let backend = backend_name(&state);
    let cloud = state.cloud_speech();

    app.emit("setup-progress", serde_json::json!({
        "step": "checking_permissions",
        "message": "Checking permissions...",
    })).map_err(|e| e.to_string())?;

    // Check permissions (non-blocking)
    let mic_ok = crate::commands::permissions::check_microphone_permission().await
        .unwrap_or(false);
    let ax_ok = crate::commands::permissions::check_accessibility_permission().await
        .unwrap_or(false);

    app.emit("setup-progress", serde_json::json!({
        "step": "initializing_asr",
        "message": format!("Loading {} models (this may take a minute)...", backend),
    })).map_err(|e| e.to_string())?;

    // Downloads (first run) and loads the speech model.
    let asr = init_asr(app.clone(), state).await;

    app.emit("setup-progress", serde_json::json!({
        "step": "complete",
        "message": "Setup complete!",
    })).map_err(|e| e.to_string())?;

    Ok(SetupResult {
        backend,
        microphone_permission: mic_ok,
        accessibility_permission: ax_ok,
        asr_ready: asr.is_ok(),
        model_available: cloud || model::is_model_available(),
        error: asr.err(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{MockAsrEngine, MockAudioCapture, MockPasteBackend};

    /// Stands in for the app handle and records what the windows would hear.
    #[derive(Default)]
    struct Quiet {
        finished: std::sync::Mutex<Vec<Result<(), String>>>,
    }

    impl SetupNotices for Quiet {
        fn started(&self) {}

        fn download_progress(&self) -> Option<tauri::async_runtime::JoinHandle<()>> {
            None
        }

        fn finished(&self, result: Result<(), String>) -> Result<(), String> {
            self.finished.lock().unwrap().push(result.clone());
            result
        }

        fn vocabulary_replaced(&self) {}
    }

    fn loaded_state(text: &str) -> AppState {
        AppState::new_with_backends(
            Box::new(MockAudioCapture::silence()),
            Box::new(MockAsrEngine::with_text(text)),
            None,
            Box::new(MockPasteBackend::new()),
            crate::models::Settings::default(),
        )
    }

    /// No previous speech model to fall back to, and nothing to select.
    const NO_PREVIOUS: Models = Models { use_current: || {}, fall_back: |_| false };

    /// An engine whose loads end as scripted, in order.
    struct Scripted {
        loads: std::collections::VecDeque<Result<(), String>>,
    }

    impl Scripted {
        fn new(loads: impl IntoIterator<Item = Result<(), String>>) -> Self {
            Self { loads: loads.into_iter().collect() }
        }
    }

    impl AsrEngine for Scripted {
        fn init(&mut self) -> Result<(), String> {
            self.loads.pop_front().expect("no load was expected")
        }
        fn is_ready(&self) -> bool { true }
        fn transcribe_file(&mut self, _: &str) -> Result<crate::asr::engine::AsrResult, String> {
            Err("not used".into())
        }
        fn transcribe_samples(&mut self, _: &[f32], _: u32) -> Result<crate::asr::engine::AsrResult, String> {
            Err("not used".into())
        }
        fn is_model_available(&self) -> bool { true }
        fn backend_name(&self) -> &'static str { "scripted" }
    }

    #[tokio::test(flavor = "current_thread")]
    #[serial_test::serial]
    async fn a_cloud_engine_is_ready_without_preparing_or_loading_a_model() {
        // An engine with no scripted load fails if anything loads it.
        let state = AppState::new_with_backends(
            Box::new(MockAudioCapture::silence()),
            Box::new(Scripted::new([])),
            None,
            Box::new(MockPasteBackend::new()),
            crate::models::Settings::default(),
        );
        state.cloud_speech.store(true, Ordering::SeqCst);
        state.is_model_loaded.store(false, Ordering::SeqCst);
        *state.asr_init_error.lock().unwrap() = Some("the local model failed to load earlier".into());
        let notices = Quiet::default();
        let never_selects = Models { use_current: || panic!("no model is selected"), fall_back: |_| panic!("no fallback") };

        let started = start(&state, &notices, || panic!("no model file may be read or downloaded"), never_selects).await;

        assert_eq!(started, Ok(()));
        assert!(state.is_model_loaded.load(Ordering::SeqCst), "dictation can start");
        assert!(!state.asr_initializing.load(Ordering::SeqCst));
        assert!(state.asr_init_error.lock().unwrap().is_none());
        assert_eq!(*notices.finished.lock().unwrap(), [Ok::<(), String>(())]);
        assert!(PreparationClaim::take().is_ok(), "nothing was prepared");
    }

    #[tokio::test(flavor = "current_thread")]
    #[serial_test::serial]
    async fn the_local_engine_still_prepares_and_loads() {
        let state = AppState::new_with_backends(
            Box::new(MockAudioCapture::silence()),
            Box::new(Scripted::new([Ok(())])),
            None,
            Box::new(MockPasteBackend::new()),
            crate::models::Settings::default(),
        );
        state.is_model_loaded.store(false, Ordering::SeqCst);
        let notices = Quiet::default();
        static PREPARED: AtomicBool = AtomicBool::new(false);
        PREPARED.store(false, Ordering::SeqCst);

        let started = start(&state, &notices, || { PREPARED.store(true, Ordering::SeqCst); Ok(()) }, NO_PREVIOUS).await;

        assert_eq!(started, Ok(()));
        assert!(PREPARED.load(Ordering::SeqCst));
        assert!(state.is_model_loaded.load(Ordering::SeqCst));
        assert!(state.asr_engine.lock().await.is_ready());
    }

    #[test]
    fn the_cloud_status_names_where_the_audio_goes_or_why_it_cannot_go() {
        use crate::asr::aws_profiles::AwsProfile;
        let options = crate::models::AwsTranscribeSettings { profile: Some("work".into()), ..Default::default() };
        let work = AwsProfile { name: "work".into(), region: Some("eu-west-1".into()) };

        let ready = cloud_status(&options, Ok(vec![work]));
        assert_eq!(ready.engine, crate::models::SpeechEngine::AwsTranscribe);
        assert_eq!(ready.name, "AWS Transcribe");
        assert!(ready.loaded && ready.downloaded && !ready.initializing);
        assert_eq!(
            ready.notice.as_deref(),
            Some("SottoASR sends each dictation to Amazon Transcribe in eu-west-1 with the AWS profile work.")
        );
        // Nothing of a local model shows, whatever is on disk.
        assert!(ready.error.is_none() && ready.path.is_none() && ready.size_bytes.is_none());
        assert!(ready.integrity_warning.is_none() && ready.previous_model.is_none());

        let removed = cloud_status(&options, Ok(Vec::new()));
        assert_eq!(removed.error.as_deref(), Some("AWS profile \"work\" was not found in ~/.aws/config or ~/.aws/credentials."));
        assert!(removed.notice.is_none() && removed.loaded);

        let unreadable = cloud_status(&options, Err("Could not read the AWS profiles: no access".into()));
        assert_eq!(unreadable.error.as_deref(), Some("Could not read the AWS profiles: no access"));
    }

    #[test]
    fn a_model_that_cannot_load_falls_back_to_the_previous_one() {
        // The current model fails, the previous one loads.
        let mut engine = Scripted::new([Err("CoreML refused the encoder".into()), Ok(())]);
        assert_eq!(load_selected(&mut engine, true, |_| true), Ok(()));
        assert!(engine.loads.is_empty(), "the previous model was loaded after the failure");

        // Without a previous model the failure is reported and nothing loads again.
        let mut engine = Scripted::new([Err("CoreML refused the encoder".into())]);
        assert_eq!(load_selected(&mut engine, false, |_| false), Err("CoreML refused the encoder".into()));

        // A load that succeeds never asks for a fallback.
        let mut engine = Scripted::new([Ok(())]);
        assert_eq!(load_selected(&mut engine, false, |_| panic!("no fallback after a successful load")), Ok(()));
    }

    #[tokio::test(flavor = "current_thread")]
    #[serial_test::serial]
    async fn background_upgrade_switches_to_the_current_model_once_it_is_verified() {
        static SELECTED: AtomicBool = AtomicBool::new(false);
        SELECTED.store(false, Ordering::SeqCst);
        let models = Models { use_current: || SELECTED.store(true, Ordering::SeqCst), fall_back: |_| false };
        let state = loaded_state("previous model");
        let notices = Quiet::default();
        let (downloading_tx, downloading) = tokio::sync::oneshot::channel();
        let (finish_tx, finish) = std::sync::mpsc::channel::<()>();
        let upgrading = upgrade(&state, &notices, move || {
            let _ = downloading_tx.send(());
            finish.recv_timeout(std::time::Duration::from_secs(2))
                .map_err(|_| "the test never finished the download".to_owned())
        }, models, std::time::Duration::ZERO);
        let dictation = async {
            downloading.await.unwrap();
            // The previous model serves, and Settings must not say "loading".
            assert!(UPGRADING.load(Ordering::SeqCst));
            assert!(!state.asr_initializing.load(Ordering::SeqCst));
            assert!(!SELECTED.load(Ordering::SeqCst), "the selection changes only when the engine switches");
            let result = crate::pipeline::transcribe_with_deadline(&state, "kept.wav".into(), Vec::new(), 1_000).await;
            finish_tx.send(()).unwrap();
            result
        };
        let ((), transcribed) = tokio::join!(upgrading, dictation);
        assert_eq!(transcribed.unwrap().text, "previous model");
        assert!(SELECTED.load(Ordering::SeqCst));
        assert_eq!(*notices.finished.lock().unwrap(), [Ok::<(), String>(())]);
        assert!(state.is_model_loaded.load(Ordering::SeqCst));
        assert!(!UPGRADING.load(Ordering::SeqCst));
        assert!(PreparationClaim::take().is_ok(), "the claim is released");
    }

    #[tokio::test(flavor = "current_thread")]
    #[serial_test::serial]
    async fn failed_or_untimely_upgrade_leaves_the_previous_model_in_use_and_reports_nothing() {
        let never_selects = Models { use_current: || panic!("the engine must not switch"), fall_back: |_| false };
        let state = loaded_state("unused");
        let notices = Quiet::default();

        // Offline: the download fails.
        upgrade(&state, &notices, || Err("Network unavailable.".into()), never_selects, std::time::Duration::ZERO).await;

        // Verified, but a dictation is in progress for the whole wait.
        state.set_state(AppStateEnum::Recording);
        upgrade(&state, &notices, || Ok(()), never_selects, std::time::Duration::ZERO).await;

        assert!(notices.finished.lock().unwrap().is_empty(), "no event reaches the windows");
        assert!(state.is_model_loaded.load(Ordering::SeqCst));
        assert!(!state.asr_initializing.load(Ordering::SeqCst));
        assert!(state.asr_init_error.lock().unwrap().is_none());
        assert!(!UPGRADING.load(Ordering::SeqCst));
        assert!(PreparationClaim::take().is_ok(), "the claim is released");
    }

    #[tokio::test(flavor = "current_thread")]
    #[serial_test::serial]
    async fn loaded_model_keeps_transcribing_while_a_replacement_downloads() {
        let state = loaded_state("still dictating");
        let notices = Quiet::default();
        let (downloading_tx, downloading) = tokio::sync::oneshot::channel();
        let (finish_tx, finish) = std::sync::mpsc::channel::<()>();
        // The command's own sequence, with the download blocked until released.
        let replacement = redownload(&state, &notices, move || {
            let _ = downloading_tx.send(());
            finish.recv_timeout(std::time::Duration::from_secs(2))
                .map_err(|_| "the test never finished the download".to_owned())?;
            Ok(model::Replacement { speech: true, vocabulary: false, vocabulary_error: None })
        }, NO_PREVIOUS);
        let dictation = async {
            downloading.await.unwrap();
            assert!(PreparationClaim::take().is_err(), "one preparation at a time");
            let result = crate::pipeline::transcribe_with_deadline(&state, "kept.wav".into(), Vec::new(), 1_000).await;
            finish_tx.send(()).unwrap();
            result
        };
        let (replaced, transcribed) = tokio::join!(replacement, dictation);
        assert_eq!(transcribed.unwrap().text, "still dictating");
        replaced.unwrap();
        assert_eq!(*notices.finished.lock().unwrap(), [Ok::<(), String>(())]);
        assert!(state.is_model_loaded.load(Ordering::SeqCst));
        assert!(!state.asr_initializing.load(Ordering::SeqCst));
        assert!(PreparationClaim::take().is_ok(), "the claim is released");
    }

    #[tokio::test(flavor = "current_thread")]
    #[serial_test::serial]
    async fn failed_replacement_keeps_the_loaded_model_in_use() {
        let state = loaded_state("unused");
        let notices = Quiet::default();
        let result = redownload(&state, &notices, || Err("Network unavailable.".into()), NO_PREVIOUS).await;
        assert_eq!(result.unwrap_err(), "Network unavailable.");
        assert_eq!(*notices.finished.lock().unwrap(), [Err::<(), String>("Network unavailable.".to_owned())]);
        assert!(state.is_model_loaded.load(Ordering::SeqCst));
        assert!(!state.asr_initializing.load(Ordering::SeqCst));
        assert!(state.asr_init_error.lock().unwrap().is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    #[serial_test::serial]
    async fn deleting_the_previous_model_needs_a_loaded_engine_and_excludes_loading() {
        let state = loaded_state("unused");
        let freed = delete_previous(&state, || {
            // A load that starts now could fall back to the model being deleted.
            PreparationClaim::take().map(|_| 0).or(Ok(7))
        }).await;
        assert_eq!(freed, Ok(7), "loading is refused during the deletion");
        assert!(!DELETING_PREVIOUS.load(Ordering::SeqCst));

        // While something else prepares or loads, nothing is deleted.
        let loading = PreparationClaim::take().unwrap();
        assert!(delete_previous(&state, || panic!("nothing may be deleted")).await.is_err());
        drop(loading);

        state.is_model_loaded.store(false, Ordering::SeqCst);
        let error = delete_previous(&state, || panic!("nothing may be deleted")).await.unwrap_err();
        assert!(error.contains("not loaded"), "{error}");
        assert!(PreparationClaim::take().is_ok(), "the claim is released");
    }
}
