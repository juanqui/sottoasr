use tauri::AppHandle;
use tauri::Emitter;

use crate::llm::engine;

/// Download the SottoASR cleanup model via the sidecar process.
pub async fn download_model(app: &AppHandle) -> Result<(), String> {
    let config = engine::model_config();

    let _ = app.emit("llm-download-started", serde_json::json!({
        "total_bytes": config.download_size_mb * 1_000_000,
        "file_count": 1u32,
    }));

    log::info!("Starting model download via sidecar: {}...", config.id);

    let result = tokio::task::spawn_blocking(move || {
        let mut sidecar = engine::LlmEngine::spawn()?;
        let result = sidecar.download_model();
        sidecar.quit();
        result
    }).await.map_err(|e| format!("Download task panicked: {}", e))?;

    match result {
        Ok(()) => {
            log::info!("Model download complete");
            let _ = app.emit("llm-download-complete", ());
            Ok(())
        }
        Err(e) => {
            log::error!("Model download failed: {}", e);
            let _ = app.emit("llm-download-error", serde_json::json!({ "message": e }));
            Err(e)
        }
    }
}

/// Delete downloaded model files from the HuggingFace cache.
pub fn delete_model() -> Result<(), String> {
    let model_id = engine::model_config().id;
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
