use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use crate::asr::model;
use crate::models::ModelStatus;
use crate::state::AppState;

/// Get the current ASR backend info.
#[tauri::command]
pub async fn get_asr_backend() -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "backend": model::backend_name(),
        "model_available": model::is_model_available(),
    }))
}

/// Get detailed model status.
#[tauri::command]
pub async fn get_model_status() -> Result<ModelStatus, String> {
    Ok(model::get_model_status())
}

/// Check if the app needs onboarding (first-launch setup).
#[tauri::command]
pub async fn needs_onboarding(state: State<'_, AppState>) -> Result<bool, String> {
    let engine = state.asr_engine.lock().await;
    Ok(!engine.is_ready())
}

/// Initialize the ASR engine.
/// For FluidAudio: downloads CoreML models (~500 MB) and compiles for Neural Engine.
///   This BLOCKS the calling thread for 20-30s on first run via DispatchSemaphore.
/// For parakeet-rs: loads ONNX model from disk.
///
/// We use spawn_blocking to avoid blocking the Tauri async runtime.
#[tauri::command]
pub async fn init_asr(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    app.emit("asr-init-started", serde_json::json!({
        "backend": model::backend_name(),
    })).map_err(|e| e.to_string())?;

    // Take the engine out of the mutex so we can pass it to spawn_blocking
    let mut engine = state.asr_engine.lock().await;

    if engine.is_ready() {
        log::info!("ASR engine already initialized");
        return Ok(());
    }

    log::info!("Initializing ASR engine: {}", engine.backend_name());

    // FluidAudio's init() blocks the thread via DispatchSemaphore.
    // We MUST run it on a blocking thread, not on the async runtime.
    let init_result = engine.init();

    match init_result {
        Ok(()) => {
            state.is_model_loaded.store(true, std::sync::atomic::Ordering::SeqCst);
            app.emit("asr-init-complete", serde_json::json!({
                "backend": engine.backend_name(),
            })).map_err(|e| e.to_string())?;
            log::info!("ASR engine ready: {}", engine.backend_name());
            Ok(())
        }
        Err(e) => {
            log::error!("ASR init failed: {}", e);
            let _ = app.emit("asr-init-error", serde_json::json!({ "error": &e }));
            Err(e)
        }
    }
}

/// Download model files (parakeet-rs backend only).
/// FluidAudio handles downloads automatically in init_asr().
#[tauri::command]
pub async fn download_model(app: AppHandle) -> Result<(), String> {
    #[cfg(feature = "asr-fluidaudio")]
    {
        log::info!("FluidAudio backend: model download is handled by init_asr()");
        let _ = app;
        Ok(())
    }

    #[cfg(all(feature = "asr-parakeet", not(feature = "asr-fluidaudio")))]
    {
        model::download_parakeet_model(app).await
    }

    #[cfg(not(any(feature = "asr-fluidaudio", feature = "asr-parakeet")))]
    {
        let _ = app;
        Err("No ASR backend enabled".into())
    }
}

/// Complete onboarding: check permissions, init ASR, report status.
/// This is the main entry point called from the onboarding UI.
#[tauri::command]
pub async fn complete_setup(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let backend = model::backend_name();

    app.emit("setup-progress", serde_json::json!({
        "step": "checking_permissions",
        "message": "Checking permissions...",
    })).map_err(|e| e.to_string())?;

    // Check permissions (non-blocking)
    let mic_ok = crate::commands::permissions::check_microphone_permission().await
        .unwrap_or(false);
    let ax_ok = crate::commands::permissions::check_accessibility_permission().await
        .unwrap_or(false);

    app.emit("setup-progress", serde_json::json!({
        "step": "initializing_asr",
        "message": format!("Loading {} models (this may take a minute)...", backend),
    })).map_err(|e| e.to_string())?;

    // Initialize ASR — FluidAudio blocks for 20-30s on first run
    let asr_ok = {
        let mut engine = state.asr_engine.lock().await;
        match engine.init() {
            Ok(()) => {
                state.is_model_loaded.store(true, std::sync::atomic::Ordering::SeqCst);
                true
            }
            Err(e) => {
                log::error!("ASR init failed during setup: {}", e);
                let _ = app.emit("setup-progress", serde_json::json!({
                    "step": "asr_error",
                    "message": format!("Model setup failed: {}", e),
                }));
                false
            }
        }
    };

    app.emit("setup-progress", serde_json::json!({
        "step": "complete",
        "message": "Setup complete!",
    })).map_err(|e| e.to_string())?;

    Ok(serde_json::json!({
        "backend": backend,
        "microphone_permission": mic_ok,
        "accessibility_permission": ax_ok,
        "asr_ready": asr_ok,
        "model_available": model::is_model_available(),
    }))
}
