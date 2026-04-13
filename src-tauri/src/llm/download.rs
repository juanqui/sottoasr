use tauri::AppHandle;
use tauri::Emitter;
use tauri::Manager;

use crate::llm::engine;
use crate::llm::engine::LlmBackend;
use crate::state::AppState;

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

            // Pre-load the model so it's warm for the first cleanup request
            let preload_app = app.clone();
            tauri::async_runtime::spawn(async move {
                log::info!("Pre-loading LLM sidecar after download...");
                match tokio::task::spawn_blocking(|| {
                    let mut e = engine::LlmEngine::spawn()?;
                    e.load_model()?;
                    Ok::<_, String>(e)
                }).await {
                    Ok(Ok(engine)) => {
                        let state = preload_app.state::<AppState>();
                        let mut guard = state.llm_engine.lock().await;
                        // Shut down existing sidecar before replacing to avoid
                        // two MLX processes competing for unified memory.
                        if let Some(mut old) = guard.take() {
                            log::info!("Shutting down old LLM sidecar before replacing");
                            old.shutdown();
                        }
                        *guard = Some(Box::new(engine) as Box<dyn LlmBackend>);
                        log::info!("LLM sidecar pre-loaded after download");
                    }
                    Ok(Err(e)) => {
                        log::warn!("LLM pre-load after download failed: {}", e);
                    }
                    Err(e) => {
                        log::error!("LLM pre-load after download panicked: {}", e);
                    }
                }
            });

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
///
/// HuggingFace's default cache lives at `~/.cache/huggingface/hub/` — NOT at
/// `~/Library/Caches/huggingface/hub/` which is what `dirs::cache_dir()`
/// resolves to on macOS. The old implementation deleted the wrong path (or
/// nothing at all), leaving orphaned weights on disk.
pub fn delete_model() -> Result<(), String> {
    let model_id = engine::model_config().id;
    let Some(home) = dirs::home_dir() else {
        return Err("Could not determine home directory".into());
    };
    let cache_name = model_id.replace('/', "--");
    let hf_cache = home
        .join(".cache")
        .join("huggingface")
        .join("hub")
        .join(format!("models--{}", cache_name));
    if hf_cache.exists() {
        std::fs::remove_dir_all(&hf_cache)
            .map_err(|e| format!("Failed to delete model cache: {}", e))?;
        log::info!("Deleted model cache at {:?}", hf_cache);
    }
    Ok(())
}
