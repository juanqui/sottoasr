use crate::models::ModelStatus;

/// Get the current ASR backend name at compile time.
pub fn backend_name() -> &'static str {
    #[cfg(feature = "asr-fluidaudio")]
    { "FluidAudio (CoreML/ANE)" }

    #[cfg(all(feature = "asr-parakeet", not(feature = "asr-fluidaudio")))]
    { "parakeet-rs (ONNX/CPU)" }

    #[cfg(not(any(feature = "asr-fluidaudio", feature = "asr-parakeet")))]
    { "none" }
}

/// Check if models are available for the current backend.
pub fn is_model_available() -> bool {
    #[cfg(feature = "asr-fluidaudio")]
    {
        // FluidAudio stores models at ~/Library/Application Support/FluidAudio/Models/
        let model_dir = dirs::data_dir()
            .unwrap_or_default()
            .join("FluidAudio")
            .join("Models")
            .join("parakeet-tdt-0.6b-v3-coreml");
        model_dir.exists()
    }

    #[cfg(all(feature = "asr-parakeet", not(feature = "asr-fluidaudio")))]
    {
        is_parakeet_model_downloaded()
    }

    #[cfg(not(any(feature = "asr-fluidaudio", feature = "asr-parakeet")))]
    { false }
}

/// Get model status for the current backend.
pub fn get_model_status() -> ModelStatus {
    #[cfg(feature = "asr-fluidaudio")]
    {
        let available = is_model_available();
        ModelStatus {
            downloaded: available,
            loaded: false,
            path: if available {
                Some(dirs::data_dir()
                    .unwrap_or_default()
                    .join("FluidAudio")
                    .join("Models")
                    .to_string_lossy()
                    .to_string())
            } else {
                None
            },
            name: "parakeet-tdt-0.6b-v3 (CoreML)".to_string(),
            size_bytes: None, // FluidAudio manages this
        }
    }

    #[cfg(all(feature = "asr-parakeet", not(feature = "asr-fluidaudio")))]
    {
        get_parakeet_model_status()
    }

    #[cfg(not(any(feature = "asr-fluidaudio", feature = "asr-parakeet")))]
    {
        ModelStatus {
            downloaded: false,
            loaded: false,
            path: None,
            name: "none".to_string(),
            size_bytes: None,
        }
    }
}

// ─── parakeet-rs model management (only compiled with asr-parakeet) ───

#[cfg(feature = "asr-parakeet")]
const PARAKEET_MODEL_NAME: &str = "parakeet-tdt-0.6b-v3";

#[cfg(feature = "asr-parakeet")]
const HF_BASE_URL: &str = "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main";

#[cfg(feature = "asr-parakeet")]
const PARAKEET_MODEL_FILES: &[(&str, u64)] = &[
    ("encoder-model.int8.onnx", 652_183_999),
    ("decoder_joint-model.int8.onnx", 18_202_004),
    ("vocab.txt", 93_939),
];

#[cfg(feature = "asr-parakeet")]
pub fn get_model_dir() -> Result<PathBuf, String> {
    let data_dir = dirs::data_dir()
        .ok_or("Could not determine data directory")?;
    Ok(data_dir.join("com.sotto.app").join("models").join(PARAKEET_MODEL_NAME))
}

#[cfg(feature = "asr-parakeet")]
fn is_parakeet_model_downloaded() -> bool {
    let model_dir = match get_model_dir() {
        Ok(dir) => dir,
        Err(_) => return false,
    };
    PARAKEET_MODEL_FILES.iter().all(|(filename, _)| {
        let path = model_dir.join(filename);
        path.exists() && path.metadata().map(|m| m.len() > 0).unwrap_or(false)
    })
}

#[cfg(feature = "asr-parakeet")]
fn get_parakeet_model_status() -> ModelStatus {
    let model_dir = get_model_dir().unwrap_or_default();
    let downloaded = is_parakeet_model_downloaded();
    ModelStatus {
        downloaded,
        loaded: false,
        path: if downloaded { Some(model_dir.to_string_lossy().to_string()) } else { None },
        name: format!("{} (ONNX INT8)", PARAKEET_MODEL_NAME),
        size_bytes: if downloaded {
            Some(PARAKEET_MODEL_FILES.iter().map(|(f, _)| {
                model_dir.join(f).metadata().map(|m| m.len()).unwrap_or(0)
            }).sum())
        } else {
            None
        },
    }
}

#[cfg(feature = "asr-parakeet")]
pub async fn download_parakeet_model(app: tauri::AppHandle) -> Result<(), String> {
    use futures_util::StreamExt;
    use tauri::Emitter;

    let model_dir = get_model_dir()?;
    std::fs::create_dir_all(&model_dir)
        .map_err(|e| format!("Failed to create model directory: {}", e))?;

    let total_bytes: u64 = PARAKEET_MODEL_FILES.iter().map(|(_, s)| s).sum();
    let mut cumulative: u64 = 0;

    app.emit("model-download-started", serde_json::json!({
        "total_bytes": total_bytes,
        "file_count": PARAKEET_MODEL_FILES.len(),
    })).map_err(|e| e.to_string())?;

    let client = reqwest::Client::builder()
        .user_agent("Sotto/0.1.0")
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    for (filename, expected_size) in PARAKEET_MODEL_FILES {
        let file_path = model_dir.join(filename);

        // Skip if already downloaded
        if file_path.exists() && file_path.metadata().map(|m| m.len() == *expected_size).unwrap_or(false) {
            log::info!("Skipping {} — already downloaded", filename);
            cumulative += expected_size;
            continue;
        }

        let url = format!("{}/{}", HF_BASE_URL, filename);
        log::info!("Downloading {} from {}", filename, url);

        let response = client.get(&url).send().await
            .map_err(|e| format!("Download failed for {}: {}", filename, e))?;

        if !response.status().is_success() {
            return Err(format!("HTTP {} for {}", response.status(), filename));
        }

        let content_length = response.content_length().unwrap_or(*expected_size);
        let temp_path = file_path.with_extension("download");
        let mut file = tokio::fs::File::create(&temp_path).await
            .map_err(|e| format!("Failed to create {}: {}", filename, e))?;

        let mut stream = response.bytes_stream();
        let mut file_dl: u64 = 0;
        let mut last_emit = std::time::Instant::now();

        use tokio::io::AsyncWriteExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("Stream error: {}", e))?;
            file.write_all(&chunk).await.map_err(|e| format!("Write error: {}", e))?;
            file_dl += chunk.len() as u64;

            if last_emit.elapsed() >= std::time::Duration::from_millis(100) {
                app.emit("model-download-progress", serde_json::json!({
                    "downloaded_bytes": cumulative + file_dl,
                    "total_bytes": total_bytes,
                    "current_file": filename,
                    "progress": (cumulative + file_dl) as f64 / total_bytes as f64,
                    "status": "downloading",
                })).map_err(|e| e.to_string())?;
                last_emit = std::time::Instant::now();
            }
        }

        file.flush().await.map_err(|e| e.to_string())?;
        drop(file);

        // Verify and rename
        let actual = tokio::fs::metadata(&temp_path).await.map_err(|e| e.to_string())?.len();
        if actual != *expected_size {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(format!("Size mismatch for {}: {} vs {}", filename, actual, expected_size));
        }
        tokio::fs::rename(&temp_path, &file_path).await.map_err(|e| e.to_string())?;
        cumulative += file_dl;
        log::info!("Downloaded {} ({} bytes)", filename, file_dl);
    }

    app.emit("model-download-complete", ()).map_err(|e| e.to_string())?;
    log::info!("All parakeet-rs model files downloaded to {:?}", model_dir);
    Ok(())
}
