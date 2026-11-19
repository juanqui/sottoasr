use crate::models::ModelStatus;

#[cfg(any(feature = "asr-fluidaudio", feature = "asr-parakeet"))]
use std::path::PathBuf;

#[cfg(feature = "asr-fluidaudio")]
use super::integrity;

#[cfg(feature = "asr-fluidaudio")]
const NO_DATA_DIRECTORY: &str = "Could not find your Application Support folder.";

/// Which models failed their last verification; shown in Settings.
#[cfg(feature = "asr-fluidaudio")]
struct Mismatches {
    speech: bool,
    vocabulary: bool,
}

#[cfg(feature = "asr-fluidaudio")]
static MISMATCHES: std::sync::Mutex<Mismatches> =
    std::sync::Mutex::new(Mismatches { speech: false, vocabulary: false });

/// Every dictation with vocabulary terms reads the vocabulary folder (the SDK
/// loads `tokenizer.json` from disk for each rescoring). Code that moves,
/// deletes or downloads files there holds this exclusively; dictation only
/// tries it and skips the vocabulary boost meanwhile.
#[cfg(feature = "asr-fluidaudio")]
pub(crate) static VOCABULARY_FILES: std::sync::RwLock<()> = std::sync::RwLock::new(());

/// What a confirmed re-download replaced.
#[cfg_attr(not(feature = "asr-fluidaudio"), allow(dead_code))]
pub struct Replacement {
    pub speech: bool,
    pub vocabulary: bool,
    /// The vocabulary model could not be replaced; the speech result stands.
    pub vocabulary_error: Option<String>,
}

/// Payload of `model-download-progress`.
#[cfg(any(feature = "asr-fluidaudio", feature = "asr-parakeet"))]
#[derive(Clone, serde::Serialize)]
pub struct DownloadProgress {
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
}

/// SDK 0.15.6 uses `Repo.folderName`, which differs from the remote repo slug.
/// The legacy `-coreml` directory is left untouched; it is not this SDK's cache.
#[cfg(feature = "asr-fluidaudio")]
pub(crate) fn fluidaudio_model_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|dir| dir.join("FluidAudio/Models").join(&integrity::speech().directory))
}

#[cfg(feature = "asr-fluidaudio")]
pub(crate) fn vocabulary_model_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|dir| dir.join("FluidAudio/Models").join(&integrity::vocabulary().directory))
}

/// Downloads missing speech model files at the pinned revision and verifies
/// them against the manifest. Never takes the engine lock; loading follows.
pub fn prepare_speech_model() -> Result<(), String> {
    #[cfg(feature = "asr-fluidaudio")]
    {
        let directory = fluidaudio_model_dir().ok_or(NO_DATA_DIRECTORY)?;
        let report = integrity::speech().prepare(&directory, || download_speech_model(&directory),
            &fluidaudio_rs::sha256_file)?;
        set_speech_mismatch(report.has_mismatch());
        if report.blocks_loading() {
            return Err(if report.missing.is_empty() {
                "The downloaded speech model failed verification, so it was not loaded. Try again to download it again."
            } else {
                "The speech model download is incomplete. Check your internet connection and try again."
            }.into());
        }
        Ok(())
    }

    #[cfg(not(feature = "asr-fluidaudio"))]
    { Ok(()) }
}

/// User-confirmed re-download of the model files that fail the manifest: the
/// speech model, then the vocabulary model if the user prepared it. Never
/// takes the engine lock; the caller reloads what was replaced.
pub fn replace_failing_models() -> Result<Replacement, String> {
    #[cfg(feature = "asr-fluidaudio")]
    {
        let speech_directory = fluidaudio_model_dir().ok_or(NO_DATA_DIRECTORY)?;
        let speech = integrity::speech().replace_failing(&speech_directory, "speech model",
            || download_speech_model(&speech_directory), &fluidaudio_rs::sha256_file)?;
        set_speech_mismatch(false);

        let vocabulary_directory = vocabulary_model_dir().ok_or(NO_DATA_DIRECTORY)?;
        let manifest = integrity::vocabulary();
        let (vocabulary, vocabulary_error) = if manifest.is_present(&vocabulary_directory) {
            // Waits for a dictation that is reading the folder, then keeps
            // later dictations off it until the files are back or replaced.
            let _files = VOCABULARY_FILES.write().unwrap_or_else(|error| error.into_inner());
            let replaced = manifest.replace_failing(&vocabulary_directory, "vocabulary model", || {
                fluidaudio_rs::download_vocabulary_model(&vocabulary_directory, true).map_err(|error| {
                    format!("Could not download the vocabulary model ({error}). Check your internet connection and try again.")
                })
            }, &fluidaudio_rs::sha256_file);
            match replaced {
                Ok(replaced) => {
                    set_vocabulary_mismatch(false);
                    (replaced, None)
                }
                Err(error) => (false, Some(error)),
            }
        } else {
            (false, None)
        };
        Ok(Replacement { speech, vocabulary, vocabulary_error })
    }

    #[cfg(not(feature = "asr-fluidaudio"))]
    { Err("Re-download is available only for the CoreML speech model.".into()) }
}

/// Manifest bytes present in the speech model cache, for download progress.
#[cfg(feature = "asr-fluidaudio")]
pub fn speech_download_progress() -> Option<DownloadProgress> {
    let manifest = integrity::speech();
    let directory = fluidaudio_model_dir()?;
    Some(DownloadProgress {
        downloaded_bytes: manifest.downloaded_bytes(&directory),
        total_bytes: manifest.total_bytes(),
    })
}

#[cfg(feature = "asr-fluidaudio")]
fn download_speech_model(directory: &std::path::Path) -> Result<(), String> {
    fluidaudio_rs::download_speech_model(directory).map_err(|error| {
        format!("Could not download the speech model ({error}). Check your internet connection and try again.")
    })
}

#[cfg(feature = "asr-fluidaudio")]
fn set_speech_mismatch(mismatch: bool) {
    MISMATCHES.lock().unwrap_or_else(|error| error.into_inner()).speech = mismatch;
}

#[cfg(feature = "asr-fluidaudio")]
pub(crate) fn set_vocabulary_mismatch(mismatch: bool) {
    MISMATCHES.lock().unwrap_or_else(|error| error.into_inner()).vocabulary = mismatch;
}

/// One message for both models, so Settings shows a single re-download offer.
#[cfg(feature = "asr-fluidaudio")]
fn integrity_warning() -> Option<String> {
    let mismatches = MISMATCHES.lock().unwrap_or_else(|error| error.into_inner());
    let models = match (mismatches.speech, mismatches.vocabulary) {
        (true, true) => "speech and vocabulary model",
        (true, false) => "speech model",
        (false, true) => "vocabulary model",
        (false, false) => return None,
    };
    Some(format!("Some {models} files differ from the verified release. Re-download the model files to replace them."))
}

#[cfg(feature = "asr-fluidaudio")]
fn fluidaudio_artifacts_available(directory: &std::path::Path) -> bool {
    // Match AsrModels.modelsExist(version: .v3, encoderPrecision: .int8).
    // Actual CoreML loading remains the authority for model validity.
    ["Preprocessor.mlmodelc", "Encoder.mlmodelc", "Decoder.mlmodelc", "JointDecisionv3.mlmodelc"]
        .iter()
        .all(|name| directory.join(name).is_dir())
        && directory.join("parakeet_vocab.json").metadata()
            .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
}

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
        fluidaudio_model_dir().is_some_and(|dir| fluidaudio_artifacts_available(&dir))
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
            initializing: false,
            error: None,
            path: if available {
                fluidaudio_model_dir().map(|dir| dir.to_string_lossy().into_owned())
            } else {
                None
            },
            name: "parakeet-tdt-0.6b-v3 (CoreML)".to_string(),
            size_bytes: None, // FluidAudio manages this
            integrity_warning: integrity_warning(),
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
            initializing: false,
            error: None,
            path: None,
            name: "none".to_string(),
            size_bytes: None,
            integrity_warning: None,
        }
    }
}

#[cfg(all(test, feature = "asr-fluidaudio"))]
mod tests {
    use super::*;

    fn populate(directory: &std::path::Path) {
        for name in ["Preprocessor.mlmodelc", "Encoder.mlmodelc", "Decoder.mlmodelc", "JointDecisionv3.mlmodelc"] {
            std::fs::create_dir_all(directory.join(name)).unwrap();
        }
        std::fs::write(directory.join("parakeet_vocab.json"), b"{\"0\":\"test\"}").unwrap();
    }

    #[test]
    fn readiness_uses_current_sdk_folder_and_preserves_legacy_artifacts() {
        let temporary = tempfile::tempdir().unwrap();
        let legacy = temporary.path().join("parakeet-tdt-0.6b-v3-coreml");
        let current = temporary.path().join("parakeet-tdt-0.6b-v3");
        populate(&legacy);
        assert!(!fluidaudio_artifacts_available(&current));
        populate(&current);
        assert!(fluidaudio_artifacts_available(&current));
        assert!(fluidaudio_artifacts_available(&legacy));
        assert_eq!(fluidaudio_model_dir().unwrap().file_name().unwrap(), "parakeet-tdt-0.6b-v3");
    }

    #[test]
    fn partial_cache_is_not_reported_as_downloaded() {
        let temporary = tempfile::tempdir().unwrap();
        assert!(!fluidaudio_artifacts_available(temporary.path()));
        populate(temporary.path());
        std::fs::write(temporary.path().join("parakeet_vocab.json"), b"").unwrap();
        assert!(!fluidaudio_artifacts_available(temporary.path()));
        std::fs::write(temporary.path().join("parakeet_vocab.json"), b"{}").unwrap();
        std::fs::remove_dir(temporary.path().join("JointDecisionv3.mlmodelc")).unwrap();
        assert!(!fluidaudio_artifacts_available(temporary.path()));
    }
}

// ─── parakeet-rs model management (only compiled with asr-parakeet) ───

#[cfg(feature = "asr-parakeet")]
const PARAKEET_MODEL_NAME: &str = "parakeet-tdt-0.6b-v3";

/// Pinned commit of `istupakov/parakeet-tdt-0.6b-v3-onnx` (resolved 2026-11-19).
/// Downloads read this commit, never the mutable `main` branch.
#[cfg(feature = "asr-parakeet")]
const HF_BASE_URL: &str =
    "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce";

/// The files parakeet-rs loads, with their size and SHA-256 at the pinned
/// commit: the Git LFS object IDs for the models, and the hash of `vocab.txt`
/// whose Git blob ID matches the commit's tree.
#[cfg(feature = "asr-parakeet")]
const PARAKEET_MODEL_FILES: &[(&str, u64, &str)] = &[
    ("encoder-model.int8.onnx", 652_183_999, "6139d2fa7e1b086097b277c7149725edbab89cc7c7ae64b23c741be4055aff09"),
    ("decoder_joint-model.int8.onnx", 18_202_004, "eea7483ee3d1a30375daedc8ed83e3960c91b098812127a0d99d1c8977667a70"),
    ("vocab.txt", 93_939, "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d"),
];

#[cfg(feature = "asr-parakeet")]
pub fn get_model_dir() -> Result<PathBuf, String> {
    let data_dir = dirs::data_dir()
        .ok_or("Could not determine data directory")?;
    Ok(data_dir.join("com.sottoasr.app").join("models").join(PARAKEET_MODEL_NAME))
}

#[cfg(feature = "asr-parakeet")]
fn is_parakeet_model_downloaded() -> bool {
    let model_dir = match get_model_dir() {
        Ok(dir) => dir,
        Err(_) => return false,
    };
    PARAKEET_MODEL_FILES.iter().all(|(filename, _, _)| {
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
        initializing: false,
        error: None,
        path: if downloaded { Some(model_dir.to_string_lossy().to_string()) } else { None },
        name: format!("{} (ONNX INT8)", PARAKEET_MODEL_NAME),
        size_bytes: if downloaded {
            Some(PARAKEET_MODEL_FILES.iter().map(|(f, _, _)| {
                model_dir.join(f).metadata().map(|m| m.len()).unwrap_or(0)
            }).sum())
        } else {
            None
        },
        integrity_warning: None,
    }
}

#[cfg(feature = "asr-parakeet")]
pub async fn download_parakeet_model(app: tauri::AppHandle) -> Result<(), String> {
    use futures_util::StreamExt;
    use tauri::Emitter;

    let model_dir = get_model_dir()?;
    std::fs::create_dir_all(&model_dir)
        .map_err(|e| format!("Failed to create model directory: {}", e))?;

    let total_bytes: u64 = PARAKEET_MODEL_FILES.iter().map(|(_, size, _)| size).sum();
    let mut cumulative: u64 = 0;

    app.emit("model-download-started", serde_json::json!({
        "total_bytes": total_bytes,
        "file_count": PARAKEET_MODEL_FILES.len(),
    })).map_err(|e| e.to_string())?;

    let client = reqwest::Client::builder()
        .user_agent("SottoASR/0.1.0")
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    for &(filename, expected_size, sha256) in PARAKEET_MODEL_FILES {
        let file_path = model_dir.join(filename);

        // Skip if already downloaded
        if file_path.exists() && file_path.metadata().map(|m| m.len() == expected_size).unwrap_or(false) {
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

            if last_emit.elapsed() >= std::time::Duration::from_millis(500) {
                app.emit("model-download-progress", DownloadProgress {
                    downloaded_bytes: cumulative + file_dl,
                    total_bytes,
                }).map_err(|e| e.to_string())?;
                last_emit = std::time::Instant::now();
            }
        }

        file.flush().await.map_err(|e| e.to_string())?;
        drop(file);

        tokio::task::spawn_blocking(move || accept_download(&temp_path, &file_path, filename, expected_size, sha256))
            .await
            .map_err(|e| format!("Could not check {}: {}", filename, e))??;
        cumulative += file_dl;
        log::info!("Downloaded {} ({} bytes)", filename, file_dl);
    }

    app.emit("model-download-complete", ()).map_err(|e| e.to_string())?;
    log::info!("All parakeet-rs model files downloaded to {:?}", model_dir);
    Ok(())
}

/// Accepts a finished download only if it is the pinned file. A short or
/// altered file is removed and reported, so the next attempt starts over.
#[cfg(feature = "asr-parakeet")]
fn accept_download(
    staging: &std::path::Path,
    destination: &std::path::Path,
    name: &str,
    size: u64,
    sha256: &str,
) -> Result<(), String> {
    let checked = std::fs::metadata(staging).map_err(|e| e.to_string()).and_then(|metadata| {
        // The size check reports a truncated download without hashing it.
        if metadata.len() != size {
            return Err(format!("The download of {name} is incomplete ({} of {size} bytes).", metadata.len()));
        }
        if sha256_file(staging)? != sha256 {
            return Err(format!("The download of {name} does not match the pinned model release."));
        }
        Ok(())
    });
    if let Err(error) = checked {
        let _ = std::fs::remove_file(staging);
        return Err(format!("{error} The partial file was removed; try the download again."));
    }
    std::fs::rename(staging, destination).map_err(|e| format!("Could not save {name}: {e}"))
}

#[cfg(feature = "asr-parakeet")]
fn sha256_file(path: &std::path::Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let unreadable = |error: std::io::Error| format!("Could not check {}: {error}", path.display());
    let mut file = std::fs::File::open(path).map_err(unreadable)?;
    let mut hasher = Sha256::new();
    // Streams the file through the hasher in fixed-size chunks.
    std::io::copy(&mut file, &mut hasher).map_err(unreadable)?;
    Ok(hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(all(test, feature = "asr-parakeet"))]
mod parakeet_tests {
    use super::*;

    // SHA-256 of b"pinned model bytes".
    const PINNED_SHA256: &str = "7826512858078d3d070641414fc406bd3d4f82b60d83968427eb3e5dd7d1377e";

    #[test]
    fn download_with_the_wrong_hash_is_rejected_and_removed() {
        let directory = tempfile::tempdir().unwrap();
        let staging = directory.path().join("vocab.download");
        let destination = directory.path().join("vocab.txt");

        // Same length as the pinned bytes, so only the digest rejects it.
        std::fs::write(&staging, b"pinned model BYTES").unwrap();
        let error = accept_download(&staging, &destination, "vocab.txt", 18, PINNED_SHA256).unwrap_err();
        assert!(error.contains("does not match"), "{error}");
        assert!(!staging.exists() && !destination.exists());

        std::fs::write(&staging, b"pinned").unwrap();
        let error = accept_download(&staging, &destination, "vocab.txt", 18, PINNED_SHA256).unwrap_err();
        assert!(error.contains("incomplete"), "{error}");
        assert!(!staging.exists() && !destination.exists());

        std::fs::write(&staging, b"pinned model bytes").unwrap();
        accept_download(&staging, &destination, "vocab.txt", 18, PINNED_SHA256).unwrap();
        assert!(!staging.exists());
        assert_eq!(std::fs::read(&destination).unwrap(), b"pinned model bytes");
    }

    #[test]
    fn downloads_read_the_pinned_commit() {
        assert!(HF_BASE_URL.ends_with("/resolve/8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce"));
        assert!(PARAKEET_MODEL_FILES.iter().all(|(_, size, sha256)| *size > 0 && sha256.len() == 64));
    }
}
