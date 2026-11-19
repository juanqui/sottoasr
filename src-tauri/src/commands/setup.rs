use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Emitter, State};
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

/// Get the current ASR backend info.
#[tauri::command]
pub async fn get_asr_backend() -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "backend": model::backend_name(),
        "model_available": model::is_model_available(),
    }))
}

/// Get detailed model status.
#[tauri::command]
pub async fn get_model_status(state: State<'_, AppState>) -> Result<ModelStatus, String> {
    let mut status = model::get_model_status();
    status.loaded = state.is_model_loaded.load(std::sync::atomic::Ordering::SeqCst);
    status.initializing = state.asr_initializing.load(Ordering::SeqCst) || PREPARING.load(Ordering::SeqCst);
    status.error = state.asr_init_error.lock().unwrap_or_else(|e| e.into_inner()).clone();
    Ok(status)
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
            "backend": model::backend_name(),
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
/// FluidAudio it downloads missing files (~460 MB on first run) at the pinned
/// revision and verifies them, so a download never holds up transcription or
/// quitting. The engine lock covers only loading and Neural Engine compilation.
/// For parakeet-rs: loads ONNX model from disk.
#[tauri::command]
pub async fn init_asr(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    initialize(&state, &app, model::prepare_speech_model).await
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
    redownload(&state, &app, model::replace_failing_models).await
}

async fn initialize(
    state: &AppState,
    notices: &impl SetupNotices,
    prepare: impl FnOnce() -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    let _claim = PreparationClaim::take()?;
    begin(state, notices);
    let prepared = prepare_with_progress(notices, prepare).await;
    let result = load_prepared(state, prepared, |engine, ()| engine.init()).await;
    notices.finished(result)
}

async fn redownload(
    state: &AppState,
    notices: &impl SetupNotices,
    replace: impl FnOnce() -> Result<model::Replacement, String> + Send + 'static,
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
    let result = load_prepared(state, prepared, |engine, replacement: model::Replacement| {
        if replacement.speech { engine.reload() } else { engine.init() }
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
            log::info!("ASR engine ready: {}", model::backend_name());
            app.emit("asr-init-complete", serde_json::json!({
                "backend": model::backend_name(),
            })).map_err(|e| e.to_string())
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
    let backend = model::backend_name();

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
        model_available: model::is_model_available(),
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
        });
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
        let result = redownload(&state, &notices, || Err("Network unavailable.".into())).await;
        assert_eq!(result.unwrap_err(), "Network unavailable.");
        assert_eq!(*notices.finished.lock().unwrap(), [Err::<(), String>("Network unavailable.".to_owned())]);
        assert!(state.is_model_loaded.load(Ordering::SeqCst));
        assert!(!state.asr_initializing.load(Ordering::SeqCst));
        assert!(state.asr_init_error.lock().unwrap().is_none());
    }
}
