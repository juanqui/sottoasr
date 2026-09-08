use std::sync::atomic::Ordering;

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
pub async fn get_model_status(state: State<'_, AppState>) -> Result<ModelStatus, String> {
    let mut status = model::get_model_status();
    status.loaded = state.is_model_loaded.load(std::sync::atomic::Ordering::SeqCst);
    status.initializing = state.asr_initializing.load(Ordering::SeqCst);
    status.error = state.asr_init_error.lock().unwrap_or_else(|e| e.into_inner()).clone();
    Ok(status)
}

/// Check if the app needs onboarding (first-launch setup).
#[tauri::command]
pub async fn needs_onboarding(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(!state.is_model_loaded.load(std::sync::atomic::Ordering::SeqCst))
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
    if state.asr_initializing.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        return Err("Speech recognition is already loading".into());
    }
    *state.asr_init_error.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let _ = app.emit("asr-init-started", serde_json::json!({
        "backend": model::backend_name(),
    }));

    let init_result = crate::asr::engine::with_engine(&state.asr_engine, |engine| engine.init()).await;

    state.asr_initializing.store(false, Ordering::SeqCst);
    *state.asr_init_error.lock().unwrap_or_else(|e| e.into_inner()) = init_result.as_ref().err().cloned();
    match init_result {
        Ok(()) => {
            state.is_model_loaded.store(true, std::sync::atomic::Ordering::SeqCst);
            crate::commands::vocabulary::restore_cached(app.clone());
            app.emit("asr-init-complete", serde_json::json!({
                "backend": model::backend_name(),
            })).map_err(|e| e.to_string())?;
            log::info!("ASR engine ready: {}", model::backend_name());
            Ok(())
        }
        Err(e) => {
            state.is_model_loaded.store(false, Ordering::SeqCst);
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
    let asr_ok = init_asr(app.clone(), state).await.is_ok();

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
