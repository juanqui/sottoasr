use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Emitter, Manager, State};

use crate::llm::engine::{PipNetwork, RuntimeSetup, VenvVerdict};
use crate::llm::{download, engine};
use crate::models::LlmStatus;
use crate::state::AppState;

/// Status reads are offline and never import Python/MLX or begin setup.
#[tauri::command]
pub async fn get_llm_status(state: State<'_, AppState>) -> Result<LlmStatus, String> {
    current_status(&state).await
}

async fn current_status(state: &AppState) -> Result<LlmStatus, String> {
    let available = engine::is_feature_compiled() && engine::is_platform_supported();
    let downloaded = if available {
        tokio::task::spawn_blocking(engine::is_model_downloaded)
            .await
            .unwrap_or(false)
    } else {
        false
    };
    let config = engine::model_config();
    Ok(LlmStatus {
        available,
        unavailable_reason: (!available)
            .then(|| "Requires the cleanup feature on an Apple Silicon Mac".into()),
        downloaded,
        downloading: state.llm_downloading.load(Ordering::SeqCst),
        preparing: reports_preparing(state),
        setup_error: state.llm_setup_error.lock().await.clone(),
        loaded: state.llm_loaded.load(Ordering::SeqCst)
            && crate::process::registered_is_alive(state.llm_pid.load(Ordering::SeqCst)),
        enabled: state.settings.lock().await.llm_cleanup_enabled,
        busy: state.llm_operation.try_lock().is_err(),
        model_name: config.display_name.to_string(),
        model_url: format!("https://huggingface.co/{}", config.id),
        download_size_mb: config.download_size_mb,
        model_path: None,
        last_cleanup_status: state.llm_last_status.lock().await.clone(),
    })
}

/// Whether Settings shows a preparation. The background sync of a stale
/// runtime is not one: cleanup already works while it runs.
fn reports_preparing(state: &AppState) -> bool {
    let running = state.llm_preparing.load(Ordering::SeqCst);
    running == Preparation::Ensure as u8 || running == Preparation::Repair as u8
}

/// `AppState::llm_preparing` while no preparation runs. Otherwise it holds
/// the running `Preparation` as `u8`.
const IDLE: u8 = 0;

/// What an explicit preparation is allowed to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum Preparation {
    /// Create a missing runtime, download the model if it is missing, then
    /// verify loading. A stale runtime is synced afterwards in the
    /// background. An inconclusive probe stops here rather than escalating a
    /// probe that never answered into an install and a multi-gigabyte model
    /// download; a broken runtime needs Repair.
    Ensure = 1,
    /// Rebuild the runtime from scratch, then verify loading. This is the
    /// user's repair lever and is never gated on the probe it repairs.
    Repair = 2,
    /// Bring a stale runtime that cleanup already runs on to the lock. It
    /// holds its own claim so it never overlaps another preparation, and it
    /// is not reported as preparing.
    Sync = 3,
}

/// Explicit enable intent: install the runtime, download if missing, and verify
/// loading. Concurrent windows join this preparation instead of duplicating it.
#[tauri::command]
pub async fn prepare_llm_model(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<LlmStatus, String> {
    prepare(&app, &state, Preparation::Ensure).await
}

/// Explicit repair intent: rebuild the runtime from the lock, then reload.
///
/// The counterpart to `prepare_llm_model` for a broken runtime. It rebuilds
/// unconditionally, so a stale or inconclusive verdict can never block the one
/// action that can fix the runtime.
#[tauri::command]
pub async fn repair_llm_runtime(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<LlmStatus, String> {
    prepare(&app, &state, Preparation::Repair).await
}

async fn prepare(
    app: &AppHandle,
    state: &AppState,
    mode: Preparation,
) -> Result<LlmStatus, String> {
    if !engine::is_feature_compiled() || !engine::is_platform_supported() {
        return Err("Local cleanup requires an Apple Silicon Mac".into());
    }
    prepare_with(state, &AppHost(app), mode).await?;
    current_status(state).await
}

/// What a preparation needs from the running app. `AppHost` is the app;
/// tests script every step.
trait PreparationHost: Sync {
    type Runtime: RuntimeSetup;
    fn runtime(&self) -> Self::Runtime;
    /// Tell open windows that the preparation state changed.
    fn changed(&self);
    /// Download the model if it is missing. The caller holds `llm_operation`.
    fn ensure_model(&self) -> impl Future<Output = Result<(), String>> + Send;
    /// Start the sidecar on the installed runtime and check that the model
    /// loads. The caller holds `llm_operation`.
    fn load(&self, state: &AppState) -> impl Future<Output = Result<(), String>> + Send;
    /// Run `sync_stale_runtime` as its own task.
    fn start_stale_sync(&self);
}

/// Set once this launch has started its stale-runtime sync.
static STALE_SYNC_STARTED: AtomicBool = AtomicBool::new(false);

struct AppHost<'a>(&'a AppHandle);

impl PreparationHost for AppHost<'_> {
    type Runtime = engine::AppRuntime;

    fn runtime(&self) -> engine::AppRuntime {
        engine::AppRuntime
    }

    fn changed(&self) {
        let _ = self.0.emit("llm-preparation-changed", ());
    }

    async fn ensure_model(&self) -> Result<(), String> {
        let model_present = tokio::task::spawn_blocking(engine::is_model_downloaded)
            .await
            .unwrap_or(false);
        if model_present {
            return Ok(());
        }
        download::download_model(self.0).await
    }

    async fn load(&self, state: &AppState) -> Result<(), String> {
        load_locked(state).await
    }

    fn start_stale_sync(&self) {
        // At most one sync per launch: after a failure, for example offline,
        // the next launch tries again. Quitting kills its pip child with every
        // other owned process.
        if STALE_SYNC_STARTED.swap(true, Ordering::SeqCst) {
            return;
        }
        let app = self.0.clone();
        tauri::async_runtime::spawn(async move {
            let state = app.state::<AppState>();
            sync_stale_runtime(&state, &AppHost(&app)).await;
        });
    }
}

/// Claim, run and release one preparation of `mode`. Callers of the same
/// mode join it and see its error. A stale runtime's sync starts only after
/// the claim is released, so the caller gets its answer as soon as cleanup
/// is loaded.
async fn prepare_with<H: PreparationHost>(
    state: &AppState,
    host: &H,
    mode: Preparation,
) -> Result<(), String> {
    let Some(claim) = claim_preparation(state, mode).await else {
        return state.llm_setup_error.lock().await.clone().map_or(Ok(()), Err);
    };
    *state.llm_setup_error.lock().await = None;
    host.changed();
    let result = run_preparation(state, host, mode).await;
    *state.llm_setup_error.lock().await = result.as_ref().err().cloned();
    // Joiners read the error above once the claim wakes them.
    drop(claim);
    host.changed();
    if result? {
        host.start_stale_sync();
    }
    Ok(())
}

/// Returns whether cleanup is loaded on a stale runtime that still needs its
/// background sync.
async fn run_preparation<H: PreparationHost>(
    state: &AppState,
    host: &H,
    mode: Preparation,
) -> Result<bool, String> {
    let verdict = prepare_runtime(state, host, mode).await?;
    if mode == Preparation::Repair {
        return Ok(false);
    }
    let loaded = {
        let _operation = state.llm_operation.lock().await;
        host.ensure_model().await?;
        host.load(state).await
    };
    let VenvVerdict::Stale(reason) = verdict else {
        return loaded.map(|()| false);
    };
    match loaded {
        Ok(()) => {
            log::info!("Cleanup runtime differs from its lock ({reason}); syncing it in the background");
            Ok(true)
        }
        Err(error) => {
            // The drift the lock exists for may be what breaks the load:
            // rebuild from the lock now, with pip's full patience, and load
            // again. Only if that fails too does the preparation fail.
            log::warn!(
                "Cleanup did not load on the stale runtime ({reason}): {error}; rebuilding it from the lock"
            );
            replace_runtime(state, host, PipNetwork::Patient, Handover::Replace).await?;
            Ok(false)
        }
    }
}

/// Recover from an interrupted preparation, then do the runtime half of
/// `mode`. Returns the verdict Ensure acted on.
async fn prepare_runtime<H: PreparationHost>(
    state: &AppState,
    host: &H,
    mode: Preparation,
) -> Result<VenvVerdict, String> {
    let runtime = host.runtime();
    run_blocking(&runtime, |runtime| runtime.recover()).await?;
    if mode == Preparation::Repair {
        replace_runtime(state, host, PipNetwork::Patient, Handover::Replace).await?;
        return Ok(VenvVerdict::Ready);
    }
    let verdict = run_blocking(&runtime, |runtime| Ok(runtime.verdict())).await?;
    match &verdict {
        // A crash between a swap and its keep-or-restore step leaves the old
        // runtime aside for good. A Ready runtime needs no swap, and the
        // claim excludes a sync's Verify handover, so the copies are leftovers.
        VenvVerdict::Ready => {
            run_blocking(&runtime, |runtime| {
                runtime.keep_installed();
                Ok(())
            })
            .await?
        }
        VenvVerdict::Stale(_) => {}
        VenvVerdict::Missing => {
            replace_runtime(state, host, PipNetwork::Patient, Handover::Fresh).await?
        }
        VenvVerdict::Broken(_) => return Err(verdict.unavailable_reason()),
        VenvVerdict::Unknown(reason) => {
            // A probe that never answered says nothing about the runtime, so
            // it must not trigger an install. The verdict expires after
            // `VENV_UNKNOWN_RETRY_INTERVAL` and the next attempt re-probes.
            log::warn!("Cleanup preparation deferred: {reason}");
            return Err(verdict.unavailable_reason());
        }
    }
    Ok(verdict)
}

/// The background sync of a stale runtime that cleanup already runs on. A
/// failure, for example offline, keeps the existing runtime.
async fn sync_stale_runtime<H: PreparationHost>(state: &AppState, host: &H) {
    let Some(claim) = claim_preparation(state, Preparation::Sync).await else {
        return;
    };
    // A Repair that won the claim first may have replaced the runtime.
    let runtime = host.runtime();
    if let Ok(VenvVerdict::Stale(_)) = run_blocking(&runtime, |runtime| Ok(runtime.verdict())).await
    {
        match replace_runtime(state, host, PipNetwork::Brief, Handover::Verify).await {
            Ok(()) => log::info!("Cleanup runtime synced to its lock"),
            Err(error) => {
                log::warn!("Cleanup runtime sync failed; cleanup keeps the existing runtime: {error}")
            }
        }
    }
    drop(claim);
    host.changed();
}

/// What a swap does with the runtime it replaces.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Handover {
    /// No runtime existed. Ensure loads the new one next.
    Fresh,
    /// The old runtime is the one the user called broken (Repair) or the one
    /// that just failed to load: keep the new one at once, then load it.
    Replace,
    /// The old runtime works: keep it aside until the new one loads, and go
    /// back to it when the new one does not.
    Verify,
}

/// Build a runtime in staging without `llm_operation`, so cleanup keeps
/// running on the installed runtime meanwhile, then take the lock only to
/// unload, swap and load.
async fn replace_runtime<H: PreparationHost>(
    state: &AppState,
    host: &H,
    network: PipNetwork,
    handover: Handover,
) -> Result<(), String> {
    let runtime = host.runtime();
    run_blocking(&runtime, move |runtime| runtime.build(network)).await?;
    let _operation = state.llm_operation.lock().await;
    let resident = state.llm_engine.lock().await.is_some();
    // The resident sidecar runs from the runtime being replaced.
    unload_locked(state).await;
    if let Err(error) = run_blocking(&runtime, |runtime| runtime.install_built()).await {
        // A failed swap put the previous runtime back.
        if resident {
            if let Err(reload) = host.load(state).await {
                log::warn!("Cleanup did not reload on the previous runtime: {reload}");
            }
        }
        return Err(error);
    }
    let keep = |runtime: &H::Runtime| {
        runtime.keep_installed();
        Ok(())
    };
    match handover {
        Handover::Fresh => run_blocking(&runtime, keep).await,
        Handover::Replace => {
            run_blocking(&runtime, keep).await?;
            host.load(state).await
        }
        // Cleanup was turned off during the build: the import probe is the
        // check, and nothing starts a sidecar while cleanup is off.
        Handover::Verify if !resident => run_blocking(&runtime, keep).await,
        Handover::Verify => match host.load(state).await {
            Ok(()) => run_blocking(&runtime, keep).await,
            Err(error) => {
                log::warn!("The synced cleanup runtime did not load ({error}); going back to the previous one");
                unload_locked(state).await;
                run_blocking(&runtime, |runtime| runtime.restore_previous()).await?;
                host.load(state).await?;
                Err(format!("The synced cleanup runtime did not load: {error}"))
            }
        },
    }
}

async fn run_blocking<R: RuntimeSetup, T: Send + 'static>(
    runtime: &R,
    step: impl FnOnce(&R) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let runtime = runtime.clone();
    tokio::task::spawn_blocking(move || step(&runtime))
        .await
        .map_err(|e| format!("Cleanup runtime task failed: {e}"))?
}

/// Owns the running preparation. Dropping it, on every exit path including a
/// cancelled future, clears `llm_preparing` and wakes waiting callers (R-16).
struct PreparationClaim<'a>(&'a AppState);

impl Drop for PreparationClaim<'_> {
    fn drop(&mut self) {
        self.0.llm_preparing.store(IDLE, Ordering::SeqCst);
        self.0.llm_preparation_finished.notify_waiters();
    }
}

/// Claims a preparation of `mode`, recording the mode. A caller whose mode is
/// already running joins it and gets `None` once it finishes; a caller with
/// another mode waits and then runs its own, so a Repair requested during
/// Ensure or a sync is not dropped.
async fn claim_preparation(state: &AppState, mode: Preparation) -> Option<PreparationClaim<'_>> {
    loop {
        match state
            .llm_preparing
            .compare_exchange(IDLE, mode as u8, Ordering::SeqCst, Ordering::SeqCst)
        {
            Ok(_) => return Some(PreparationClaim(state)),
            Err(running) => {
                wait_until_idle(state).await;
                if running == mode as u8 {
                    return None;
                }
            }
        }
    }
}

async fn wait_until_idle(state: &AppState) {
    loop {
        let notified = state.llm_preparation_finished.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if state.llm_preparing.load(Ordering::SeqCst) == IDLE {
            return;
        }
        notified.await;
    }
}

async fn load_locked(state: &AppState) -> Result<(), String> {
    let mut sidecar = engine::ensure_running(state).await?;
    // A resident process may have died since its last use. Validate it too.
    let result = tokio::task::spawn_blocking(move || {
        let result = sidecar.request_raw(&serde_json::json!({"action": "load"}));
        (sidecar, result)
    })
    .await
    .map_err(|e| format!("Cleanup load task failed: {e}"))?;
    let (sidecar, response) = result;
    let response = response.and_then(|response| {
        engine::validate_loaded_model(&response)?;
        Ok(response)
    });
    if let Err(error) = response {
        state.llm_loaded.store(false, Ordering::SeqCst);
        return Err(error);
    }
    state.llm_loaded.store(true, Ordering::SeqCst);
    *state.llm_engine.lock().await = Some(sidecar);
    Ok(())
}

#[tauri::command]
pub async fn delete_llm_model(state: State<'_, AppState>) -> Result<(), String> {
    let _operation = state.llm_operation.lock().await;
    unload_locked(&state).await;
    tokio::task::spawn_blocking(download::delete_model)
        .await
        .map_err(|e| format!("Model removal failed: {e}"))?
}

async fn unload_locked(state: &AppState) {
    let sidecar = state.llm_engine.lock().await.take();
    if let Some(mut sidecar) = sidecar {
        let _ = tokio::task::spawn_blocking(move || sidecar.shutdown()).await;
    }
    state.llm_pid.store(0, Ordering::SeqCst);
    state.llm_loaded.store(false, Ordering::SeqCst);
}

/// Persistence acknowledgements must not wait for inference or setup to finish.
/// A later saved-on preference wins over an earlier queued unload request.
pub fn notify_on_cleanup_preference_saved(app: &AppHandle, enabled: bool) {
    if enabled {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let _operation = state.llm_operation.lock().await;
        if !state.settings.lock().await.llm_cleanup_enabled {
            unload_locked(&state).await;
            let _ = app.emit("llm-preparation-changed", ());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Settings;
    use crate::test_support::{MockAsrEngine, MockAudioCapture, MockPasteBackend};
    use std::collections::VecDeque;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    fn test_state() -> Arc<AppState> {
        Arc::new(AppState::new_with_backends(
            Box::new(MockAudioCapture::sine_wave()),
            Box::new(MockAsrEngine::with_text("unused")),
            None,
            Box::new(MockPasteBackend::new()),
            Settings::default(),
        ))
    }

    #[tokio::test]
    async fn preparation_callers_join_one_owner_and_observe_its_error() {
        let state = test_state();
        let claim = claim_preparation(&state, Preparation::Ensure)
            .await
            .expect("the first caller owns the preparation");
        let waiting_state = state.clone();
        let waiter = tokio::spawn(async move {
            let owner = claim_preparation(&waiting_state, Preparation::Ensure)
                .await
                .is_some();
            let error = waiting_state.llm_setup_error.lock().await.clone();
            (owner, error)
        });
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());
        *state.llm_setup_error.lock().await = Some("offline".into());
        drop(claim);
        let (owner, error) = waiter.await.unwrap();
        assert!(!owner);
        assert_eq!(error.as_deref(), Some("offline"));
        // An explicit retry owns a new flight.
        assert!(claim_preparation(&state, Preparation::Ensure).await.is_some());
    }

    #[tokio::test]
    async fn a_repair_requested_during_ensure_runs_after_it() {
        let state = test_state();
        let ensure = claim_preparation(&state, Preparation::Ensure).await.unwrap();
        let waiting_state = state.clone();
        let repair = tokio::spawn(async move {
            let claim = claim_preparation(&waiting_state, Preparation::Repair).await;
            (
                claim.is_some(),
                waiting_state.llm_preparing.load(Ordering::SeqCst),
            )
        });
        tokio::task::yield_now().await;
        assert!(!repair.is_finished(), "Repair waits for the running Ensure");
        drop(ensure);
        let (owned, running) = repair.await.unwrap();
        assert!(owned, "a Repair requested during Ensure must run, not join it");
        assert_eq!(running, Preparation::Repair as u8);
        assert_eq!(state.llm_preparing.load(Ordering::SeqCst), IDLE);
    }

    #[tokio::test]
    async fn a_cancelled_preparation_still_releases_its_claim() {
        let state = test_state();
        let task_state = state.clone();
        let flight = tokio::spawn(async move {
            let _claim = claim_preparation(&task_state, Preparation::Repair).await;
            std::future::pending::<()>().await;
        });
        tokio::task::yield_now().await;
        assert_eq!(
            state.llm_preparing.load(Ordering::SeqCst),
            Preparation::Repair as u8
        );
        flight.abort();
        assert!(flight.await.unwrap_err().is_cancelled());
        assert_eq!(state.llm_preparing.load(Ordering::SeqCst), IDLE);
        assert!(claim_preparation(&state, Preparation::Ensure).await.is_some());
    }

    /// A sidecar that answers the load handshake for the pinned model.
    struct LoadedSidecar;

    impl engine::LlmBackend for LoadedSidecar {
        fn cleanup_batch(
            &mut self,
            texts: &[String],
            _mode: crate::models::CleanupMode,
        ) -> Result<Vec<engine::BatchItem>, String> {
            Ok(texts
                .iter()
                .map(|text| engine::BatchItem::Proposal(Some(text.clone())))
                .collect())
        }

        fn request_raw(&mut self, _req: &serde_json::Value) -> Result<serde_json::Value, String> {
            Ok(serde_json::json!({
                "ok": true,
                "model_id": engine::SOTTO_MODEL.id,
                "revision": engine::MODEL_REVISION,
                "prompt_sha256": engine::PROMPT_SHA256,
                "warmed": true,
            }))
        }
    }

    /// The runtime and the app, scripted: loads and builds return queued
    /// results, and every step is logged in order.
    #[derive(Clone)]
    struct Script {
        state: Arc<AppState>,
        verdict: VenvVerdict,
        loads: Arc<Mutex<VecDeque<Result<(), String>>>>,
        builds: Arc<Mutex<VecDeque<Result<(), String>>>>,
        events: Arc<Mutex<Vec<String>>>,
        syncs: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    }

    impl Script {
        fn stale(
            state: &Arc<AppState>,
            loads: &[Result<(), &str>],
            builds: &[Result<(), &str>],
        ) -> Self {
            let queue = |results: &[Result<(), &str>]| {
                Arc::new(Mutex::new(
                    results
                        .iter()
                        .map(|result| result.map_err(str::to_string))
                        .collect(),
                ))
            };
            Self {
                state: state.clone(),
                verdict: VenvVerdict::Stale("packages differ from the runtime lock: certifi".into()),
                loads: queue(loads),
                builds: queue(builds),
                events: Arc::default(),
                syncs: Arc::default(),
            }
        }

        fn log(&self, event: impl Into<String>) {
            self.events.lock().unwrap().push(event.into());
        }

        fn events(&self) -> Vec<String> {
            self.events.lock().unwrap().clone()
        }

        async fn finish_syncs(&self) {
            let syncs = std::mem::take(&mut *self.syncs.lock().unwrap());
            for sync in syncs {
                sync.await.unwrap();
            }
        }
    }

    impl RuntimeSetup for Script {
        fn recover(&self) -> Result<(), String> {
            self.log("recover");
            Ok(())
        }

        fn verdict(&self) -> VenvVerdict {
            self.verdict.clone()
        }

        fn build(&self, network: PipNetwork) -> Result<(), String> {
            let cleanup = if self.state.llm_operation.try_lock().is_ok() {
                "free"
            } else {
                "blocked"
            };
            self.log(format!(
                "build {network:?} preparing={} cleanup={cleanup}",
                reports_preparing(&self.state)
            ));
            self.builds.lock().unwrap().pop_front().expect("unscripted build")
        }

        fn install_built(&self) -> Result<(), String> {
            self.log("install");
            Ok(())
        }

        fn keep_installed(&self) {
            self.log("keep new runtime");
        }

        fn restore_previous(&self) -> Result<(), String> {
            self.log("restore previous runtime");
            Ok(())
        }
    }

    impl PreparationHost for Script {
        type Runtime = Script;

        fn runtime(&self) -> Script {
            self.clone()
        }

        fn changed(&self) {}

        async fn ensure_model(&self) -> Result<(), String> {
            Ok(())
        }

        async fn load(&self, state: &AppState) -> Result<(), String> {
            let result = self.loads.lock().unwrap().pop_front().expect("unscripted load");
            self.log(if result.is_ok() { "load ok" } else { "load failed" });
            if result.is_ok() {
                *state.llm_engine.lock().await = Some(Box::new(LoadedSidecar));
            }
            state.llm_loaded.store(result.is_ok(), Ordering::SeqCst);
            result
        }

        fn start_stale_sync(&self) {
            self.log("sync task started");
            let host = self.clone();
            self.syncs.lock().unwrap().push(tokio::spawn(async move {
                let state = host.state.clone();
                sync_stale_runtime(&state, &host).await;
            }));
        }
    }

    #[tokio::test]
    async fn a_stale_runtime_loads_first_and_syncs_after_the_claim_is_released() {
        let state = test_state();
        let script = Script::stale(&state, &[Ok(())], &[Err("ERROR: network is unreachable")]);

        prepare_with(&state, &script, Preparation::Ensure).await.unwrap();

        assert_eq!(
            script.events(),
            ["recover", "load ok", "sync task started"],
            "the preparation answers before the sync builds"
        );
        assert_eq!(state.llm_preparing.load(Ordering::SeqCst), IDLE);
        script.finish_syncs().await;
        assert_eq!(
            script.events(),
            [
                "recover",
                "load ok",
                "sync task started",
                // Own claim, not shown as preparing, cleanup not blocked.
                "build Brief preparing=false cleanup=free",
            ]
        );
        assert!(state.llm_loaded.load(Ordering::SeqCst));
        assert!(
            state.llm_engine.lock().await.is_some(),
            "an offline sync keeps cleanup on the existing runtime"
        );
        assert_eq!(state.llm_preparing.load(Ordering::SeqCst), IDLE);
    }

    #[tokio::test]
    async fn a_stale_runtime_that_does_not_load_is_rebuilt_from_the_lock() {
        let state = test_state();
        let script = Script::stale(&state, &[Err("model did not load"), Ok(())], &[Ok(())]);

        prepare_with(&state, &script, Preparation::Ensure).await.unwrap();

        assert_eq!(
            script.events(),
            [
                "recover",
                "load failed",
                "build Patient preparing=true cleanup=free",
                "install",
                "keep new runtime",
                "load ok",
            ]
        );
        assert!(script.syncs.lock().unwrap().is_empty(), "no background sync follows");
        assert!(state.llm_loaded.load(Ordering::SeqCst));
        assert_eq!(*state.llm_setup_error.lock().await, None);
    }

    #[tokio::test]
    async fn a_stale_runtime_that_neither_loads_nor_rebuilds_reports_the_error() {
        let state = test_state();
        let script = Script::stale(
            &state,
            &[Err("model did not load")],
            &[Err("ERROR: network is unreachable")],
        );

        let error = prepare_with(&state, &script, Preparation::Ensure)
            .await
            .unwrap_err();

        assert!(error.contains("network is unreachable"), "{error}");
        assert_eq!(state.llm_setup_error.lock().await.as_deref(), Some(error.as_str()));
        assert!(!state.llm_loaded.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_synced_runtime_that_does_not_load_gives_way_to_the_previous_one() {
        let state = test_state();
        let script = Script::stale(
            &state,
            &[Ok(()), Err("synced runtime did not load"), Ok(())],
            &[Ok(())],
        );

        prepare_with(&state, &script, Preparation::Ensure).await.unwrap();
        script.finish_syncs().await;

        assert_eq!(
            script.events(),
            [
                "recover",
                "load ok",
                "sync task started",
                "build Brief preparing=false cleanup=free",
                "install",
                "load failed",
                "restore previous runtime",
                "load ok",
            ],
            "the old runtime is kept until the new one loads"
        );
        assert!(state.llm_loaded.load(Ordering::SeqCst));
    }

    fn script(directory: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = directory.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// The runtime in a temporary directory. The stand-in host interpreter
    /// makes venvs whose interpreter (`venv_python`) passes the probe and runs
    /// `pip_body` for pip. No sidecar ever starts.
    #[derive(Clone)]
    struct DirRuntime {
        venv: PathBuf,
        venv_python: PathBuf,
        host: PathBuf,
        lock: PathBuf,
    }

    impl DirRuntime {
        fn new(directory: &tempfile::TempDir, pip_body: &str) -> Self {
            let venv_python = script(
                directory.path(),
                "venv-python",
                &format!("[ \"$2\" = \"-c\" ] && exit 0\n{pip_body}"),
            );
            let host = script(
                directory.path(),
                "host-python",
                &format!(
                    "mkdir -p \"$4/bin\" && cp '{}' \"$4/bin/python3\"",
                    venv_python.display()
                ),
            );
            let lock = directory.path().join("requirements.lock");
            std::fs::write(&lock, "").unwrap();
            Self {
                venv: directory.path().join("support").join("llm-venv"),
                venv_python,
                host,
                lock,
            }
        }

        fn staging(&self) -> PathBuf {
            engine::staging_path(&self.venv)
        }
    }

    impl RuntimeSetup for DirRuntime {
        fn recover(&self) -> Result<(), String> {
            engine::recover_runtime_at(&self.venv).map(|_| ())
        }

        fn verdict(&self) -> VenvVerdict {
            engine::probe_venv_at(&self.venv.join("bin").join("python3"), Duration::from_secs(5))
        }

        fn build(&self, network: PipNetwork) -> Result<(), String> {
            engine::build_runtime_at(&self.staging(), &self.host, &self.lock, network)
        }

        fn install_built(&self) -> Result<(), String> {
            engine::install_built_at(&self.venv, &self.staging(), chrono::Utc::now())
        }

        fn keep_installed(&self) {
            engine::sweep_aside_dirs(&self.venv);
        }

        fn restore_previous(&self) -> Result<(), String> {
            engine::restore_previous_at(&self.venv)
        }
    }

    impl PreparationHost for DirRuntime {
        type Runtime = DirRuntime;

        fn runtime(&self) -> DirRuntime {
            self.clone()
        }

        fn changed(&self) {}

        async fn ensure_model(&self) -> Result<(), String> {
            Ok(())
        }

        async fn load(&self, _state: &AppState) -> Result<(), String> {
            Err("these tests start no sidecar".into())
        }

        fn start_stale_sync(&self) {}
    }

    #[tokio::test]
    async fn a_ready_runtime_is_left_alone_and_its_leftovers_are_removed() {
        let directory = tempfile::TempDir::new().unwrap();
        let runtime = DirRuntime::new(&directory, "exit 0");
        let installed = runtime.venv.join("bin").join("python3");
        std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
        std::fs::copy(&runtime.venv_python, &installed).unwrap();
        let before = std::fs::read(&installed).unwrap();
        // A quit during an earlier build left a half-installed staging copy.
        std::fs::create_dir_all(runtime.staging().join("bin")).unwrap();
        std::fs::write(runtime.staging().join("bin").join("python3"), "partial").unwrap();
        // A crash between a swap and its keep-or-restore step left the old
        // runtime aside behind the installed one.
        let aside = runtime.venv.with_file_name("llm-venv.aside-20261119T101500Z");
        std::fs::create_dir_all(aside.join("bin")).unwrap();

        let verdict = prepare_runtime(&test_state(), &runtime, Preparation::Ensure)
            .await
            .unwrap();

        // A Ready runtime builds nothing, so only recovery removed staging.
        assert_eq!(verdict, VenvVerdict::Ready);
        assert!(!runtime.staging().exists());
        assert!(!aside.exists(), "the leftover aside copy is removed");
        assert_eq!(
            std::fs::read(&installed).unwrap(),
            before,
            "the installed runtime is untouched"
        );
        assert_eq!(runtime.verdict(), VenvVerdict::Ready);
    }

    #[tokio::test]
    async fn an_ensure_without_a_runtime_creates_one() {
        let directory = tempfile::TempDir::new().unwrap();
        let runtime = DirRuntime::new(&directory, "exit 0");
        let state = test_state();

        let verdict = prepare_runtime(&state, &runtime, Preparation::Ensure)
            .await
            .unwrap();

        assert_eq!(verdict, VenvVerdict::Missing);
        assert_eq!(runtime.verdict(), VenvVerdict::Ready, "a new runtime is in place");
        assert!(!runtime.staging().exists());
        assert!(
            state.llm_engine.lock().await.is_none(),
            "loading waits for the model check"
        );
    }

    #[tokio::test]
    async fn a_repair_interrupted_after_the_move_aside_gets_its_runtime_back() {
        let directory = tempfile::TempDir::new().unwrap();
        let runtime = DirRuntime::new(
            &directory,
            "echo 'ERROR: network is unreachable' >&2\nexit 1",
        );
        // A quit after the old runtime moved aside and before the new one
        // moved in.
        let aside = runtime.venv.with_file_name("llm-venv.aside-20261119T101500Z");
        std::fs::create_dir_all(&aside).unwrap();
        std::fs::write(aside.join("marker"), "previous runtime").unwrap();
        std::fs::create_dir_all(runtime.staging()).unwrap();

        let error = prepare_runtime(&test_state(), &runtime, Preparation::Repair)
            .await
            .unwrap_err();

        assert!(error.contains("network is unreachable"), "{error}");
        assert_eq!(
            std::fs::read_to_string(runtime.venv.join("marker")).unwrap(),
            "previous runtime"
        );
        assert!(!aside.exists());
        assert!(!runtime.staging().exists());
    }
}
