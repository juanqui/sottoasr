use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager};

use crate::llm::engine;
use crate::state::AppState;

/// How often the model cache is measured while a download runs.
const DOWNLOAD_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// A download whose cache stops growing for this long is stopped (R-07).
/// There is no total deadline: a slow but moving download always finishes.
const DOWNLOAD_STALL_LIMIT: Duration = Duration::from_secs(120);

/// Fails a download whose cache stops growing. The caller passes the time in,
/// so tests drive it without waiting.
pub(crate) struct DownloadStallWatch {
    largest: u64,
    last_growth: Instant,
    limit: Duration,
}

impl DownloadStallWatch {
    pub(crate) fn new(size: u64, now: Instant, limit: Duration) -> Self {
        Self {
            largest: size,
            last_growth: now,
            limit,
        }
    }

    pub(crate) fn observe(&mut self, size: u64, now: Instant) -> Result<(), String> {
        if size > self.largest {
            self.largest = size;
            self.last_growth = now;
            return Ok(());
        }
        if now.saturating_duration_since(self.last_growth) >= self.limit {
            return Err(format!(
                "The cleanup model download made no progress for {} seconds. Check your internet connection and try again.",
                self.limit.as_secs()
            ));
        }
        Ok(())
    }
}

/// Clears `llm_downloading` on every exit path.
struct Downloading<'a>(&'a AtomicBool);

impl<'a> Downloading<'a> {
    fn start(flag: &'a AtomicBool) -> Self {
        flag.store(true, Ordering::SeqCst);
        Self(flag)
    }
}

impl Drop for Downloading<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Bytes stored under `dir`. Links are not followed: snapshot entries link to
/// blobs that are already counted.
fn cache_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => cache_size(&entry.path()),
            Ok(kind) if kind.is_file() => entry.metadata().map_or(0, |metadata| metadata.len()),
            _ => 0,
        })
        .sum()
}

/// Download and verify the SottoASR cleanup model in a separate sidecar
/// process. Emits `llm-download-progress` every poll and stops the download
/// after `DOWNLOAD_STALL_LIMIT` without growth.
pub async fn download_model(app: &AppHandle) -> Result<(), String> {
    let config = engine::model_config();
    let state = app.state::<AppState>();
    let _downloading = Downloading::start(&state.llm_downloading);
    let cache = engine::model_cache_dir()?;
    let total_bytes = config.download_size_mb * 1_000_000;
    log::info!("Starting model download via sidecar: {}...", config.id);

    let handle = app.clone();
    let result = tokio::task::spawn_blocking(move || {
        let emit = |downloaded: u64| {
            let _ = handle.emit(
                "llm-download-progress",
                serde_json::json!({
                    "downloaded_bytes": downloaded.min(total_bytes),
                    "total_bytes": total_bytes,
                }),
            );
        };
        let mut sidecar = engine::LlmEngine::spawn(engine::SidecarRole::Download)?;
        let started = cache_size(&cache);
        emit(started);
        let mut watch = DownloadStallWatch::new(started, Instant::now(), DOWNLOAD_STALL_LIMIT);
        let result = sidecar.download_model(DOWNLOAD_POLL_INTERVAL, || {
            let size = cache_size(&cache);
            emit(size);
            watch.observe(size, Instant::now())
        });
        sidecar.quit();
        result
    })
    .await
    .map_err(|e| format!("Download task panicked: {}", e))
    .and_then(|result| result);

    // Loading is explicit/on first enabled cleanup. A download must not
    // start a Metal process while correction is disabled.
    match &result {
        Ok(()) => log::info!("Model download complete"),
        Err(e) => log::error!("Model download failed: {}", e),
    }
    result
}

/// Delete downloaded model files from the HuggingFace cache.
///
/// HuggingFace's default cache lives at `~/.cache/huggingface/hub/` — NOT at
/// `~/Library/Caches/huggingface/hub/` which is what `dirs::cache_dir()`
/// resolves to on macOS. The old implementation deleted the wrong path (or
/// nothing at all), leaving orphaned weights on disk.
pub fn delete_model() -> Result<(), String> {
    let hf_cache = engine::model_cache_dir()?;
    if hf_cache.exists() {
        std::fs::remove_dir_all(&hf_cache)
            .map_err(|e| format!("Failed to delete model cache: {}", e))?;
        log::info!("Deleted model cache at {:?}", hf_cache);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn growth_resets_the_stall_clock_and_only_a_full_limit_without_growth_fails() {
        let limit = Duration::from_secs(120);
        let start = Instant::now();
        let at = |seconds| start + Duration::from_secs(seconds);
        let mut watch = DownloadStallWatch::new(0, start, limit);
        assert!(watch.observe(0, at(119)).is_ok());
        assert!(watch.observe(10, at(200)).is_ok(), "growth resets the clock");
        assert!(watch.observe(10, at(319)).is_ok());
        assert!(watch.observe(5, at(320)).is_err(), "a shrinking cache is not growth");
    }

    #[test]
    fn cache_size_counts_files_once_without_following_links() {
        let directory = tempfile::TempDir::new().unwrap();
        let blobs = directory.path().join("blobs");
        let snapshot = directory.path().join("snapshots").join("rev");
        std::fs::create_dir_all(&blobs).unwrap();
        std::fs::create_dir_all(&snapshot).unwrap();
        std::fs::write(blobs.join("weights.incomplete"), vec![0u8; 4096]).unwrap();
        std::os::unix::fs::symlink(blobs.join("weights.incomplete"), snapshot.join("model.safetensors"))
            .unwrap();
        assert_eq!(cache_size(directory.path()), 4096);
        assert_eq!(cache_size(&directory.path().join("absent")), 0);
    }
}
