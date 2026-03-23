use tauri::AppHandle;
use tauri::Emitter;

/// Download the model via the sidecar process.
/// The sidecar delegates to huggingface_hub for robust, resumable downloads.
pub async fn download_model_with_id(app: &AppHandle, model_id: &str) -> Result<(), String> {
    let config = crate::llm::engine::all_model_configs()
        .into_iter()
        .find(|c| c.id == model_id)
        .unwrap_or(&crate::llm::engine::MODEL_4B);

    let _ = app.emit("llm-download-started", serde_json::json!({
        "total_bytes": config.download_size_mb * 1_000_000,
        "file_count": 1u32,
    }));

    log::info!("Starting LLM model download via sidecar: {}...", model_id);

    let model_id_owned = model_id.to_string();
    let result = tokio::task::spawn_blocking(move || {
        let mut engine = crate::llm::engine::LlmEngine::spawn_with_model(&model_id_owned)?;
        let result = engine.download_model();
        engine.quit();
        result
    }).await.map_err(|e| format!("Download task panicked: {}", e))?;

    match result {
        Ok(()) => {
            log::info!("LLM model download complete");
            let _ = app.emit("llm-download-complete", ());
            Ok(())
        }
        Err(e) => {
            log::error!("LLM model download failed: {}", e);
            let _ = app.emit("llm-download-error", serde_json::json!({ "message": e }));
            Err(e)
        }
    }
}

/// Download the model for the given settings size string.
pub async fn download_model(app: &AppHandle, model_size: &str) -> Result<(), String> {
    let model_id = crate::llm::engine::model_id_for_size(model_size);
    download_model_with_id(app, model_id).await
}

/// Delete downloaded model files from the HuggingFace cache for a specific model.
pub fn delete_model_by_id(model_id: &str) -> Result<(), String> {
    if let Some(cache_dir) = dirs::cache_dir() {
        let cache_name = model_id.replace('/', "--");
        let hf_cache = cache_dir.join("huggingface").join("hub").join(format!("models--{}", cache_name));
        if hf_cache.exists() {
            std::fs::remove_dir_all(&hf_cache)
                .map_err(|e| format!("Failed to delete model cache: {}", e))?;
            log::info!("Deleted model cache at {:?}", hf_cache);
        }
    }
    Ok(())
}

/// Delete model files for the given settings size string.
pub fn delete_model(model_size: &str) -> Result<(), String> {
    let model_id = crate::llm::engine::model_id_for_size(model_size);
    delete_model_by_id(model_id)
}
