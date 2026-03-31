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

    // Read selected model from settings
    let settings = state.settings.lock().await;
    let model_size = settings.llm_model_size.clone();
    drop(settings);

    let model_id = engine::model_id_for_size(&model_size);
    let config = engine::model_config_for_size(&model_size);

    // Check if sidecar is running (model loaded)
    let loaded = {
        let engine_guard = state.llm_engine.lock().await;
        engine_guard.is_some()
    };

    // Check if model is downloaded by looking at HuggingFace cache
    let downloaded = if available && venv_ready {
        let cache_dir = dirs::home_dir()
            .map(|h| h.join(".cache/huggingface/hub"))
            .unwrap_or_default();
        // Convert model ID "mlx-community/Qwen3.5-2B-OptiQ-4bit" to cache dir name
        let cache_name = format!("models--{}", model_id.replace('/', "--"));
        let model_cache = cache_dir.join(&cache_name);
        let has_snapshots = model_cache.join("snapshots").is_dir();
        if has_snapshots {
            log::info!("LLM model found in HuggingFace cache: {:?}", model_cache);
        }
        has_snapshots
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
    })
}

/// Start downloading the LLM model.
#[tauri::command]
pub async fn download_llm_model(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let settings = state.settings.lock().await;
    let model_size = settings.llm_model_size.clone();
    drop(settings);

    download::download_model(&app, &model_size).await
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

    let settings = state.settings.lock().await;
    let model_size = settings.llm_model_size.clone();
    drop(settings);

    download::delete_model(&model_size)
}

/// Load the LLM model (spawn sidecar and load model into memory).
#[tauri::command]
pub async fn load_llm_model(state: State<'_, AppState>) -> Result<(), String> {
    let settings = state.settings.lock().await;
    let model_size = settings.llm_model_size.clone();
    drop(settings);

    let model_id = engine::model_id_for_size(&model_size).to_string();

    let engine = tokio::task::spawn_blocking(move || {
        let mut e = engine::LlmEngine::spawn_with_model(&model_id)?;
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
