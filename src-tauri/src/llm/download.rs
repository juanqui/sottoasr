use tauri::AppHandle;
use tauri::Emitter;

/// Download the model via the sidecar process.
/// The sidecar delegates to huggingface_hub for robust, resumable downloads.
pub async fn download_model(app: &AppHandle) -> Result<(), String> {
    let _ = app.emit("llm-download-started", serde_json::json!({
        "total_bytes": 570_000_000u64,
        "file_count": 1u32,
    }));

    log::info!("Starting LLM model download via sidecar...");

    // Spawn a temporary sidecar for the download
    let result = tokio::task::spawn_blocking(|| {
        let mut engine = crate::llm::engine::LlmEngine::spawn()?;
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

/// Delete downloaded model files from the HuggingFace cache.
pub fn delete_model() -> Result<(), String> {
    // The model is cached by huggingface_hub in ~/.cache/huggingface/hub/
    if let Some(cache_dir) = dirs::cache_dir() {
        let cache_name = crate::llm::engine::MODEL_ID.replace('/', "--");
        let hf_cache = cache_dir.join("huggingface").join("hub").join(format!("models--{}", cache_name));
        if hf_cache.exists() {
            std::fs::remove_dir_all(&hf_cache)
                .map_err(|e| format!("Failed to delete model cache: {}", e))?;
            log::info!("Deleted model cache at {:?}", hf_cache);
        }
    }
    Ok(())
}
