use crate::state::AppState;
use std::sync::atomic::Ordering;
use tauri::AppHandle;
use tauri::{Emitter, Manager};

use crate::llm::engine;

/// Download the SottoASR cleanup model via the sidecar process.
pub async fn download_model(app: &AppHandle) -> Result<(), String> {
    let config = engine::model_config();
    let state = app.state::<AppState>();
    state.llm_downloading.store(true, Ordering::SeqCst);

    let _ = app.emit(
        "llm-download-started",
        serde_json::json!({
            "total_bytes": config.download_size_mb * 1_000_000,
            "file_count": 1u32,
        }),
    );

    log::info!("Starting model download via sidecar: {}...", config.id);

    let result = tokio::task::spawn_blocking(move || {
        if !engine::is_venv_ready() {
            engine::setup_venv()?;
        }
        let mut sidecar = engine::LlmEngine::spawn()?;
        let result = sidecar.download_model();
        sidecar.quit();
        result
    })
    .await
    .map_err(|e| format!("Download task panicked: {}", e))
    .and_then(|result| result);
    state.llm_downloading.store(false, Ordering::SeqCst);

    match result {
        Ok(()) => {
            log::info!("Model download complete");
            let _ = app.emit("llm-download-complete", ());

            // Loading is explicit/on first enabled cleanup. A download must not
            // start a Metal process while correction is disabled.

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
