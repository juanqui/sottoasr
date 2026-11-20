//! Restart recovery uses recording IDs, never webview-supplied filesystem paths.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};
use tauri::{Emitter, Manager, State};

use crate::audio::recovery::{recording_id, recordings_dir};
use crate::models::{AppStateEnum, LlmCleanupStatus, Transcription};
use crate::state::AppState;
use super::transcription::TranscriptionStore;

/// Serializes the restart scan with every path that deletes recordings, so a
/// scan never imports a legacy copy between a deletion and its tombstone.
/// Lock it before the history store.
pub(super) static RECOVERY_IO: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
        let notice = (!previous.is_empty() && previous != b"clean\n").then(|| format!(
            "SottoASR did not shut down cleanly. Review the saved recordings below. If no recording is listed, no recording of {} seconds or more needs recovery.",
            MIN_RECOVERY_DURATION_MS / 1000
        ));
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

/// A shorter recording is not offered for recovery: to say the words again
/// is faster than to recover them. Its audio stays on disk and follows the
/// audio retention limit, like every other recording.
const MIN_RECOVERY_DURATION_MS: u64 = 15_000;

impl RecoveryRecording {
    /// A recording with no readable length stays in the list, so that the
    /// user sees its error and can open or delete the file.
    fn is_offered(&self) -> bool {
        self.duration_ms.is_none_or(|duration| duration >= MIN_RECOVERY_DURATION_MS)
    }
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

/// Recording IDs name files, so only the canonical UUID form is accepted.
fn checked_recording_id(id: &str) -> Result<(), String> {
    match uuid::Uuid::parse_str(id) {
        Ok(parsed) if parsed.to_string() == id => Ok(()),
        _ => Err("Invalid recording ID".into()),
    }
}

/// Delete a recording and its receipt. While a legacy copy remains in the
/// temporary directory, a tombstone receipt stays instead, so the restart scan
/// never imports that copy again. The legacy copy itself is never deleted.
/// Returns whether a WAV was removed.
fn remove_recording(directory: &Path, legacy: &Path, id: &str) -> Result<bool, String> {
    checked_recording_id(id)?;
    let name = format!("sotto_{id}.wav");
    let receipt = receipt_path(directory, id);
    let legacy_copy = fs::symlink_metadata(legacy.join(&name)).is_ok();
    // The tombstone must be durable before the WAV goes. Otherwise a failed
    // write, or a crash in between, lets the next scan import the copy again.
    let wrote_tombstone = legacy_copy && !receipt.try_exists().map_err(|e| e.to_string())?;
    if wrote_tombstone {
        #[cfg(test)]
        take_tombstone_failure(id)?;
        crate::persistence::write_atomic(&receipt, b"Recording deleted.\n").map_err(|e| {
            format!("Could not record the deletion, so the recording was kept: {e}")
        })?;
    }
    let removed = match fs::remove_file(directory.join(&name)) {
        Ok(()) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => {
            // A receipt hides the kept recording from recovery. Drop only the
            // one this call wrote, so the user still sees it and can retry.
            if wrote_tombstone {
                let _ = fs::remove_file(&receipt);
            }
            return Err(format!("Could not delete the recording: {error}"));
        }
    };
    if !legacy_copy {
        match fs::remove_file(&receipt) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(format!("Could not delete the recovery receipt: {error}")),
        }
    }
    Ok(removed)
}

/// Tombstone writes that fail once, as on a full disk, keyed by recording ID.
#[cfg(test)]
static FAILING_TOMBSTONES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

#[cfg(test)]
fn take_tombstone_failure(id: &str) -> Result<(), String> {
    let mut failing = FAILING_TOMBSTONES.lock().unwrap_or_else(|e| e.into_inner());
    match failing.iter().position(|item| item == id) {
        Some(index) => {
            failing.swap_remove(index);
            Err("Could not record the deletion, so the recording was kept: injected test failure".into())
        }
        None => Ok(()),
    }
}

/// Entries without a recording ID (older history) have no files to remove.
/// Failures are logged and keep that recording.
fn remove_recordings<'a>(directory: &Path, legacy: &Path, ids: impl IntoIterator<Item = &'a str>) -> bool {
    let mut removed = false;
    for id in ids {
        if checked_recording_id(id).is_err() {
            continue;
        }
        match remove_recording(directory, legacy, id) {
            Ok(wav) => removed |= wav,
            Err(error) => log::warn!("Could not remove retained audio of recording {id}: {error}"),
        }
    }
    removed
}

/// The recordings a history change may remove. Deletes and retention hold the
/// recovery claim, and the pipeline saves (and trims) only outside Idle; both
/// keep a capture from starting, so none of these is a live recording.
pub(super) struct RecordingFiles {
    directory: PathBuf,
    legacy: PathBuf,
}

impl RecordingFiles {
    /// Blocking. `None`, logged, while another instance owns the recordings
    /// (`session_error`) or their directory is unavailable. The history
    /// change still applies; only the audio stays.
    pub(super) fn resolve(session_error: Option<String>) -> Option<Self> {
        let directory = match session_error {
            Some(error) => Err(error),
            None => recordings_dir(),
        };
        match directory {
            Ok(directory) => Some(Self { directory, legacy: legacy_directory() }),
            Err(error) => {
                log::warn!("Retained audio was kept: {error}");
                None
            }
        }
    }

    /// Returns whether a WAV was removed.
    fn remove<'a>(&self, ids: impl IntoIterator<Item = &'a str>) -> bool {
        remove_recordings(&self.directory, &self.legacy, ids)
    }
}

/// Captures the second-instance guard now; resolves the directory later, on
/// the blocking worker, and only if a history change needs it.
pub(super) fn history_files(
    app: &tauri::AppHandle,
) -> impl FnOnce() -> Option<RecordingFiles> + Send + 'static {
    let session_error = app.state::<RecoverySession>().error.clone();
    move || RecordingFiles::resolve(session_error)
}

/// Delete one history entry, then its audio and receipt. The save comes
/// first, so a failed save keeps both; an ID history did not list keeps its
/// audio. Returns whether a WAV was removed.
pub(super) fn delete_history_entry(
    store: &mut TranscriptionStore,
    id: &str,
    files: impl FnOnce() -> Option<RecordingFiles>,
) -> Result<bool, String> {
    if !store.delete(id)? {
        return Ok(false);
    }
    Ok(files().is_some_and(|files| files.remove([id])))
}

/// Clear history, then the audio and receipts of every removed entry.
/// Returns the removed IDs and whether a WAV was removed.
pub(super) fn clear_history(
    store: &mut TranscriptionStore,
    files: impl FnOnce() -> Option<RecordingFiles>,
) -> Result<(Vec<String>, bool), String> {
    let removed = store.clear()?;
    let audio = !removed.is_empty()
        && files().is_some_and(|files| files.remove(removed.iter().map(String::as_str)));
    Ok((removed, audio))
}

/// Save an entry. Entries the 5,000-entry cap trims lose their audio and
/// receipt once the trimmed list is durable, except interrupted ones: that
/// audio is still the user's recovery copy. Returns the trimmed IDs.
pub(super) fn add_history_entry(
    store: &mut TranscriptionStore,
    transcription: Transcription,
    files: impl FnOnce() -> Option<RecordingFiles>,
) -> Result<Vec<String>, String> {
    let trimmed = store.add(transcription)?;
    let completed: Vec<&str> = trimmed
        .iter()
        .filter(|item| item.capture_error.is_none())
        .map(|item| item.id.as_str())
        .collect();
    let files = if completed.is_empty() { None } else { files() };
    if let Some(files) = files {
        files.remove(completed);
    }
    Ok(trimmed.into_iter().map(|item| item.id).collect())
}

/// Remove recordings last modified before `cutoff`, listed in History or not.
fn expire_recordings(directory: &Path, legacy: &Path, cutoff: SystemTime) -> Result<bool, String> {
    let mut expired = Vec::new();
    for entry in
        fs::read_dir(directory).map_err(|e| format!("Could not inspect saved recordings: {e}"))?
    {
        let path = entry.map_err(|e| e.to_string())?.path();
        let Ok(id) = recording_id(&path) else {
            continue;
        };
        let old = fs::symlink_metadata(&path).is_ok_and(|metadata| {
            metadata.file_type().is_file()
                && metadata.modified().is_ok_and(|modified| modified < cutoff)
        });
        if old {
            expired.push(id);
        }
    }
    Ok(remove_recordings(directory, legacy, expired.iter().map(String::as_str)))
}

/// The blocking half of a retention pass, after History durably dropped
/// `expired`. An interrupted entry's audio is still the user's recovery copy,
/// so it follows the recording limit, not the history limit.
fn expire_audio(
    directory: &Path,
    legacy: &Path,
    expired: &[Transcription],
    cutoff: Option<SystemTime>,
) -> Result<bool, String> {
    let completed = expired
        .iter()
        .filter(|item| item.capture_error.is_none())
        .map(|item| item.id.as_str());
    let mut removed = remove_recordings(directory, legacy, completed);
    if let Some(cutoff) = cutoff {
        removed |= expire_recordings(directory, legacy, cutoff)?;
    }
    Ok(removed)
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
    recordings.retain(RecoveryRecording::is_offered);
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

/// The capture path outlives its capture, so it names the live recording
/// only while SottoASR is not Idle, as in `pending_recordings`.
fn active_recording_id(state: &AppState) -> Option<String> {
    if state.get_state() == AppStateEnum::Idle {
        return None;
    }
    crate::audio::capture::active_recording_path(state).and_then(|path| recording_id(&path).ok())
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

/// Holds SottoASR out of Idle, so no capture, dictation or second reprocess
/// starts, and no delete or retention pass removes a file being reprocessed.
pub(super) struct RecoveryClaim<'a>(&'a AppState);
impl<'a> RecoveryClaim<'a> {
    pub(super) fn acquire(state: &'a AppState) -> Result<Self, String> {
        use std::sync::atomic::Ordering;
        let mut current = state
            .current_state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if *current != AppStateEnum::Idle
            || state.is_exiting.load(Ordering::SeqCst)
            || state.shortcuts_updating.load(Ordering::SeqCst)
        {
            return Err("Finish the current recording or transcription, then try again.".into());
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
    state.new_job();
    let result = crate::pipeline::transcribe_with_deadline(
        state,
        recording.audio_path.clone(),
        terms,
        recording.duration_ms.unwrap_or(0),
    )
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
        saved_at: None,
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

/// Delete a pending recording's audio after the user confirmed it.
#[tauri::command]
pub async fn delete_recovery_recording(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    if let Some(error) = &app.state::<RecoverySession>().error { return Err(error.clone()); }
    checked_recording_id(&id)?;
    if active_recording_id(&state).as_deref() == Some(id.as_str()) {
        return Err("This recording is still in progress. Stop it, then try again.".into());
    }
    let result = {
        let _claim = RecoveryClaim::acquire(&state)?;
        let _io = RECOVERY_IO.lock().await;
        tokio::task::spawn_blocking(move || {
            remove_recording(&recordings_dir()?, &legacy_directory(), &id)
        })
        .await
        .map_err(|e| format!("Could not delete the recording: {e}"))?
    };
    let _ = app.emit("recovery-recordings-changed", ());
    result.map(|_| ())
}

/// Apply the user's retention limits; `0` keeps data until the user deletes
/// it. A pass that finds SottoASR busy is skipped until the next one, so it
/// never removes a file that a capture or reprocess holds. Problems are
/// logged, never returned.
pub async fn enforce_retention(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    // Settings loaded at startup are not validated, and a hand-edited value
    // must not overflow the date arithmetic and end the retention timer.
    let (history_days, audio_days) = {
        let settings = state.settings.lock().await;
        let limit = crate::models::Settings::MAX_RETENTION_DAYS;
        (settings.history_retention_days.min(limit), settings.audio_retention_days.min(limit))
    };
    if history_days == 0 && audio_days == 0 {
        return;
    }
    if let Some(error) = &app.state::<RecoverySession>().error {
        log::warn!("Retention skipped: {error}");
        return;
    }
    let (removed_ids, recoveries_changed) = retention_pass(&state, history_days, audio_days).await;
    if !removed_ids.is_empty() {
        log::info!("Retention removed {} history entries", removed_ids.len());
        let _ = app.emit("history-changed", serde_json::json!({ "removed_ids": removed_ids }));
    }
    if recoveries_changed {
        let _ = app.emit("recovery-recordings-changed", ());
    }
}

/// Returns the removed history IDs and whether the recovery list changed: a
/// WAV was removed, or an expired interrupted entry left its audio pending.
/// Skipped while the recovery claim cannot be taken: a capture, dictation or
/// reprocess is running.
async fn retention_pass(state: &AppState, history_days: u32, audio_days: u32) -> (Vec<String>, bool) {
    let Ok(_claim) = RecoveryClaim::acquire(state) else {
        return (Vec::new(), false);
    };
    let _io = RECOVERY_IO.lock().await;
    let now = Utc::now();
    let history_cutoff = match history_days {
        0 => None,
        days => chrono::Duration::try_days(i64::from(days)).and_then(|age| now.checked_sub_signed(age)),
    };
    let expired = match history_cutoff {
        None => Vec::new(),
        Some(cutoff) => super::transcription::expire_history(cutoff).await.unwrap_or_else(|error| {
            log::warn!("History retention failed: {error}");
            Vec::new()
        }),
    };
    let removed_ids: Vec<String> = expired.iter().map(|item| item.id.clone()).collect();
    let pending_again = expired.iter().any(|item| item.capture_error.is_some());
    let audio_cutoff = match audio_days {
        0 => None,
        days => SystemTime::from(now).checked_sub(Duration::from_secs(u64::from(days) * 86_400)),
    };
    let removed_audio = match tokio::task::spawn_blocking(move || {
        expire_audio(&recordings_dir()?, &legacy_directory(), &expired, audio_cutoff)
    })
    .await
    {
        Ok(Ok(removed)) => removed,
        Ok(Err(error)) => {
            log::warn!("Recording retention failed: {error}");
            false
        }
        Err(error) => {
            log::warn!("Recording retention failed: {error}");
            false
        }
    };
    (removed_ids, removed_audio || pending_again)
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

    /// A recording of 16.75 s, long enough for recovery: 16,000 samples at
    /// 1 kHz and the trailing silence.
    fn recording(directory: &Path) -> PathBuf {
        let path = directory.join(format!("sotto_{}.wav", uuid::Uuid::new_v4()));
        crate::audio::wav::write_recording_wav(&path, &[0.25; 16000], 1000).unwrap();
        path
    }

    #[test]
    fn a_recording_below_the_minimum_length_is_not_offered_and_stays_on_disk() {
        let durable = tempfile::tempdir().unwrap();
        let legacy = tempfile::tempdir().unwrap();
        // The length in milliseconds includes 750 ms of trailing silence.
        let with_length = |milliseconds: usize| {
            let path = durable.path().join(format!("sotto_{}.wav", uuid::Uuid::new_v4()));
            crate::audio::wav::write_recording_wav(&path, &vec![0.25; milliseconds - 750], 1000).unwrap();
            path
        };
        let short = with_length(2_800);
        let almost = with_length(14_999);
        let exact = with_length(15_000);
        // A file with no readable length: the user must still see it.
        let unreadable = durable.path().join(format!("sotto_{}.wav", uuid::Uuid::new_v4()));
        fs::write(&unreadable, b"not a WAV file").unwrap();

        let offered = scan_recordings(durable.path(), legacy.path(), &[], None).unwrap();

        let mut ids: Vec<&str> = offered.iter().map(|item| item.id.as_str()).collect();
        ids.sort_unstable();
        let mut expected = [id_of(&exact), id_of(&unreadable)];
        expected.sort_unstable();
        assert_eq!(ids, expected);
        assert!(offered.iter().find(|item| item.id == id_of(&unreadable)).unwrap().error.is_some());
        for kept in [&short, &almost] {
            assert!(kept.is_file(), "a short recording is not deleted");
        }
        // A short recording in the older temporary folder is still copied to
        // the durable folder, and is not offered there either.
        let temporary = legacy.path().join(format!("sotto_{}.wav", uuid::Uuid::new_v4()));
        crate::audio::wav::write_recording_wav(&temporary, &[0.25; 2_000], 1000).unwrap();
        let offered = scan_recordings(durable.path(), legacy.path(), &[], None).unwrap();
        assert_eq!(offered.len(), 2);
        assert!(durable.path().join(temporary.file_name().unwrap()).is_file());
    }

    #[test]
    fn legacy_restart_import_preserves_source_and_does_not_duplicate() {
        let legacy = tempfile::tempdir().unwrap();
        let durable = tempfile::tempdir().unwrap();
        let source = recording(legacy.path());
        let bytes = fs::read(&source).unwrap();
        let first = scan_recordings(durable.path(), legacy.path(), &[], None).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].duration_ms, Some(16750));
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

    fn id_of(path: &Path) -> String {
        recording_id(path).unwrap()
    }

    fn age(path: &Path, days: u64) {
        let modified = SystemTime::now() - Duration::from_secs(days * 86_400);
        OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
    }

    fn entry(id: &str, created_days_ago: i64, saved_days_ago: Option<i64>, capture_error: Option<&str>) -> Transcription {
        let now = Utc::now();
        Transcription {
            id: id.into(),
            text: "Saved words.".into(),
            duration_ms: 1000,
            created_at: now - chrono::Duration::days(created_days_ago),
            word_count: 2,
            cancelled: false,
            capture_error: capture_error.map(Into::into),
            raw_text: None,
            cleanup_suggestion: None,
            llm_applied: false,
            llm_cleanup_status: LlmCleanupStatus::Disabled,
            saved_at: saved_days_ago.map(|days| now - chrono::Duration::days(days)),
        }
    }

    fn write_receipt(directory: &Path, id: &str) {
        crate::persistence::write_atomic(&receipt_path(directory, id), b"Transcript saved to History.\n").unwrap();
    }

    #[test]
    fn history_delete_and_clear_remove_audio_only_after_a_durable_save() {
        let durable = tempfile::tempdir().unwrap();
        let legacy = tempfile::tempdir().unwrap();
        let history = tempfile::tempdir().unwrap();
        let files = || {
            Some(RecordingFiles { directory: durable.path().to_path_buf(), legacy: legacy.path().to_path_buf() })
        };
        let [recovered, interrupted, completed, unlisted]: [PathBuf; 4] =
            std::array::from_fn(|_| recording(durable.path()));
        let (recovered_id, completed_id) = (id_of(&recovered), id_of(&completed));
        write_receipt(durable.path(), &recovered_id);
        write_receipt(durable.path(), &completed_id);
        let history_path = history.path().join("history.json");
        let mut store = TranscriptionStore::new(history_path.clone());
        store.add(entry(&recovered_id, 400, None, None)).unwrap();
        store.add(entry(&id_of(&interrupted), 1, None, Some("Microphone disconnected"))).unwrap();
        store.add(entry(&completed_id, 1, None, None)).unwrap();

        // Only an entry that history listed takes its audio with it.
        assert!(!delete_history_entry(&mut store, &id_of(&unlisted), files).unwrap());
        assert!(unlisted.exists());
        // A failed save keeps every entry, recording and receipt.
        fs::remove_file(&history_path).unwrap();
        fs::create_dir(&history_path).unwrap();
        assert!(delete_history_entry(&mut store, &recovered_id, files).is_err());
        assert!(clear_history(&mut store, files).is_err());
        for kept in [&recovered, &interrupted, &completed] {
            assert!(kept.exists(), "{}", kept.display());
        }
        assert!(receipt_path(durable.path(), &recovered_id).exists());
        fs::remove_dir(&history_path).unwrap();

        assert!(delete_history_entry(&mut store, &recovered_id, files).unwrap());
        assert!(!recovered.exists());
        assert!(!receipt_path(durable.path(), &recovered_id).exists());
        let (cleared, removed_audio) = clear_history(&mut store, files).unwrap();
        assert_eq!(cleared.len(), 2);
        assert!(removed_audio);
        // The user confirmed a dialog that names audio, so the interrupted
        // entry's recovery copy goes too.
        assert!(!interrupted.exists());
        assert!(!completed.exists());
        assert!(!receipt_path(durable.path(), &completed_id).exists());
        assert!(unlisted.exists());
        assert!(TranscriptionStore::load(&history_path).unwrap().is_empty());

        // While another instance owns the recordings, the entry goes and its
        // audio stays.
        let owned_elsewhere = recording(durable.path());
        store.add(entry(&id_of(&owned_elsewhere), 1, None, None)).unwrap();
        let other_instance = || {
            RecordingFiles::resolve(Some("Another SottoASR instance already owns the recording directory.".into()))
        };
        assert!(!delete_history_entry(&mut store, &id_of(&owned_elsewhere), other_instance).unwrap());
        assert!(owned_elsewhere.exists());
        assert!(store.get_all().is_empty());
    }

    #[test]
    fn deleted_legacy_recording_is_not_imported_again() {
        let legacy = tempfile::tempdir().unwrap();
        let durable = tempfile::tempdir().unwrap();
        let source = recording(legacy.path());
        let bytes = fs::read(&source).unwrap();
        let pending = scan_recordings(durable.path(), legacy.path(), &[], None).unwrap();
        assert_eq!(pending.len(), 1);
        let id = pending[0].id.clone();
        let imported = durable.path().join(format!("sotto_{id}.wav"));
        assert!(imported.exists());
        assert!(remove_recording(durable.path(), legacy.path(), &id).unwrap());
        assert!(!imported.exists());
        assert!(receipt_path(durable.path(), &id).exists());
        assert!(scan_recordings(durable.path(), legacy.path(), &[], None).unwrap().is_empty());
        assert!(!imported.exists());
        assert_eq!(fs::read(&source).unwrap(), bytes);
        // Deleting again, as a later history delete would, keeps the tombstone.
        assert!(!remove_recording(durable.path(), legacy.path(), &id).unwrap());
        assert!(receipt_path(durable.path(), &id).exists());
        let upper = id.to_uppercase();
        for invalid in ["../session", "session.state", upper.as_str()] {
            assert!(remove_recording(durable.path(), legacy.path(), invalid).is_err());
        }
    }

    #[test]
    fn failed_tombstone_keeps_the_recording_and_the_scan_does_not_import_it_again() {
        let legacy = tempfile::tempdir().unwrap();
        let durable = tempfile::tempdir().unwrap();
        recording(legacy.path());
        let id = scan_recordings(durable.path(), legacy.path(), &[], None).unwrap()[0].id.clone();
        let imported = durable.path().join(format!("sotto_{id}.wav"));
        let bytes = fs::read(&imported).unwrap();
        // The receipt cannot be written, as on a full disk.
        FAILING_TOMBSTONES.lock().unwrap().push(id.clone());
        assert!(remove_recording(durable.path(), legacy.path(), &id).is_err());
        assert_eq!(fs::read(&imported).unwrap(), bytes);
        assert!(!receipt_path(durable.path(), &id).exists());
        // A history delete logs the same failure and keeps the audio too.
        FAILING_TOMBSTONES.lock().unwrap().push(id.clone());
        assert!(!remove_recordings(durable.path(), legacy.path(), [id.as_str()]));
        assert_eq!(fs::read(&imported).unwrap(), bytes);
        let pending = scan_recordings(durable.path(), legacy.path(), &[], None).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, id);
        let recordings = fs::read_dir(durable.path())
            .unwrap()
            .filter(|entry| recording_id(&entry.as_ref().unwrap().path()).is_ok())
            .count();
        assert_eq!(recordings, 1);
    }

    /// macOS's user-immutable flag makes the WAV undeletable for its owner.
    #[cfg(target_os = "macos")]
    #[test]
    fn undeletable_recording_drops_the_new_tombstone_and_stays_pending() {
        use std::os::unix::ffi::OsStrExt;
        let legacy = tempfile::tempdir().unwrap();
        let durable = tempfile::tempdir().unwrap();
        recording(legacy.path());
        let id = scan_recordings(durable.path(), legacy.path(), &[], None).unwrap()[0].id.clone();
        let imported = durable.path().join(format!("sotto_{id}.wav"));
        let path = std::ffi::CString::new(imported.as_os_str().as_bytes()).unwrap();
        let set_flags = |flags| {
            // SAFETY: `path` is a valid NUL-terminated path that outlives the call.
            assert_eq!(unsafe { libc::chflags(path.as_ptr(), flags) }, 0);
        };
        set_flags(libc::UF_IMMUTABLE);
        let result = remove_recording(durable.path(), legacy.path(), &id);
        set_flags(0);
        assert!(result.unwrap_err().contains("Could not delete the recording"));
        assert!(imported.exists());
        assert!(!receipt_path(durable.path(), &id).exists());
        let pending = scan_recordings(durable.path(), legacy.path(), &[], None).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, id);

        // A receipt that existed before the attempt is never removed.
        write_receipt(durable.path(), &id);
        set_flags(libc::UF_IMMUTABLE);
        let result = remove_recording(durable.path(), legacy.path(), &id);
        set_flags(0);
        assert!(result.is_err());
        assert!(imported.exists());
        assert!(receipt_path(durable.path(), &id).exists());
    }

    #[test]
    fn retention_removes_only_expired_data_and_spares_recovery_copies() {
        let durable = tempfile::tempdir().unwrap();
        let legacy = tempfile::tempdir().unwrap();
        let history = tempfile::tempdir().unwrap();
        let [expired, recovered, interrupted, orphan, fresh]: [PathBuf; 5] =
            std::array::from_fn(|_| recording(durable.path()));
        let expired_id = id_of(&expired);
        let recovered_id = id_of(&recovered);
        let interrupted_id = id_of(&interrupted);
        write_receipt(durable.path(), &expired_id);
        write_receipt(durable.path(), &recovered_id);
        for path in [&recovered, &interrupted, &orphan] {
            age(path, 400);
        }
        // Older builds saved no `saved_at`, so write that history file directly.
        let older_build = entry(&uuid::Uuid::new_v4().to_string(), 40, None, None);
        let history_path = history.path().join("history.json");
        let saved = vec![
            entry(&expired_id, 40, Some(40), None),
            entry(&interrupted_id, 40, Some(40), Some("Microphone disconnected")),
            older_build.clone(),
        ];
        crate::persistence::write_atomic(&history_path, &serde_json::to_vec(&saved).unwrap()).unwrap();
        let mut store = TranscriptionStore::new(history_path);
        // Reprocessing keeps the recording's date; the store dates the save.
        store.add(entry(&recovered_id, 400, None, None)).unwrap();

        let removed = store.expire(Utc::now() - chrono::Duration::days(30)).unwrap();
        let removed_ids: Vec<&str> = removed.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(removed_ids, [expired_id.as_str(), interrupted_id.as_str(), older_build.id.as_str()]);
        assert_eq!(store.get_all().len(), 1);
        assert_eq!(store.get_all()[0].id, recovered_id);
        assert!(expire_audio(durable.path(), legacy.path(), &removed, None).unwrap());
        assert!(!expired.exists());
        assert!(!receipt_path(durable.path(), &expired_id).exists());
        assert!(recovered.exists() && receipt_path(durable.path(), &recovered_id).exists());
        // The interrupted entry's audio stays and is offered for recovery.
        assert!(interrupted.exists());
        let pending = scan_recordings(durable.path(), legacy.path(), &store.get_all(), None).unwrap();
        assert!(pending.iter().any(|item| item.id == interrupted_id));

        let cutoff = SystemTime::now() - Duration::from_secs(30 * 86_400);
        assert!(expire_audio(durable.path(), legacy.path(), &[], Some(cutoff)).unwrap());
        assert!(fresh.exists());
        for gone in [&interrupted, &orphan, &recovered] {
            assert!(!gone.exists(), "{}", gone.display());
        }
        assert!(!receipt_path(durable.path(), &recovered_id).exists());
        assert_eq!(store.get_all()[0].id, recovered_id);
    }

    #[tokio::test]
    async fn retention_waits_while_recording_and_reports_audio_pending_again() {
        use crate::commands::transcription::{add_transcription, persisted_transcriptions};
        let state = AppState::new_with_backends(
            Box::new(MockAudioCapture::sine_wave()),
            Box::new(MockAsrEngine::with_text("Unused.")),
            None,
            Box::new(MockPasteBackend::new()),
            Settings::default(),
        );
        let completed = uuid::Uuid::new_v4().to_string();
        let interrupted = uuid::Uuid::new_v4().to_string();
        add_transcription(entry(&completed, 400, Some(400), None)).await.unwrap();
        add_transcription(entry(&interrupted, 400, Some(400), Some("Microphone disconnected"))).await.unwrap();
        // A live capture keeps SottoASR out of Idle, so the pass does nothing.
        state.set_state(AppStateEnum::Recording);
        assert_eq!(retention_pass(&state, 30, 0).await, (Vec::new(), false));
        assert_eq!(state.get_state(), AppStateEnum::Recording);
        assert!(persisted_transcriptions().await.unwrap().iter().any(|item| item.id == completed));

        state.set_state(AppStateEnum::Idle);
        let (removed, recoveries_changed) = retention_pass(&state, 30, 0).await;
        assert!(removed.contains(&completed));
        assert!(removed.contains(&interrupted));
        // No WAV was removed, but the interrupted entry's audio is pending again.
        assert!(recoveries_changed);
        assert_eq!(state.get_state(), AppStateEnum::Idle);
        let saved = persisted_transcriptions().await.unwrap();
        assert!(!saved.iter().any(|item| item.id == completed || item.id == interrupted));
    }

    #[test]
    fn live_capture_is_named_only_outside_idle_and_blocks_deletion() {
        let state = AppState::new_with_backends(
            Box::new(MockAudioCapture::sine_wave()),
            Box::new(MockAsrEngine::with_text("Unused.")),
            None,
            Box::new(MockPasteBackend::new()),
            Settings::default(),
        );
        let id = uuid::Uuid::new_v4().to_string();
        *state.recording_audio_path.lock().unwrap() =
            Some(std::env::temp_dir().join(format!("sotto_{id}.wav")));
        assert_eq!(active_recording_id(&state), None);
        state.set_state(AppStateEnum::Recording);
        assert_eq!(active_recording_id(&state).as_deref(), Some(id.as_str()));
        assert!(RecoveryClaim::acquire(&state).is_err());
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
