use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;
use tokio::sync::Mutex;

/// Timeout for the HTTP request to the update endpoint.
const UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Managed state for the auto-updater.  Registered via `.manage()` in lib.rs.
pub struct UpdateState {
    /// Whether an update is available (drives tray icon badge).
    pub update_available: AtomicBool,
    /// Version string of the available update (e.g. "0.6.0").
    pub available_version: Mutex<Option<String>>,
    /// Release notes from the GitHub Release body (markdown).
    pub release_notes: Mutex<Option<String>>,
    /// Whether a download+install is currently in progress.
    pub downloading: AtomicBool,
    /// Whether the update has been installed and a restart is pending.
    pub restart_pending: AtomicBool,
    /// Whether a newer AI model is available on HuggingFace.
    pub model_update_available: AtomicBool,
    /// Consecutive model update check failures. Reset on success.
    /// After 3 failures (~12h), model_update_available is cleared to prevent stale indicators.
    pub model_update_consecutive_errors: AtomicU32,
}

impl UpdateState {
    pub fn new() -> Self {
        Self {
            update_available: AtomicBool::new(false),
            available_version: Mutex::new(None),
            release_notes: Mutex::new(None),
            downloading: AtomicBool::new(false),
            restart_pending: AtomicBool::new(false),
            model_update_available: AtomicBool::new(false),
            model_update_consecutive_errors: AtomicU32::new(0),
        }
    }
}

// ---------------------------------------------------------------------------
// Serialisable status (returned to the frontend)
// ---------------------------------------------------------------------------

#[derive(serde::Serialize, Clone)]
pub struct UpdateStatus {
    pub update_available: bool,
    pub version: Option<String>,
    pub release_notes: Option<String>,
    pub downloading: bool,
    pub restart_pending: bool,
    pub model_update_available: bool,
}

/// Download progress payload emitted to the frontend via `"update-download-progress"`.
#[derive(serde::Serialize, Clone)]
struct UpdateDownloadProgress {
    downloaded_bytes: usize,
    total_bytes: Option<u64>,
    /// 0.0 – 1.0 (or 0.0 if total is unknown).
    progress: f64,
}

// ---------------------------------------------------------------------------
// App Translocation detection
// ---------------------------------------------------------------------------

/// macOS App Translocation moves quarantined apps to a random read-only path.
/// The updater cannot replace the .app bundle in that state.
fn is_app_translocated() -> bool {
    if let Ok(exe) = std::env::current_exe() {
        exe.to_string_lossy().contains("/AppTranslocation/")
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Periodic update checker
// ---------------------------------------------------------------------------

/// Spawn a background task that checks for updates periodically.
/// Called once during app setup.
pub fn start_update_checker(app: &AppHandle) {
    if is_app_translocated() {
        log::warn!(
            "App is running from an App Translocation path — auto-update is disabled. \
             Move SottoASR to /Applications for updates."
        );
        return;
    }

    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        // Wait 15 seconds after launch so ASR model loading gets priority.
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;

        loop {
            // Respect the user's auto-check setting (default: true).
            let auto_check = read_auto_check_setting(&handle).unwrap_or(true);
            if auto_check {
                // Panic isolation: wrap each check in tokio::spawn so a panic
                // in one check doesn't kill the entire loop.
                let app_check = tokio::spawn({
                    let h = handle.clone();
                    async move { check_for_update(&h).await }
                });
                let model_check = tokio::spawn({
                    let h = handle.clone();
                    async move { check_for_model_update(&h).await }
                });

                if let Err(e) = app_check.await {
                    log::warn!("App update check panicked: {}", e);
                }
                if let Err(e) = model_check.await {
                    log::warn!("Model update check panicked: {}", e);
                }

                // Refresh tray from canonical state (reads UpdateState directly).
                crate::tray::menu::refresh_tray_from_state(&handle);
            } else {
                log::debug!("Auto-update check disabled by user setting");
            }
            // Sleep 4 hours (active uptime — does not advance during system sleep).
            tokio::time::sleep(std::time::Duration::from_secs(4 * 60 * 60)).await;
        }
    });
}

/// Read the `auto_check_updates` preference from the app settings.
fn read_auto_check_setting(app: &AppHandle) -> Option<bool> {
    let state = app.try_state::<crate::state::AppState>()?;
    // settings is a TokioMutex — use try_lock to avoid blocking the async runtime.
    let settings = state.settings.try_lock().ok()?;
    Some(settings.auto_check_updates)
}

// ---------------------------------------------------------------------------
// Core check logic
// ---------------------------------------------------------------------------

/// Fetch latest.json from the configured endpoint and compare versions.
/// If an update is available, stores it in UpdateState and refreshes the tray.
pub async fn check_for_update(
    app: &AppHandle,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    log::info!("Checking for updates...");

    let updater = app.updater()?;
    let check_result = tokio::time::timeout(UPDATE_CHECK_TIMEOUT, updater.check())
        .await
        .map_err(|_| {
            let msg = "Update check timed out — please check your internet connection";
            log::warn!("{}", msg);
            let _ = app.emit("update-check-error", msg);
            msg
        })?;
    match check_result {
        Ok(Some(update)) => {
            let version = update.version.clone();
            let body = update.body.clone();
            log::info!(
                "Update available: v{} (current: v{})",
                version,
                update.current_version
            );

            let state = app.state::<UpdateState>();
            *state.available_version.lock().await = Some(version.clone());
            *state.release_notes.lock().await = body;
            state.update_available.store(true, Ordering::SeqCst);

            // Emit event to any open frontend windows.
            let _ = app.emit("update-available", &version);
            Ok(())
        }
        Ok(None) => {
            log::info!("App is up to date");

            // Clear any stale state from a previous check (e.g. user already
            // updated to the version we had cached).
            let state = app.state::<UpdateState>();
            *state.available_version.lock().await = None;
            *state.release_notes.lock().await = None;
            state.update_available.store(false, Ordering::SeqCst);

            let _ = app.emit("update-up-to-date", ());
            Ok(())
        }
        Err(e) => {
            log::warn!("Update check error: {}", e);
            let _ = app.emit("update-check-error", e.to_string());
            Err(e.into())
        }
    }
}

/// Check if a newer AI model is available on HuggingFace.
/// Only runs when the LLM cleanup feature is compiled and enabled in settings.
/// Returns Err on persistent failures so the loop can log warnings.
async fn check_for_model_update(
    app: &AppHandle,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Compile-time feature guard (runtime check, matches lib.rs pattern).
    if !crate::llm::engine::is_feature_compiled() {
        if let Some(updater) = app.try_state::<UpdateState>() {
            updater.model_update_available.store(false, Ordering::SeqCst);
        }
        return Ok(());
    }

    // Runtime settings guard — use try_lock to match read_auto_check_setting() pattern.
    let state = app
        .try_state::<crate::state::AppState>()
        .ok_or("AppState not available")?;
    let settings = state
        .settings
        .try_lock()
        .map_err(|_| "Settings lock contended — skipping model check this cycle")?;
    let llm_enabled = settings.llm_cleanup_enabled;
    drop(settings);

    if !llm_enabled {
        if let Some(updater) = app.try_state::<UpdateState>() {
            updater.model_update_available.store(false, Ordering::SeqCst);
            updater.model_update_consecutive_errors.store(0, Ordering::SeqCst);
        }
        return Ok(());
    }

    // Delegate to llm::engine (NOT commands::llm — avoids layer crossing).
    let result = crate::llm::engine::check_model_update(app).await;

    let updater = app.state::<UpdateState>();
    match result {
        Ok(available) => {
            updater.model_update_consecutive_errors.store(0, Ordering::SeqCst);
            updater.model_update_available.store(available, Ordering::SeqCst);
            if available {
                log::info!("AI model update available");
                let _ = tauri::Emitter::emit(app, "llm-update-available", ());
            } else {
                let _ = tauri::Emitter::emit(app, "llm-update-up-to-date", ());
            }
        }
        Err(e) => {
            let errors = updater.model_update_consecutive_errors.fetch_add(1, Ordering::SeqCst) + 1;
            if errors >= 3 {
                updater.model_update_available.store(false, Ordering::SeqCst);
                updater.model_update_consecutive_errors.store(0, Ordering::SeqCst);
                log::warn!("Model update check failed {} times — clearing stale flag: {}", errors, e);
            } else {
                log::debug!("Model update check failed ({}/3): {}", errors, e);
            }
            return Err(e.into());
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tauri commands (called from frontend or tray menu handlers)
// ---------------------------------------------------------------------------

/// Manually trigger an update check.  Returns the version string if an
/// update is available, or null if the app is up to date.
#[tauri::command]
pub async fn check_app_update(
    app: AppHandle,
) -> Result<Option<String>, String> {
    check_for_update(&app)
        .await
        .map_err(|e| e.to_string())?;
    let state = app.state::<UpdateState>();
    let ver = state.available_version.lock().await.clone();
    Ok(ver)
}

/// Download and install the pending update.  Returns the installed version
/// string on success.  The caller should then prompt the user to restart.
#[tauri::command]
pub async fn perform_app_update(app: AppHandle) -> Result<String, String> {
    let state = app.state::<UpdateState>();

    // Prevent concurrent downloads.
    if state.downloading.load(Ordering::SeqCst) {
        return Err("Download already in progress".into());
    }
    state.downloading.store(true, Ordering::SeqCst);

    let result = do_download_and_install(&app).await;

    state.downloading.store(false, Ordering::SeqCst);

    match result {
        Ok(version) => {
            state.update_available.store(false, Ordering::SeqCst);
            state.restart_pending.store(true, Ordering::SeqCst);

            // Refresh tray from canonical state.
            crate::tray::menu::refresh_tray_from_state(&app);

            Ok(version)
        }
        Err(e) => Err(e),
    }
}

/// Return the current update status for the frontend.
#[tauri::command]
pub async fn get_update_status(app: AppHandle) -> Result<UpdateStatus, String> {
    let state = app.state::<UpdateState>();
    let version = state.available_version.lock().await.clone();
    let release_notes = state.release_notes.lock().await.clone();
    let status = UpdateStatus {
        update_available: state.update_available.load(Ordering::SeqCst),
        version,
        release_notes,
        downloading: state.downloading.load(Ordering::SeqCst),
        restart_pending: state.restart_pending.load(Ordering::SeqCst),
        model_update_available: state.model_update_available.load(Ordering::SeqCst),
    };
    Ok(status)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Attempt to download+install.  On failure, re-checks for a fresh URL and
/// retries once (handles stale signed GitHub download URLs).
async fn do_download_and_install(app: &AppHandle) -> Result<String, String> {
    // First, we need to get a fresh update object by calling check().
    // This ensures we always have a valid download URL (not stale).
    let updater = app.updater().map_err(|e| e.to_string())?;
    let check_result = tokio::time::timeout(UPDATE_CHECK_TIMEOUT, updater.check())
        .await
        .map_err(|_| "Update check timed out — please check your internet connection".to_string())?;
    let update = check_result
        .map_err(|e| format!("Update check failed: {}", e))?
        .ok_or_else(|| "No update available".to_string())?;

    let version = update.version.clone();

    let mut downloaded: usize = 0;
    let emit_handle = app.clone();
    update
        .download_and_install(
            move |chunk_length, content_length| {
                downloaded += chunk_length;
                // Emit progress roughly every 512 KB.
                if downloaded % (512 * 1024) < chunk_length || downloaded == chunk_length {
                    let progress = match content_length {
                        Some(total) if total > 0 => downloaded as f64 / total as f64,
                        _ => 0.0,
                    };
                    let _ = emit_handle.emit(
                        "update-download-progress",
                        UpdateDownloadProgress {
                            downloaded_bytes: downloaded,
                            total_bytes: content_length,
                            progress,
                        },
                    );
                }
            },
            || {
                log::info!("Update download complete, installing...");
            },
        )
        .await
        .map_err(|e| format!("Download/install failed: {}", e))?;

    log::info!("Update v{} installed successfully — restart pending", version);
    Ok(version)
}
