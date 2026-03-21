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

    let venv_ready = engine::is_venv_ready();

    // Check if sidecar is running (model loaded)
    let loaded = {
        let engine_guard = state.llm_engine.lock().await;
        engine_guard.is_some()
    };

    // Check if model is downloaded (only if venv is ready)
    let downloaded = if available && venv_ready {
        tokio::task::spawn_blocking(|| {
            match engine::LlmEngine::spawn() {
                Ok(mut e) => {
                    let status = e.status();
                    e.quit();
                    status.ok()
                        .and_then(|v| v.get("downloaded").and_then(|d| d.as_bool()))
                        .unwrap_or(false)
                }
                Err(_) => false,
            }
        }).await.unwrap_or(false)
    } else {
        false
    };

    Ok(LlmStatus {
        available,
        unavailable_reason,
        downloaded,
        downloading: false,
        loaded,
        model_name: "Qwen3.5-0.8B".into(),
        model_path: None,
    })
}

/// Start downloading the LLM model.
#[tauri::command]
pub async fn download_llm_model(app: tauri::AppHandle) -> Result<(), String> {
    download::download_model(&app).await
}

/// Cancel an in-progress LLM model download.
#[tauri::command]
pub fn cancel_llm_download() -> Result<(), String> {
    log::warn!("LLM download cancellation not implemented in v1");
    Ok(())
}

/// Delete the downloaded LLM model to free disk space.
#[tauri::command]
pub async fn delete_llm_model(state: State<'_, AppState>) -> Result<(), String> {
    // Shut down sidecar first
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
    let engine = tokio::task::spawn_blocking(|| {
        let mut e = engine::LlmEngine::spawn()?;
        e.load_model()?;
        Ok::<_, String>(e)
    }).await.map_err(|e| format!("Load task panicked: {}", e))??;

    let mut guard = state.llm_engine.lock().await;
    *guard = Some(engine);
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
