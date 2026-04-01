use tauri::State;
use crate::state::AppState;
use crate::models::LlmStatus;
use crate::llm::{engine, download};

/// Get the current LLM model status.
#[tauri::command]
pub async fn get_llm_status(state: State<'_, AppState>) -> Result<LlmStatus, String> {
    let compiled = engine::is_feature_compiled();
    let supported = if compiled {
        tokio::task::spawn_blocking(engine::is_platform_supported)
            .await.unwrap_or(false)
    } else {
        false
    };
    let available = compiled && supported;

    let unavailable_reason = if !compiled {
        Some("LLM feature not included in this build".into())
    } else if !supported {
        Some("Requires Apple Silicon (M1 or later) with Python 3".into())
    } else {
        None
    };

    let config = engine::model_config();

    // Check if sidecar is running (model loaded)
    let loaded = {
        let engine_guard = state.llm_engine.lock().await;
        engine_guard.is_some()
    };

    // Check if model is downloaded by looking at HuggingFace cache
    let downloaded = if available && engine::is_venv_ready() {
        let model_id = config.id;
        let cache_dir = dirs::home_dir()
            .map(|h| h.join(".cache/huggingface/hub"))
            .unwrap_or_default();
        let cache_name = format!("models--{}", model_id.replace('/', "--"));
        let model_cache = cache_dir.join(&cache_name);
        model_cache.join("snapshots").is_dir()
    } else {
        false
    };

    Ok(LlmStatus {
        available,
        unavailable_reason,
        downloaded,
        downloading: false,
        loaded,
        model_name: config.display_name.to_string(),
        model_path: None,
        update_available: false, // Use check_llm_update for async update check
    })
}

/// Check if a model update is available on HuggingFace.
#[tauri::command]
pub async fn check_llm_update() -> Result<bool, String> {
    tokio::task::spawn_blocking(move || {
        match engine::LlmEngine::spawn() {
            Ok(mut e) => {
                let resp = e.request_raw(&serde_json::json!({"action": "check_update"}));
                e.quit();
                match resp {
                    Ok(v) => Ok(v.get("update_available")
                        .and_then(|u| u.as_bool())
                        .unwrap_or(false)),
                    Err(e) => {
                        log::warn!("Update check failed: {}", e);
                        Ok(false)
                    }
                }
            }
            Err(e) => {
                log::warn!("Could not spawn sidecar for update check: {}", e);
                Ok(false)
            }
        }
    }).await.map_err(|e| format!("Update check panicked: {}", e))?
}

/// Start downloading (or updating) the LLM model.
#[tauri::command]
pub async fn download_llm_model(
    app: tauri::AppHandle,
    _state: State<'_, AppState>,
) -> Result<(), String> {
    download::download_model(&app).await
}

/// Update the LLM model: shut down sidecar, re-download, ready for reload.
#[tauri::command]
pub async fn update_llm_model(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    // Shut down running sidecar so it picks up new model on next use
    {
        let mut engine_guard = state.llm_engine.lock().await;
        if let Some(mut e) = engine_guard.take() {
            e.quit();
        }
    }

    // Re-download (huggingface_hub will fetch the latest revision)
    download::download_model(&app).await
}

/// Cancel an in-progress LLM model download.
#[tauri::command]
pub fn cancel_llm_download() -> Result<(), String> {
    log::warn!("LLM download cancellation not implemented");
    Ok(())
}

/// Delete the downloaded LLM model to free disk space.
#[tauri::command]
pub async fn delete_llm_model(state: State<'_, AppState>) -> Result<(), String> {
    {
        let mut engine_guard = state.llm_engine.lock().await;
        if let Some(mut e) = engine_guard.take() {
            e.quit();
        }
    }

    download::delete_model()
}

/// Load the LLM model (spawn sidecar and load model into memory).
#[tauri::command]
pub async fn load_llm_model(state: State<'_, AppState>) -> Result<(), String> {
    let sidecar = tokio::task::spawn_blocking(move || {
        let mut e = engine::LlmEngine::spawn()?;
        e.load_model()?;
        Ok::<_, String>(e)
    }).await.map_err(|e| format!("Load task panicked: {}", e))??;

    let mut guard = state.llm_engine.lock().await;
    *guard = Some(sidecar);
    log::info!("LLM sidecar running and model loaded");
    Ok(())
}

/// Unload the LLM model (shut down sidecar).
#[tauri::command]
pub async fn unload_llm_model(state: State<'_, AppState>) -> Result<(), String> {
    let mut guard = state.llm_engine.lock().await;
    if let Some(mut e) = guard.take() {
        e.quit();
    }
    log::info!("LLM sidecar shut down");
    Ok(())
}
