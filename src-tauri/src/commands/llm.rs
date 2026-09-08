use std::sync::atomic::Ordering;

use tauri::{AppHandle, Emitter, Manager, State};

use crate::llm::{download, engine};
use crate::models::LlmStatus;
use crate::state::AppState;
use crate::tray::menu;

/// Status reads are offline and never import Python/MLX or begin setup.
#[tauri::command]
pub async fn get_llm_status(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<LlmStatus, String> {
    current_status(&app, &state).await
}

async fn current_status(app: &AppHandle, state: &AppState) -> Result<LlmStatus, String> {
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
        preparing: state.llm_preparing.load(Ordering::SeqCst),
        setup_error: state.llm_setup_error.lock().await.clone(),
        loaded: state.llm_loaded.load(Ordering::SeqCst)
            && crate::process::registered_is_alive(state.llm_pid.load(Ordering::SeqCst)),
        enabled: state.settings.lock().await.llm_cleanup_enabled,
        busy: state.llm_operation.try_lock().is_err(),
        model_name: config.display_name.to_string(),
        model_url: format!("https://huggingface.co/{}", config.id),
        download_size_mb: config.download_size_mb,
        model_path: None,
        update_available: app
            .try_state::<crate::updater::UpdateState>()
            .map(|u| u.model_update_available.load(Ordering::SeqCst))
            .unwrap_or(false),
        last_cleanup_status: state.llm_last_status.lock().await.clone(),
    })
}

/// Explicit enable intent: install the runtime, download if missing, and verify
/// loading. Concurrent windows join this preparation instead of duplicating it.
#[tauri::command]
pub async fn prepare_llm_model(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<LlmStatus, String> {
    if !engine::is_feature_compiled() || !engine::is_platform_supported() {
        return Err("Local cleanup requires an Apple Silicon Mac".into());
    }
    if !claim_preparation(&state).await {
        if let Some(error) = state.llm_setup_error.lock().await.clone() {
            return Err(error);
        }
        return current_status(&app, &state).await;
    }
    *state.llm_setup_error.lock().await = None;
    let _ = app.emit("llm-preparation-changed", ());
    let result = async {
        let _operation = state.llm_operation.lock().await;
        let ready = tokio::task::spawn_blocking(|| {
            engine::is_venv_ready() && engine::is_model_downloaded()
        })
        .await
        .map_err(|e| format!("Cleanup readiness check failed: {e}"))?;
        if !ready {
            download::download_model(&app).await?;
        }
        load_locked(&state).await
    }
    .await;
    *state.llm_setup_error.lock().await = result.as_ref().err().cloned();
    state.llm_preparing.store(false, Ordering::SeqCst);
    state.llm_preparation_finished.notify_waiters();
    let _ = app.emit("llm-preparation-changed", ());
    result?;
    current_status(&app, &state).await
}

/// Returns true only to the owner; other callers join the current preparation.
async fn claim_preparation(state: &AppState) -> bool {
    if state
        .llm_preparing
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        return true;
    }
    loop {
        let notified = state.llm_preparation_finished.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if !state.llm_preparing.load(Ordering::SeqCst) {
            return false;
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
pub async fn check_llm_update(app: AppHandle) -> Result<bool, String> {
    engine::check_model_update(&app).await
}

#[tauri::command]
pub async fn download_llm_model(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let _operation = state.llm_operation.lock().await;
    download::download_model(&app).await
}

#[tauri::command]
pub async fn update_llm_model(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let _operation = state.llm_operation.lock().await;
    unload_locked(&state).await;
    download::download_model(&app).await?;
    if let Some(updater) = app.try_state::<crate::updater::UpdateState>() {
        updater
            .model_update_available
            .store(false, Ordering::SeqCst);
        updater
            .model_update_consecutive_errors
            .store(0, Ordering::SeqCst);
    }
    menu::refresh_tray_from_state(&app).await;
    Ok(())
}

#[tauri::command]
pub fn cancel_llm_download() -> Result<(), String> {
    Err(
        "Model setup continues in the background; cancelling activation keeps cleanup disabled"
            .into(),
    )
}

#[tauri::command]
pub async fn delete_llm_model(state: State<'_, AppState>) -> Result<(), String> {
    let _operation = state.llm_operation.lock().await;
    unload_locked(&state).await;
    tokio::task::spawn_blocking(download::delete_model)
        .await
        .map_err(|e| format!("Model removal failed: {e}"))?
}

#[tauri::command]
pub async fn load_llm_model(state: State<'_, AppState>) -> Result<(), String> {
    let _operation = state.llm_operation.lock().await;
    load_locked(&state).await
}

async fn unload_locked(state: &AppState) {
    let sidecar = state.llm_engine.lock().await.take();
    if let Some(mut sidecar) = sidecar {
        let _ = tokio::task::spawn_blocking(move || sidecar.shutdown()).await;
    }
    state.llm_pid.store(0, Ordering::SeqCst);
    state.llm_loaded.store(false, Ordering::SeqCst);
}

#[tauri::command]
pub async fn unload_llm_model(state: State<'_, AppState>) -> Result<(), String> {
    let _operation = state.llm_operation.lock().await;
    unload_locked(&state).await;
    Ok(())
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
    use std::sync::Arc;

    #[tokio::test]
    async fn preparation_callers_join_one_owner_and_observe_its_error() {
        let state = Arc::new(AppState::new_with_backends(
            Box::new(MockAudioCapture::sine_wave()),
            Box::new(MockAsrEngine::with_text("unused")),
            None,
            Box::new(MockPasteBackend::new()),
            Settings::default(),
        ));
        assert!(claim_preparation(&state).await);
        let waiting_state = state.clone();
        let waiter = tokio::spawn(async move {
            let owner = claim_preparation(&waiting_state).await;
            let error = waiting_state.llm_setup_error.lock().await.clone();
            (owner, error)
        });
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());
        *state.llm_setup_error.lock().await = Some("offline".into());
        state.llm_preparing.store(false, Ordering::SeqCst);
        state.llm_preparation_finished.notify_waiters();
        let (owner, error) = waiter.await.unwrap();
        assert!(!owner);
        assert_eq!(error.as_deref(), Some("offline"));
        assert!(claim_preparation(&state).await); // Explicit retry owns a new flight.
    }
}
