use tauri::{Emitter, State};
use crate::state::AppState;
use crate::models::Transcription;
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};

/// Quote text cells and mark formula-looking content as text for spreadsheet
/// import. Only the export gains a prefix; stored dictation stays byte-identical.
/// CSV has no universal spreadsheet type metadata; resaving it in a spreadsheet
/// can remove this protection (OWASP CSV Injection guidance).
fn csv_cell(value: &str) -> String {
    let formula = value.trim_start().starts_with(['=', '+', '-', '@', '＝', '＋', '－', '＠'])
        || value.starts_with(['\t', '\r', '\n']);
    let escaped = value.replace('"', "\"\"");
    format!("\"{}{}\"", if formula { "'" } else { "" }, escaped)
}

/// A testable store for transcriptions, backed by a JSON file on disk.
pub struct TranscriptionStore {
    items: Vec<Transcription>,
    path: PathBuf,
    load_error: Option<String>,
}

impl TranscriptionStore {
    /// Create a new store that persists to the given path.
    /// Loads existing transcriptions from disk if the file exists.
    pub fn new(path: PathBuf) -> Self {
        let (items, load_error) = match Self::load_from(&path) {
            Ok(items) => (items, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        Self { items, path, load_error }
    }

    /// Add a transcription (newest first), capping at 5000 entries, and persist.
    /// Returns the entries the cap removed, only once the trimmed list is durable.
    pub fn add(&mut self, mut transcription: Transcription) -> Result<Vec<Transcription>, String> {
        // Retention measures age from the first save: a recovered recording
        // keeps the date of its audio in `created_at`.
        if transcription.saved_at.is_none() {
            transcription.saved_at = Some(Utc::now());
        }
        // Retrying a recording replaces its unsaved/partial result, not its ID.
        self.items.retain(|item| item.id != transcription.id);
        self.items.insert(0, transcription);
        #[cfg(test)]
        take_injected_failure(&self.items[0].id)?;
        // Keep the newly captured text in memory if storage fails. Never destroy
        // an unreadable existing file by replacing it with an empty fallback.
        let retained = self.items.len().min(5000);
        self.save_items(&self.items[..retained])?;
        Ok(self.items.split_off(retained))
    }

    /// Delete a transcription by ID and persist. Returns whether it existed.
    pub fn delete(&mut self, id: &str) -> Result<bool, String> {
        let remaining: Vec<_> = self.items.iter().filter(|t| t.id != id).cloned().collect();
        self.save_items(&remaining)?;
        let removed = remaining.len() != self.items.len();
        self.items = remaining;
        Ok(removed)
    }

    /// Clear all transcriptions and persist. Returns the removed IDs.
    pub fn clear(&mut self) -> Result<Vec<String>, String> {
        self.save_items(&[])?;
        Ok(self.items.drain(..).map(|item| item.id).collect())
    }

    /// Remove entries first saved before `cutoff` and persist. Entries saved by
    /// older builds have no `saved_at` and are measured from `created_at`.
    pub fn expire(&mut self, cutoff: DateTime<Utc>) -> Result<Vec<Transcription>, String> {
        let expired = |item: &Transcription| item.saved_at.unwrap_or(item.created_at) < cutoff;
        if !self.items.iter().any(expired) {
            return Ok(Vec::new());
        }
        let (removed, remaining): (Vec<_>, Vec<_>) = self.items.iter().cloned().partition(expired);
        self.save_items(&remaining)?;
        self.items = remaining;
        Ok(removed)
    }

    /// Export all transcriptions as CSV.
    pub fn export_csv(&self) -> String {
        let mut csv = String::from("id,created_at,duration_ms,word_count,llm_applied,text,raw_text,capture_error,cleanup_suggestion\n");
        for t in &self.items {
            csv.push_str(&format!(
                "{},{},{},{},{},{},{},{},{}\n",
                csv_cell(&t.id), csv_cell(&t.created_at.to_string()),
                t.duration_ms, t.word_count, t.llm_applied,
                csv_cell(&t.text), csv_cell(t.raw_text.as_deref().unwrap_or("")),
                csv_cell(t.capture_error.as_deref().unwrap_or("")),
                csv_cell(t.cleanup_suggestion.as_deref().unwrap_or("")),
            ));
        }
        csv
    }

    fn ensure_loaded(&self) -> Result<(), String> {
        match &self.load_error {
            Some(error) => Err(format!("History could not be loaded; the original file has been preserved. {error}")),
            None => Ok(()),
        }
    }

    fn save_items(&self, items: &[Transcription]) -> Result<(), String> {
        self.ensure_loaded()?;
        let data = serde_json::to_vec(items)
            .map_err(|e| format!("Failed to serialize transcriptions: {e}"))?;
        crate::persistence::write_atomic(&self.path, &data)
            .map_err(|e| format!("History could not be saved: {e}"))
    }

    /// Load transcriptions from the persistent JSON file at the given path.
    #[cfg(test)]
    pub fn load(path: &PathBuf) -> Result<Vec<Transcription>, String> {
        Self::load_from(path)
    }

    /// Internal load helper.
    fn load_from(path: &PathBuf) -> Result<Vec<Transcription>, String> {
        let data = match std::fs::read_to_string(path) {
            Ok(data) => data,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(format!("Failed to read transcriptions file: {error}")),
        };
        serde_json::from_str(&data)
            .map_err(|e| format!("Failed to parse transcriptions file: {}", e))
    }

    /// Get a clone of all transcriptions.
    pub fn get_all(&self) -> Vec<Transcription> {
        self.items.clone()
    }
}

/// Get the persistent storage file path.
#[cfg(not(test))]
fn storage_path() -> Result<PathBuf, String> {
    let data_dir = dirs::data_dir().ok_or("Could not determine data directory")?;
    let app_dir = data_dir.join("com.sottoasr.app");
    std::fs::create_dir_all(&app_dir)
        .map_err(|e| format!("Failed to create app data dir: {}", e))?;
    Ok(app_dir.join("transcriptions.json"))
}

/// Pipeline tests must never read or write the user's real dictation history.
#[cfg(test)]
fn storage_path() -> Result<PathBuf, String> {
    static TEST_DIR: std::sync::LazyLock<tempfile::TempDir> =
        std::sync::LazyLock::new(|| tempfile::tempdir().expect("test history directory"));
    Ok(TEST_DIR.path().join("transcriptions.json"))
}

#[cfg(test)]
static FAILING_SAVES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// The next save of this entry fails once, as a full disk would. Keyed by ID
/// so a test running in parallel cannot consume another test's failure.
#[cfg(test)]
pub(crate) fn fail_next_save(id: &str) {
    FAILING_SAVES.lock().unwrap_or_else(|e| e.into_inner()).push(id.to_owned());
}

#[cfg(test)]
fn take_injected_failure(id: &str) -> Result<(), String> {
    let mut failing = FAILING_SAVES.lock().unwrap_or_else(|e| e.into_inner());
    match failing.iter().position(|item| item == id) {
        Some(index) => {
            failing.swap_remove(index);
            Err("History could not be saved: injected test failure".into())
        }
        None => Ok(()),
    }
}

/// Persistent transcription store — survives app restarts and reinstalls.
/// Stored at ~/Library/Application Support/com.sottoasr.app/transcriptions.json
static TRANSCRIPTIONS: std::sync::LazyLock<Result<tokio::sync::Mutex<TranscriptionStore>, String>> =
    std::sync::LazyLock::new(|| {
        let store = TranscriptionStore::new(storage_path()?);
        log::info!("Loaded {} transcriptions from disk", store.items.len());
        Ok(tokio::sync::Mutex::new(store))
    });

/// File access, JSON serialization, and the first lazy load belong on a worker,
/// never on the async executor used by recording and settings commands.
async fn with_store<T: Send + 'static>(
    operation: impl FnOnce(&mut TranscriptionStore) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(move || {
        let store = TRANSCRIPTIONS.as_ref().map_err(Clone::clone)?;
        operation(&mut store.blocking_lock())
    }).await.map_err(|e| format!("History storage task failed: {e}"))?
}

#[tauri::command]
pub async fn get_transcriptions() -> Result<Vec<Transcription>, String> {
    with_store(|store| {
        store.ensure_loaded()?;
        Ok(store.get_all())
    }).await
}

/// Deleting history also deletes its retained audio: the dialog names it.
/// The recovery claim keeps a reprocess from losing the file it reads.
#[tauri::command]
pub async fn delete_transcription(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let files = super::recovery::history_files(&app);
    let removed_audio = {
        let _claim = super::recovery::RecoveryClaim::acquire(&state)?;
        let _io = super::recovery::RECOVERY_IO.lock().await;
        with_store(move |store| super::recovery::delete_history_entry(store, &id, files)).await?
    };
    if removed_audio {
        let _ = app.emit("recovery-recordings-changed", ());
    }
    Ok(())
}

#[tauri::command]
pub async fn clear_transcriptions(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    let files = super::recovery::history_files(&app);
    let (removed, removed_audio) = {
        let _claim = super::recovery::RecoveryClaim::acquire(&state)?;
        let _io = super::recovery::RECOVERY_IO.lock().await;
        with_store(move |store| super::recovery::clear_history(store, files)).await?
    };
    if removed_audio {
        let _ = app.emit("recovery-recordings-changed", ());
    }
    Ok(removed)
}

/// Retention's history half. The caller holds the recovery claim and lock,
/// and removes the audio of the returned entries.
pub(super) async fn expire_history(cutoff: DateTime<Utc>) -> Result<Vec<Transcription>, String> {
    with_store(move |store| store.expire(cutoff)).await
}

const CSV_EXPORT_PREFIX: &str = "SottoASR-transcriptions-";

fn csv_export_name(date: chrono::NaiveDate, id: uuid::Uuid) -> String {
    format!("{CSV_EXPORT_PREFIX}{}-{id}.csv", date.format("%Y-%m-%d"))
}

/// True only for names `write_csv_export` produces.
fn is_csv_export_name(name: &str) -> bool {
    let Some(stem) = name.strip_prefix(CSV_EXPORT_PREFIX).and_then(|rest| rest.strip_suffix(".csv")) else {
        return false;
    };
    let (Some(date), Some(id)) = (stem.get(..10), stem.get(11..)) else {
        return false;
    };
    match (chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d"), uuid::Uuid::parse_str(id)) {
        (Ok(date), Ok(id)) => csv_export_name(date, id) == name,
        _ => false,
    }
}

/// Export through native file I/O: WKWebView has no browser download handler.
/// Reserve a unique name before atomic replacement, so an existing export is
/// never overwritten and success means the complete CSV reached disk.
fn write_csv_export(directory: &Path, csv: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(directory).map_err(|error| format!("Could not access Downloads: {error}"))?;
    let path = directory.join(csv_export_name(Utc::now().date_naive(), uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let reservation = options.open(&path).map_err(|error| format!("Could not create CSV export: {error}"))?;
    drop(reservation);
    if let Err(error) = crate::persistence::write_atomic(&path, csv.as_bytes()) {
        let _ = std::fs::remove_file(&path);
        return Err(format!("Could not save CSV export: {error}"));
    }
    Ok(path)
}

#[tauri::command]
pub async fn export_transcriptions_csv_file() -> Result<String, String> {
    with_store(|store| {
        store.ensure_loaded()?;
        let directory = dirs::download_dir().ok_or("Could not locate your Downloads folder")?;
        write_csv_export(&directory, &store.export_csv()).map(|path| path.to_string_lossy().into_owned())
    }).await
}

/// The webview supplies this path, so only an export this app wrote directly
/// into Downloads, as a regular file and not a link, may reach Finder.
fn checked_csv_export(path: &Path, downloads: &Path) -> Result<PathBuf, String> {
    let refused = || String::from("Only a CSV export in your Downloads folder can be shown.");
    let name = path.file_name().and_then(|name| name.to_str()).ok_or_else(refused)?;
    if path.parent() != Some(downloads) || !is_csv_export_name(name) {
        return Err(refused());
    }
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("The export is no longer in Downloads: {error}"))?;
    if !metadata.file_type().is_file() {
        return Err(refused());
    }
    Ok(path.to_path_buf())
}

#[tauri::command]
pub async fn reveal_csv_export(path: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let downloads = dirs::download_dir().ok_or("Could not locate your Downloads folder")?;
        let path = checked_csv_export(Path::new(&path), &downloads)?;
        #[cfg(target_os = "macos")]
        {
            let status = std::process::Command::new("/usr/bin/open").arg("-R").arg(path).status()
                .map_err(|error| format!("Could not open Finder: {error}"))?;
            if status.success() { Ok(()) } else { Err("Finder could not show the export.".into()) }
        }
        #[cfg(not(target_os = "macos"))]
        { let _ = path; Err("Showing exports is supported on macOS".into()) }
    }).await.map_err(|error| format!("Could not open Finder: {error}"))?
}

/// Entries the 5,000-entry cap removes lose their audio and receipt, except
/// interrupted ones: that audio is still the user's recovery copy.
pub async fn add_transcription(transcription: Transcription) -> Result<Vec<String>, String> {
    // Lock order is recovery I/O, then the store, as in the restart scan.
    let _io = super::recovery::RECOVERY_IO.lock().await;
    with_store(move |store| {
        // A second instance cannot record or reprocess, so it never saves here.
        super::recovery::add_history_entry(store, transcription, || {
            super::recovery::RecordingFiles::resolve(None)
        })
    }).await
}

/// Recovery must not mistake the in-memory copy of a failed save for durability.
pub(crate) async fn persisted_transcriptions() -> Result<Vec<Transcription>, String> {
    with_store(|store| {
        store.ensure_loaded()?;
        TranscriptionStore::load_from(&store.path)
    }).await
}

/// Only a successful durable save can acknowledge the existing retention policy.
/// Failed saves keep recovery-only entries without telling the UI to remove data.
pub fn emit_transcription(app: &tauri::AppHandle, transcription: &Transcription, saved: &Result<Vec<String>, String>) {
    #[derive(Clone, serde::Serialize)]
    struct SavedEvent<'a> {
        #[serde(flatten)]
        transcription: &'a Transcription,
        removed_ids: &'a [String],
    }
    let _ = app.emit("transcription-complete", SavedEvent {
        transcription, removed_ids: saved.as_ref().map(Vec::as_slice).unwrap_or_default(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn retry_replaces_partial_or_unsaved_result_without_duplicate_history() {
        let (_directory, mut store) = temp_store();
        store.add(make_transcription("recording", "partial speech")).unwrap();
        store.add(make_transcription("recording", "complete recovered speech")).unwrap();
        let persisted = TranscriptionStore::load(&store.path).unwrap();
        assert_eq!(persisted.len(), 1);
        assert_eq!(persisted[0].text, "complete recovered speech");
    }

    #[test]
    fn failed_history_save_is_not_a_recovery_acknowledgement() {
        let (directory, mut store) = temp_store();
        let path = directory.path().join("blocked");
        std::fs::create_dir(&path).unwrap();
        store.path = path.clone();
        assert!(store.add(make_transcription("recording", "unsaved speech")).is_err());
        assert_eq!(store.get_all()[0].text, "unsaved speech");
        assert!(TranscriptionStore::load_from(&path).is_err());
        store.path = directory.path().join("restored.json");
        store.add(make_transcription("recording", "recovered speech")).unwrap();
        let saved = TranscriptionStore::load(&store.path).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].text, "recovered speech");
    }

    fn make_transcription(id: &str, text: &str) -> Transcription {
        Transcription {
            id: id.into(),
            text: text.into(),
            duration_ms: 1000,
            created_at: Utc::now(),
            word_count: text.split_whitespace().count(),
            cancelled: false,
            capture_error: None,
            raw_text: None,
            cleanup_suggestion: None,
            llm_applied: false,
            llm_cleanup_status: crate::models::LlmCleanupStatus::Idle,
            saved_at: None,
        }
    }

    fn temp_store() -> (TempDir, TranscriptionStore) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("transcriptions.json");
        let store = TranscriptionStore::new(path);
        (dir, store)
    }

    fn ids(entries: &[Transcription]) -> Vec<&str> {
        entries.iter().map(|entry| entry.id.as_str()).collect()
    }

    #[test]
    fn first_save_records_saved_at_and_later_saves_keep_it() {
        let (_directory, mut store) = temp_store();
        let mut recovered = make_transcription("recovered", "speech from an old recording");
        recovered.created_at = Utc::now() - chrono::Duration::days(400);
        let before = Utc::now();
        store.add(recovered).unwrap();
        let saved_at = store.get_all()[0].saved_at.expect("store sets saved_at");
        assert!(saved_at >= before);
        assert_eq!(TranscriptionStore::load(&store.path).unwrap()[0].saved_at, Some(saved_at));
        let mut kept = make_transcription("kept", "text");
        kept.saved_at = Some(saved_at - chrono::Duration::days(2));
        store.add(kept.clone()).unwrap();
        assert_eq!(store.get_all()[0].saved_at, kept.saved_at);
    }

    #[test]
    fn expire_removes_only_entries_saved_before_the_cutoff() {
        let (_directory, mut store) = temp_store();
        let now = Utc::now();
        let mut recovered = make_transcription("recovered", "recent save of old audio");
        recovered.created_at = now - chrono::Duration::days(400);
        recovered.saved_at = Some(now - chrono::Duration::days(1));
        let mut expired = make_transcription("expired", "old");
        expired.saved_at = Some(now - chrono::Duration::days(40));
        store.add(expired).unwrap();
        store.add(recovered).unwrap();
        let mut older_build = make_transcription("older-build", "saved without saved_at");
        older_build.created_at = now - chrono::Duration::days(40);
        store.items.push(older_build);
        let removed = store.expire(now - chrono::Duration::days(30)).unwrap();
        assert_eq!(ids(&removed), ["expired", "older-build"]);
        assert_eq!(ids(&store.get_all()), ["recovered"]);
        assert_eq!(ids(&TranscriptionStore::load(&store.path).unwrap()), ["recovered"]);
        assert!(store.expire(now - chrono::Duration::days(30)).unwrap().is_empty());
    }

    #[test]
    fn injected_save_failure_hits_only_the_named_entry_once() {
        let (_directory, mut store) = temp_store();
        let id = uuid::Uuid::new_v4().to_string();
        fail_next_save(&id);
        store.add(make_transcription("other", "unaffected")).unwrap();
        assert!(store.add(make_transcription(&id, "unsaved")).is_err());
        assert_eq!(store.get_all()[0].text, "unsaved");
        assert_eq!(ids(&TranscriptionStore::load(&store.path).unwrap()), ["other"]);
        store.add(make_transcription(&id, "saved on retry")).unwrap();
        assert_eq!(TranscriptionStore::load(&store.path).unwrap()[0].text, "saved on retry");
    }

    #[test]
    fn only_this_apps_exports_in_downloads_can_be_revealed() {
        let downloads = TempDir::new().unwrap();
        let elsewhere = TempDir::new().unwrap();
        let export = write_csv_export(downloads.path(), "text\n").unwrap();
        assert!(is_csv_export_name(export.file_name().unwrap().to_str().unwrap()));
        assert_eq!(checked_csv_export(&export, downloads.path()).unwrap(), export);
        let foreign = write_csv_export(elsewhere.path(), "text\n").unwrap();
        assert!(checked_csv_export(&foreign, downloads.path()).is_err());
        let dotted = downloads.path().join("..").join(downloads.path().file_name().unwrap())
            .join(export.file_name().unwrap());
        assert!(checked_csv_export(&dotted, downloads.path()).is_err());
        let renamed = downloads.path().join("notes.csv");
        std::fs::write(&renamed, "private").unwrap();
        assert!(checked_csv_export(&renamed, downloads.path()).is_err());
        let missing = downloads.path().join(csv_export_name(Utc::now().date_naive(), uuid::Uuid::new_v4()));
        assert!(checked_csv_export(&missing, downloads.path()).is_err());
        for name in ["SottoASR-transcriptions-2026-13-01-00000000-0000-0000-0000-000000000000.csv",
            "SottoASR-transcriptions-2026-11-19-00000000-0000-0000-0000-000000000000.csv.app",
            "SottoASR-transcriptions-2026-11-19-0000000000000000000000000000000.csv",
            "SottoASR-transcriptions-2026-11-19_00000000-0000-0000-0000-000000000000.csv"] {
            assert!(!is_csv_export_name(name), "{name}");
        }
        #[cfg(unix)]
        {
            let link = downloads.path().join(csv_export_name(Utc::now().date_naive(), uuid::Uuid::new_v4()));
            std::os::unix::fs::symlink(&renamed, &link).unwrap();
            assert!(checked_csv_export(&link, downloads.path()).is_err());
        }
    }

    #[test]
    fn native_csv_export_is_complete_private_and_does_not_overwrite() {
        let directory = TempDir::new().unwrap();
        let csv = "text\n\"Complete, preserved transcript.\"\n";
        let first = write_csv_export(directory.path(), csv).unwrap();
        let second = write_csv_export(directory.path(), "second export").unwrap();
        assert_ne!(first, second);
        assert_eq!(std::fs::read_to_string(&first).unwrap(), csv);
        assert_eq!(std::fs::read_to_string(second).unwrap(), "second export");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(first).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn native_csv_export_reports_unwritable_destination_without_changing_it() {
        let directory = TempDir::new().unwrap();
        let occupied = directory.path().join("not-a-directory");
        std::fs::write(&occupied, "keep original").unwrap();
        assert!(write_csv_export(&occupied, "new export").is_err());
        assert_eq!(std::fs::read_to_string(occupied).unwrap(), "keep original");
    }

    #[test]
    fn new_store_starts_empty() {
        let (_dir, store) = temp_store();
        assert!(store.get_all().is_empty());
    }

    #[test]
    fn add_inserts_at_front() {
        let (_dir, mut store) = temp_store();
        store.add(make_transcription("1", "first")).unwrap();
        store.add(make_transcription("2", "second")).unwrap();
        let items = store.get_all();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "2");
        assert_eq!(items[1].id, "1");
    }

    #[test]
    fn add_truncates_at_5000() {
        let (_dir, mut store) = temp_store();
        store.items = (0..5000).map(|i| make_transcription(&i.to_string(), "text")).collect();
        let removed = store.add(make_transcription("new", "newest")).unwrap();
        assert_eq!(ids(&removed), ["4999"]);
        assert_eq!(store.items.len(), 5000);
        assert_eq!(store.items[0].id, "new");
        assert_eq!(store.items[4999].id, "4998");
        assert_eq!(TranscriptionStore::load(&store.path).unwrap().len(), 5000);
    }

    #[test]
    fn failed_rollover_keeps_recovery_entries_and_only_acknowledges_durable_evictions() {
        let (directory, mut store) = temp_store();
        std::fs::create_dir(&store.path).unwrap();
        store.items = (0..5000).map(|i| make_transcription(&i.to_string(), "saved text")).collect();
        assert!(store.add(make_transcription("recovery", "unsaved dictation")).is_err());
        assert_eq!(store.items.len(), 5001);
        assert_eq!(store.items.last().unwrap().id, "4999");
        assert!(store.path.is_dir());
        store.path = directory.path().join("recovered.json");
        let removed = store.add(make_transcription("next", "next dictation")).unwrap();
        assert_eq!(ids(&removed), ["4998", "4999"]);
        assert_eq!(store.items[1].id, "recovery");
        let persisted = TranscriptionStore::load(&store.path).unwrap();
        assert_eq!(persisted[1].text, "unsaved dictation");
    }

    #[test]
    fn delete_removes_by_id() {
        let (_dir, mut store) = temp_store();
        store.add(make_transcription("a", "hello")).unwrap();
        store.add(make_transcription("b", "world")).unwrap();
        store.delete("a").unwrap();
        let items = store.get_all();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "b");
    }

    #[test]
    fn delete_nonexistent_id_is_ok() {
        let (_dir, mut store) = temp_store();
        store.add(make_transcription("a", "hello")).unwrap();
        assert!(store.delete("nonexistent").is_ok());
        assert_eq!(store.get_all().len(), 1);
    }

    #[test]
    fn clear_removes_all() {
        let (_dir, mut store) = temp_store();
        store.add(make_transcription("1", "one")).unwrap();
        store.add(make_transcription("2", "two")).unwrap();
        store.clear().unwrap();
        assert!(store.get_all().is_empty());
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("transcriptions.json");

        {
            let mut store = TranscriptionStore::new(path.clone());
            store.add(make_transcription("rt1", "round trip")).unwrap();
        }

        let loaded = TranscriptionStore::load(&path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "rt1");
        assert_eq!(loaded[0].text, "round trip");
    }

    #[test]
    fn load_nonexistent_returns_empty() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("does_not_exist.json");
        let loaded = TranscriptionStore::load(&path).unwrap();
        assert!(loaded.is_empty());
    }

    #[test]
    fn load_invalid_json_returns_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "not valid json").unwrap();
        assert!(TranscriptionStore::load(&path).is_err());
    }

    #[test]
    fn export_csv_header() {
        let (_dir, store) = temp_store();
        let csv = store.export_csv();
        assert!(csv.starts_with("id,created_at,duration_ms,word_count,llm_applied,text,raw_text,capture_error,cleanup_suggestion\n"));
    }

    #[test]
    fn export_csv_contains_data() {
        let (_dir, mut store) = temp_store();
        store.add(make_transcription("csv1", "hello world")).unwrap();
        let csv = store.export_csv();
        assert!(csv.contains("csv1"));
        assert!(csv.contains("hello world"));
    }

    #[test]
    fn export_csv_escapes_quotes() {
        let (_dir, mut store) = temp_store();
        let mut t = make_transcription("q1", r#"she said "hello""#);
        t.raw_text = Some(r#"she said "hi""#.into());
        store.add(t).unwrap();
        let csv = store.export_csv();
        // Quotes should be doubled inside CSV fields
        assert!(csv.contains(r#"she said ""hello"""#));
        assert!(csv.contains(r#"she said ""hi"""#));
    }

    #[test]
    fn csv_preserves_paragraphs_quotes_and_capture_failure_metadata() {
        let (_directory, mut store) = temp_store();
        let mut transcription = make_transcription("partial", "First line.\r\nSecond \"quoted\" line.");
        transcription.capture_error = Some("Input disconnected, device \"USB\"".into());
        store.add(transcription).unwrap();
        let csv = store.export_csv();
        assert!(csv.contains("\"First line.\r\nSecond \"\"quoted\"\" line.\""));
        assert!(csv.contains("\"Input disconnected, device \"\"USB\"\"\""));
    }

    #[test]
    fn csv_marks_formula_prefixes_as_text_and_keeps_stored_suggestions_unchanged() {
        for value in ["=1+1", "+SUM(1,2)", "-2", "@SUM(1)", "\t=1", "\r=1", "\n=1", "  =1", "＝1", "＋1", "－1", "＠x"] {
            assert!(csv_cell(value).starts_with("\"'"), "{value:?}");
        }
        assert_eq!(csv_cell("Safe, \"quoted\"\ntext"), "\"Safe, \"\"quoted\"\"\ntext\"");
        let (_directory, mut store) = temp_store();
        let mut transcription = make_transcription("formula", "=1+1");
        transcription.raw_text = Some("+SUM(1,2)".into());
        transcription.capture_error = Some("@device".into());
        transcription.cleanup_suggestion = Some("-2".into());
        transcription.saved_at = Some(Utc::now());
        store.add(transcription.clone()).unwrap();
        let exported = store.export_csv();
        for cell in ["\"'=1+1\"", "\"'+SUM(1,2)\"", "\"'@device\"", "\"'-2\""] {
            assert!(exported.contains(cell), "{cell}");
        }
        assert_eq!(store.items[0], transcription);
        assert_eq!(TranscriptionStore::load(&store.path).unwrap()[0], transcription);
    }

    #[test]
    fn export_csv_with_llm_fields() {
        let (_dir, mut store) = temp_store();
        let mut t = make_transcription("llm1", "cleaned text");
        t.raw_text = Some("raw uh text".into());
        t.llm_applied = true;
        store.add(t).unwrap();
        let csv = store.export_csv();
        assert!(csv.contains("true"));
        assert!(csv.contains("raw uh text"));
    }

    #[test]
    fn persistence_across_store_instances() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("transcriptions.json");

        {
            let mut store = TranscriptionStore::new(path.clone());
            store.add(make_transcription("p1", "persistent")).unwrap();
            store.add(make_transcription("p2", "data")).unwrap();
        }

        let store2 = TranscriptionStore::new(path);
        let items = store2.get_all();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "p2");
        assert_eq!(items[1].id, "p1");
    }

    #[test]
    fn failed_deletion_keeps_memory_and_disk_unchanged() {
        let (dir, mut store) = temp_store();
        store.add(make_transcription("keep", "important text")).unwrap();
        let original_path = store.path.clone();
        let original = std::fs::read(&original_path).unwrap();
        // A file cannot be used as a parent directory: deterministic I/O failure.
        store.path = dir.path().join("transcriptions.json/blocked.json");
        assert!(store.delete("keep").is_err());
        assert!(store.clear().is_err());
        assert_eq!(store.items.len(), 1);
        assert_eq!(std::fs::read(original_path).unwrap(), original);
    }

    #[test]
    fn failed_add_keeps_new_dictation_in_memory() {
        let (dir, mut store) = temp_store();
        std::fs::write(dir.path().join("blocked"), "file").unwrap();
        store.path = dir.path().join("blocked/history.json");
        assert!(store.add(make_transcription("new", "recoverable text")).is_err());
        assert_eq!(store.items[0].text, "recoverable text");
    }

    #[test]
    fn invalid_history_is_never_overwritten_by_fallback() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("transcriptions.json");
        let damaged = b"[{\"id\":\"valuable-but-incomplete";
        std::fs::write(&path, damaged).unwrap();
        let mut store = TranscriptionStore::new(path.clone());
        assert!(store.ensure_loaded().is_err());
        assert!(store.add(make_transcription("new", "new text")).is_err());
        assert!(store.clear().is_err());
        assert!(store.delete("new").is_err());
        assert_eq!(std::fs::read(path).unwrap(), damaged);
        assert_eq!(store.items[0].text, "new text");
    }
}
