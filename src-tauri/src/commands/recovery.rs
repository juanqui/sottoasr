//! Restart recovery uses recording IDs, never webview-supplied filesystem paths.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use tauri::{Emitter, Manager, State};

use crate::audio::recovery::{recording_id, recordings_dir};
use crate::models::{AppStateEnum, LlmCleanupStatus, Transcription};
use crate::state::AppState;

static RECOVERY_IO: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn legacy_directory() -> PathBuf {
    #[cfg(not(test))]
    {
        std::env::temp_dir()
    }
    #[cfg(test)]
    {
        static DIRECTORY: std::sync::LazyLock<tempfile::TempDir> =
            std::sync::LazyLock::new(|| tempfile::tempdir().expect("isolated legacy recordings"));
        DIRECTORY.path().to_path_buf()
    }
}

/// The advisory lock prevents two app instances from sharing capture recovery.
pub struct RecoverySession {
    marker: Option<File>,
    notice: Option<String>,
    pub(crate) error: Option<String>,
}

impl RecoverySession {
    pub(crate) fn start() -> Self {
        match recordings_dir().and_then(|directory| Self::start_in(&directory)) {
            Ok(session) => session,
            Err(error) => {
                log::error!("Recording recovery unavailable: {error}");
                Self { marker: None, notice: Some(format!("Recording recovery is unavailable: {error}. Recordings are disabled until this is resolved.")), error: Some(error) }
            }
        }
    }

    fn start_in(directory: &Path) -> Result<Self, String> {
        use std::io::{Read, Seek, SeekFrom, Write};
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut marker = options
            .open(directory.join("session.state"))
            .map_err(|e| format!("Could not track recording recovery: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            // SAFETY: marker owns a live file descriptor for this session.
            if unsafe { libc::flock(marker.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                return Err(
                    "Another SottoASR instance already owns the recording directory.".into(),
                );
            }
        }
        let mut previous = Vec::new();
        Read::by_ref(&mut marker)
            .take(32)
            .read_to_end(&mut previous)
            .map_err(|e| e.to_string())?;
        let notice = (!previous.is_empty() && previous != b"clean\n")
            .then(|| "SottoASR did not shut down cleanly. Review the saved recordings below. If no recording is listed, no pending audio file was found.".into());
        marker.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        marker.write_all(b"running\n").map_err(|e| e.to_string())?;
        marker.set_len(8).map_err(|e| e.to_string())?;
        marker.sync_all().map_err(|e| e.to_string())?;
        File::open(directory)
            .and_then(|file| file.sync_all())
            .map_err(|e| format!("Could not sync recovery session: {e}"))?;
        Ok(Self { marker: Some(marker), notice, error: None })
    }

    pub(crate) fn clean_exit(&self) {
        use std::io::{Seek, SeekFrom, Write};
        let Some(mut marker) = self.marker.as_ref() else { return; };
        let result = marker
            .seek(SeekFrom::Start(0))
            .and_then(|_| marker.write_all(b"clean\n"))
            .and_then(|_| marker.set_len(6))
            .and_then(|_| marker.sync_all());
        if let Err(error) = result {
            log::error!("Could not acknowledge clean shutdown: {error}");
        }
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            // Restart can launch the replacement before Tauri drops managed state.
            // SAFETY: self.marker remains a live owned descriptor through exit.
            unsafe {
                libc::flock(marker.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}

#[tauri::command]
pub fn get_recovery_notice(session: State<'_, RecoverySession>) -> Option<String> {
    session.notice.clone()
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct RecoveryRecording {
    pub id: String,
    pub audio_path: String,
    pub created_at: DateTime<Utc>,
    pub duration_ms: Option<u64>,
    pub size_bytes: u64,
    pub error: Option<String>,
}

/// Reject symlinks and paths outside the designated recording directory.
pub(crate) fn checked_audio_path(path: &Path, directory: &Path) -> Result<PathBuf, String> {
    recording_id(path)?;
    let metadata =
        fs::symlink_metadata(path).map_err(|e| format!("Recording is unavailable: {e}"))?;
    if !metadata.file_type().is_file() {
        return Err("Recording must be a regular file".into());
    }
    let canonical = path.canonicalize().map_err(|e| e.to_string())?;
    let directory = directory.canonicalize().map_err(|e| e.to_string())?;
    if canonical.parent() != Some(directory.as_path()) {
        return Err("Recording is outside SottoASR's audio directory".into());
    }
    Ok(canonical)
}

fn open_audio(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Recording must be a regular file".into());
    }
    Ok(file)
}

fn receipt_path(directory: &Path, id: &str) -> PathBuf {
    directory.join(format!("sotto_{id}.recovered"))
}

/// Import through an unpublished file so a crash cannot publish half a copy.
/// The legacy source stays untouched, including after successful recovery.
fn import_legacy(source: &Path, directory: &Path, id: &str) -> Result<(), String> {
    let destination = directory.join(format!("sotto_{id}.wav"));
    if destination.try_exists().map_err(|e| e.to_string())? {
        return Ok(());
    }
    let temporary = directory.join(format!(".import-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<(), String> {
        let mut source = open_audio(source)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut output = options.open(&temporary).map_err(|e| e.to_string())?;
        io::copy(&mut source, &mut output).map_err(|e| e.to_string())?;
        output.sync_all().map_err(|e| e.to_string())?;
        // Keep the source recording date rather than the import date.
        if let Ok(modified) = source.metadata().and_then(|metadata| metadata.modified()) {
            output
                .set_times(fs::FileTimes::new().set_modified(modified))
                .map_err(|e| e.to_string())?;
        }
        match fs::hard_link(&temporary, &destination) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error.to_string()),
        }
        if let Err(error) = File::open(directory).and_then(|file| file.sync_all()) {
            log::warn!("Recording import succeeded but directory sync failed: {error}");
        }
        Ok(())
    })();
    let _ = fs::remove_file(temporary);
    result
}

fn describe_recording(path: &Path) -> Result<RecoveryRecording, String> {
    let id = recording_id(path)?;
    let file = open_audio(path)?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    let created_at = metadata
        .modified()
        .or_else(|_| metadata.created())
        .map(DateTime::<Utc>::from)
        .map_err(|e| format!("Could not read recording date: {e}"))?;
    let (duration_ms, error) = match hound::WavReader::new(io::BufReader::new(file)) {
        Ok(reader) => {
            let spec = reader.spec();
            if spec.sample_rate == 0 || spec.channels == 0 || reader.duration() == 0 {
                (
                    None,
                    Some("The recording contains no readable audio samples.".into()),
                )
            } else {
                (
                    Some(reader.duration() as u64 * 1000 / spec.sample_rate as u64),
                    None,
                )
            }
        }
        Err(error) => (
            None,
            Some(format!(
                "The WAV file cannot be read: {error}. The original file is preserved."
            )),
        ),
    };
    Ok(RecoveryRecording {
        id,
        audio_path: path.to_string_lossy().into_owned(),
        created_at,
        duration_ms,
        size_bytes: metadata.len(),
        error,
    })
}

fn describe_or_error(path: &Path, id: String) -> RecoveryRecording {
    describe_recording(path).unwrap_or_else(|error| {
        let metadata = fs::symlink_metadata(path).ok();
        RecoveryRecording { id, audio_path: path.to_string_lossy().into_owned(),
            created_at: metadata.as_ref().and_then(|metadata| metadata.modified().ok()).map(DateTime::<Utc>::from).unwrap_or_default(),
            duration_ms: None, size_bytes: metadata.map_or(0, |metadata| metadata.len()),
            error: Some(format!("The recording could not be read: {error}. The file was not changed.")) }
    })
}

fn scan_recordings(
    directory: &Path,
    temporary: &Path,
    saved: &[Transcription],
    active: Option<&Path>,
) -> Result<Vec<RecoveryRecording>, String> {
    let completed: HashSet<_> = saved
        .iter()
        .filter(|item| item.capture_error.is_none())
        .map(|item| item.id.as_str())
        .collect();
    let mut recordings = Vec::new();
    for entry in fs::read_dir(temporary)
        .map_err(|e| format!("Could not inspect temporary recordings: {e}"))?
    {
        let path = entry.map_err(|e| e.to_string())?.path();
        let Ok(id) = recording_id(&path) else {
            continue;
        };
        if completed.contains(id.as_str())
            || receipt_path(directory, &id)
                .try_exists()
                .map_err(|e| e.to_string())?
        {
            continue;
        }
        if checked_audio_path(&path, temporary).is_err() {
            continue;
        }
        if let Err(error) = import_legacy(&path, directory, &id) {
            let mut recording = describe_or_error(&path, id);
            recording.error = Some(format!("Could not copy audio to permanent storage: {error}. The temporary recording is preserved."));
            recordings.push(recording);
        }
    }
    let active = active.and_then(|path| recording_id(path).ok());
    for entry in
        fs::read_dir(directory).map_err(|e| format!("Could not inspect saved recordings: {e}"))?
    {
        let path = entry.map_err(|e| e.to_string())?.path();
        let Ok(id) = recording_id(&path) else {
            continue;
        };
        if completed.contains(id.as_str())
            || receipt_path(directory, &id)
                .try_exists()
                .map_err(|e| e.to_string())?
        {
            continue;
        }
        let Ok(path) = checked_audio_path(&path, directory) else {
            continue;
        };
        if active.as_deref() == Some(id.as_str()) {
            continue;
        }
        recordings.push(describe_or_error(&path, id));
    }
    recordings.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(recordings)
}

pub(crate) async fn pending_recordings(state: &AppState) -> Result<Vec<RecoveryRecording>, String> {
    if let Some(error) = state.recording_storage_error.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return Err(error);
    }
    let active = if state.get_state() == AppStateEnum::Idle {
        None
    } else {
        crate::audio::capture::active_recording_path(state)
    };
    let mut recordings = pending_with_active(active).await?;
    // Capture can start while disk inspection runs. Never offer its live file.
    if state.get_state() != AppStateEnum::Idle {
        if let Some(active) = crate::audio::capture::active_recording_path(state) {
            if let Ok(id) = recording_id(&active) { recordings.retain(|recording| recording.id != id); }
        }
    }
    Ok(recordings)
}

async fn pending_with_active(active: Option<PathBuf>) -> Result<Vec<RecoveryRecording>, String> {
    let _io = RECOVERY_IO.lock().await;
    // In-memory history can contain unsaved results. Only disk is an acknowledgement.
    let saved = super::transcription::persisted_transcriptions().await;
    tokio::task::spawn_blocking(move || {
        let mut recordings = scan_recordings(
            &recordings_dir()?,
            &legacy_directory(),
            saved.as_deref().unwrap_or_default(),
            active.as_deref(),
        )?;
        if let Err(error) = saved {
            if recordings.is_empty() {
                return Err(error);
            }
            for recording in &mut recordings {
                recording.error = Some(format!(
                    "History could not be read: {error}. The audio is preserved."
                ));
            }
        }
        Ok(recordings)
    })
    .await
    .map_err(|e| format!("Recovery inspection failed: {e}"))?
}

#[tauri::command]
pub async fn get_recoverable_recordings(
    state: State<'_, AppState>,
) -> Result<Vec<RecoveryRecording>, String> {
    pending_recordings(&state).await
}

struct RecoveryClaim<'a>(&'a AppState);
impl<'a> RecoveryClaim<'a> {
    fn acquire(state: &'a AppState) -> Result<Self, String> {
        use std::sync::atomic::Ordering;
        let mut current = state
            .current_state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if *current != AppStateEnum::Idle
            || state.is_exiting.load(Ordering::SeqCst)
            || state.shortcuts_updating.load(Ordering::SeqCst)
        {
            return Err("Finish the current operation before reprocessing a recording.".into());
        }
        *current = AppStateEnum::Transcribing;
        Ok(Self(state))
    }
}
impl Drop for RecoveryClaim<'_> {
    fn drop(&mut self) {
        self.0.set_state(AppStateEnum::Idle);
    }
}

async fn reprocess(
    state: &AppState,
    recording: &RecoveryRecording,
) -> Result<Transcription, String> {
    if let Some(error) = &recording.error {
        return Err(error.clone());
    }
    let settings = state.settings.lock().await.clone();
    let terms = settings.vocabulary.clone();
    let path = recording.audio_path.clone();
    state.new_job();
    let result = crate::asr::engine::with_engine(&state.asr_engine, move |engine| {
        if !engine.is_ready() {
            return Err(
                "The speech model is not ready. Wait for model loading, then try again.".into(),
            );
        }
        engine.transcribe_file_with_vocabulary(&path, &terms)
    })
    .await?;
    let raw = result.unboosted_text.unwrap_or_else(|| result.text.clone());
    let mut text = crate::dictionary::apply(&result.text, &settings.dictionary);
    let status = if settings.llm_cleanup_enabled {
        state.set_state(AppStateEnum::CleaningUp);
        let protected = settings
            .vocabulary
            .into_iter()
            .chain(
                settings
                    .dictionary
                    .into_iter()
                    .map(|entry| entry.replacement),
            )
            .collect::<Vec<_>>();
        let (cleaned, status) = crate::llm::cleanup::run_cleanup(state, &text, &protected).await;
        if matches!(status, LlmCleanupStatus::Applied { .. }) {
            text = cleaned;
        }
        status
    } else {
        LlmCleanupStatus::Disabled
    };
    *state.llm_last_status.lock().await = status.clone();
    Ok(Transcription {
        id: recording.id.clone(),
        word_count: text.split_whitespace().count(),
        raw_text: (text != raw).then_some(raw),
        text,
        duration_ms: recording.duration_ms.unwrap_or(0),
        created_at: recording.created_at,
        cancelled: false,
        capture_error: None,
        cleanup_suggestion: None,
        llm_applied: matches!(status, LlmCleanupStatus::Applied { .. }),
        llm_cleanup_status: status,
    })
}

#[tauri::command]
pub async fn recover_recording(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<Transcription, String> {
    if let Some(error) = &app.state::<RecoverySession>().error { return Err(error.clone()); }
    let claim = RecoveryClaim::acquire(&state)?;
    // Claim before lookup: stale simultaneous retries cannot duplicate inference.
    let recording = pending_with_active(None)
        .await?
        .into_iter()
        .find(|item| item.id == id)
        .ok_or("This recording is no longer pending recovery. Refresh History.")?;
    super::overlay::publish_state(&app, AppStateEnum::Transcribing);
    let result = async {
        let transcription = reprocess(&state, &recording).await?;
        *state.last_transcription.lock().await = Some(transcription.clone());
        let saved = super::transcription::add_transcription(transcription.clone()).await;
        super::transcription::emit_transcription(&app, &transcription, &saved);
        saved?;
        let id = transcription.id.clone();
        let receipt = tokio::task::spawn_blocking(move || {
            let directory = recordings_dir()?;
            crate::persistence::write_atomic(
                &receipt_path(&directory, &id),
                b"Transcript saved to History.\n",
            )
            .map_err(|e| e.to_string())
        })
        .await;
        if !matches!(receipt, Ok(Ok(()))) {
            // Disk history already acknowledges this UUID. Never report lost
            // work after its durable save; the receipt only outlives retention.
            log::warn!("Transcript saved, but recovery receipt failed: {receipt:?}");
        }
        log::info!(
            "Recovered recording {} to History; audio retained at {}",
            transcription.id,
            recording.audio_path
        );
        Ok(transcription)
    }
    .await;
    drop(claim);
    super::overlay::publish_state(&app, AppStateEnum::Idle);
    let _ = app.emit("recovery-recordings-changed", ());
    result.map_err(|e: String| format!("{e} Audio retained at {}", recording.audio_path))
}

#[tauri::command]
pub async fn reveal_recovery_recording(
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let recording = pending_recordings(&state)
        .await?
        .into_iter()
        .find(|item| item.id == id)
        .ok_or("This recording is no longer pending recovery. Refresh History.")?;
    super::overlay::reveal_recording_audio(recording.audio_path).await
}

pub(crate) fn offer_on_startup(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        match pending_recordings(&state).await {
            Ok(items) if items.is_empty() && app.state::<RecoverySession>().notice.is_none() => (),
            result => {
                match result {
                    Ok(items) => log::warn!(
                        "Found {} interrupted recording(s); opening History recovery",
                        items.len()
                    ),
                    Err(error) => log::error!("Could not inspect interrupted recordings: {error}"),
                }
                // The window fetches a snapshot on mount, so no startup event can be lost.
                if let Err(error) = super::overlay::open_transcription_history(app.clone()).await {
                    log::error!("Could not open recording recovery: {error}");
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vanished_file_remains_an_error_item_without_failing_other_recovery() {
        let directory = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let missing = directory.path().join(format!("sotto_{id}.wav"));
        let item = describe_or_error(&missing, id.clone());
        assert_eq!(item.id, id);
        assert!(item.error.is_some());
        assert_eq!(item.duration_ms, None);
        let session = RecoverySession { marker: None, notice: Some("Storage unavailable".into()), error: Some("Disk unavailable".into()) };
        session.clean_exit();
        assert!(session.error.is_some());
    }

    #[test]
    fn session_restart_distinguishes_crash_from_clean_exit_and_live_instance() {
        let directory = tempfile::tempdir().unwrap();
        let first = RecoverySession::start_in(directory.path()).unwrap();
        assert!(first.notice.is_none());
        assert!(RecoverySession::start_in(directory.path()).is_err());
        drop(first); // Abrupt exit never acknowledges clean shutdown.
        let restarted = RecoverySession::start_in(directory.path()).unwrap();
        assert!(restarted.notice.is_some());
        restarted.clean_exit();
        drop(restarted);
        assert!(RecoverySession::start_in(directory.path())
            .unwrap()
            .notice
            .is_none());
    }

    use crate::models::Settings;
    use crate::test_support::{MockAsrEngine, MockAudioCapture, MockPasteBackend};

    fn recording(directory: &Path) -> PathBuf {
        let path = directory.join(format!("sotto_{}.wav", uuid::Uuid::new_v4()));
        crate::audio::wav::write_recording_wav(&path, &[0.25; 16000], 16000).unwrap();
        path
    }

    #[test]
    fn legacy_restart_import_preserves_source_and_does_not_duplicate() {
        let legacy = tempfile::tempdir().unwrap();
        let durable = tempfile::tempdir().unwrap();
        let source = recording(legacy.path());
        let bytes = fs::read(&source).unwrap();
        let first = scan_recordings(durable.path(), legacy.path(), &[], None).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].duration_ms, Some(1750));
        assert_eq!(fs::read(&first[0].audio_path).unwrap(), bytes);
        let second = scan_recordings(durable.path(), legacy.path(), &[], None).unwrap();
        assert_eq!(second[0].id, first[0].id);
        assert_eq!(second.len(), 1);
        crate::persistence::write_atomic(&receipt_path(durable.path(), &first[0].id), b"saved")
            .unwrap();
        assert!(scan_recordings(durable.path(), legacy.path(), &[], None)
            .unwrap()
            .is_empty());
        assert_eq!(fs::read(source).unwrap(), bytes);
    }

    #[test]
    fn restart_ignores_active_audio_but_reports_unreadable_recordings() {
        let legacy = tempfile::tempdir().unwrap();
        let durable = tempfile::tempdir().unwrap();
        let active = recording(durable.path());
        let damaged = durable
            .path()
            .join(format!("sotto_{}.wav", uuid::Uuid::new_v4()));
        fs::write(&damaged, b"partial header").unwrap();
        fs::write(
            durable.path().join("unrelated.wav"),
            b"private unrelated file",
        )
        .unwrap();
        let pending = scan_recordings(durable.path(), legacy.path(), &[], Some(&active)).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].audio_path,
            damaged.canonicalize().unwrap().to_string_lossy()
        );
        assert!(pending[0].error.is_some());
        assert_eq!(fs::read(damaged).unwrap(), b"partial header");
        #[cfg(unix)]
        {
            let link = legacy
                .path()
                .join(format!("sotto_{}.wav", uuid::Uuid::new_v4()));
            std::os::unix::fs::symlink(&active, &link).unwrap();
            assert!(checked_audio_path(&link, legacy.path()).is_err());
            assert!(checked_audio_path(&active, legacy.path()).is_err());
        }
    }

    #[tokio::test]
    async fn recovery_preserves_audio_on_failure_and_never_pastes_on_success() {
        let directory = tempfile::tempdir().unwrap();
        let path = recording(directory.path());
        let item = describe_recording(&path).unwrap();
        let bytes = fs::read(&path).unwrap();
        let paste = std::sync::Arc::new(MockPasteBackend::new());
        let state = AppState::new_with_backends(
            Box::new(MockAudioCapture::sine_wave()),
            Box::new(MockAsrEngine::with_text("Recovered speech.")),
            None,
            Box::new(crate::test_support::SharedMockPaste(paste.clone())),
            Settings {
                llm_cleanup_enabled: false,
                ..Settings::default()
            },
        );
        let claim = RecoveryClaim::acquire(&state).unwrap();
        assert!(RecoveryClaim::acquire(&state).is_err());
        assert!(state.begin_exit().is_err());
        let result = reprocess(&state, &item).await.unwrap();
        assert_eq!(result.text, "Recovered speech.");
        assert_eq!(result.id, item.id);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(paste.pasted_texts.lock().unwrap().is_empty());
        assert!(paste.copied_texts.lock().unwrap().is_empty());
        // A failed save leaves the file pending on restart. A durable save
        // closes the crash window before any WAV deletion or receipt write.
        let history = tempfile::tempdir().unwrap();
        let blocked = history.path().join("blocked.json");
        fs::create_dir(&blocked).unwrap();
        let mut failed_store = super::super::transcription::TranscriptionStore::new(blocked);
        assert!(failed_store.add(result.clone()).is_err());
        let legacy = tempfile::tempdir().unwrap();
        assert_eq!(
            scan_recordings(directory.path(), legacy.path(), &[], None)
                .unwrap()
                .len(),
            1
        );
        let history_path = history.path().join("history.json");
        let mut store = super::super::transcription::TranscriptionStore::new(history_path.clone());
        store.add(result).unwrap();
        let saved = super::super::transcription::TranscriptionStore::load(&history_path).unwrap();
        assert!(
            scan_recordings(directory.path(), legacy.path(), &saved, None)
                .unwrap()
                .is_empty()
        );
        drop(claim);
        assert_eq!(state.get_state(), AppStateEnum::Idle);
        *state.asr_engine.lock().await = Box::new(MockAsrEngine::with_error("Model failed"));
        assert!(reprocess(&state, &item)
            .await
            .unwrap_err()
            .contains("Model failed"));
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}
