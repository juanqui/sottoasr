use tauri::State;
use crate::state::AppState;
use crate::models::Transcription;
use std::path::PathBuf;

/// A testable store for transcriptions, backed by a JSON file on disk.
pub struct TranscriptionStore {
    items: Vec<Transcription>,
    path: PathBuf,
}

impl TranscriptionStore {
    /// Create a new store that persists to the given path.
    /// Loads existing transcriptions from disk if the file exists.
    pub fn new(path: PathBuf) -> Self {
        let items = Self::load_from(&path).unwrap_or_default();
        Self { items, path }
    }

    /// Add a transcription (newest first), capping at 5000 entries, and persist.
    pub fn add(&mut self, transcription: Transcription) {
        self.items.insert(0, transcription);
        if self.items.len() > 5000 {
            self.items.truncate(5000);
        }
        if let Err(e) = self.save() {
            log::error!("Failed to persist transcription: {}", e);
        }
    }

    /// Delete a transcription by ID and persist.
    pub fn delete(&mut self, id: &str) -> Result<(), String> {
        self.items.retain(|t| t.id != id);
        self.save()
    }

    /// Clear all transcriptions and persist.
    pub fn clear(&mut self) -> Result<(), String> {
        self.items.clear();
        self.save()
    }

    /// Export all transcriptions as CSV.
    pub fn export_csv(&self) -> String {
        let mut csv = String::from("id,created_at,duration_ms,word_count,llm_applied,text,raw_text\n");
        for t in self.items.iter() {
            let text_escaped = t.text.replace('"', "\"\"").replace('\n', " ").replace('\r', "");
            let raw_escaped = t.raw_text.as_deref().unwrap_or("").replace('"', "\"\"").replace('\n', " ").replace('\r', "");
            csv.push_str(&format!(
                "{},{},{},{},{},\"{}\",\"{}\"\n",
                t.id, t.created_at, t.duration_ms, t.word_count, t.llm_applied,
                text_escaped, raw_escaped,
            ));
        }
        csv
    }

    /// Save transcriptions to the persistent JSON file.
    pub fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create parent dir: {}", e))?;
        }
        let data = serde_json::to_string_pretty(&self.items)
            .map_err(|e| format!("Failed to serialize transcriptions: {}", e))?;
        std::fs::write(&self.path, data)
            .map_err(|e| format!("Failed to write transcriptions file: {}", e))?;
        Ok(())
    }

    /// Load transcriptions from the persistent JSON file at the given path.
    #[cfg(test)]
    pub fn load(path: &PathBuf) -> Result<Vec<Transcription>, String> {
        Self::load_from(path)
    }

    /// Internal load helper.
    fn load_from(path: &PathBuf) -> Result<Vec<Transcription>, String> {
        if !path.exists() {
            return Ok(Vec::new());
        }
        let data = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read transcriptions file: {}", e))?;
        serde_json::from_str(&data)
            .map_err(|e| format!("Failed to parse transcriptions file: {}", e))
    }

    /// Get a clone of all transcriptions.
    pub fn get_all(&self) -> Vec<Transcription> {
        self.items.clone()
    }
}

/// Get the persistent storage file path.
fn storage_path() -> Result<PathBuf, String> {
    let data_dir = dirs::data_dir().ok_or("Could not determine data directory")?;
    let app_dir = data_dir.join("com.sottoasr.app");
    std::fs::create_dir_all(&app_dir)
        .map_err(|e| format!("Failed to create app data dir: {}", e))?;
    Ok(app_dir.join("transcriptions.json"))
}

/// Persistent transcription store — survives app restarts and reinstalls.
/// Stored at ~/Library/Application Support/com.sottoasr.app/transcriptions.json
static TRANSCRIPTIONS: std::sync::LazyLock<tokio::sync::Mutex<TranscriptionStore>> =
    std::sync::LazyLock::new(|| {
        let path = storage_path().expect("Could not determine transcription storage path");
        let store = TranscriptionStore::new(path);
        log::info!("Loaded {} transcriptions from disk", store.items.len());
        tokio::sync::Mutex::new(store)
    });

#[tauri::command]
pub async fn get_transcriptions() -> Result<Vec<Transcription>, String> {
    let store = TRANSCRIPTIONS.lock().await;
    Ok(store.get_all())
}

#[tauri::command]
pub async fn get_last_transcription(
    state: State<'_, AppState>,
) -> Result<Option<Transcription>, String> {
    let last = state.last_transcription.lock().await;
    Ok(last.clone())
}

#[tauri::command]
pub async fn delete_transcription(id: String) -> Result<(), String> {
    let mut store = TRANSCRIPTIONS.lock().await;
    store.delete(&id)
}

#[tauri::command]
pub async fn clear_transcriptions() -> Result<(), String> {
    let mut store = TRANSCRIPTIONS.lock().await;
    store.clear()
}

/// Export all transcriptions as CSV.
#[tauri::command]
pub async fn export_transcriptions_csv() -> Result<String, String> {
    let store = TRANSCRIPTIONS.lock().await;
    Ok(store.export_csv())
}

pub async fn add_transcription(transcription: Transcription) {
    let mut store = TRANSCRIPTIONS.lock().await;
    store.add(transcription);
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tempfile::TempDir;

    fn make_transcription(id: &str, text: &str) -> Transcription {
        Transcription {
            id: id.into(),
            text: text.into(),
            duration_ms: 1000,
            created_at: Utc::now(),
            word_count: text.split_whitespace().count(),
            cancelled: false,
            raw_text: None,
            llm_applied: false,
            llm_cleanup_status: crate::models::LlmCleanupStatus::Idle,
        }
    }

    fn temp_store() -> (TempDir, TranscriptionStore) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("transcriptions.json");
        let store = TranscriptionStore::new(path);
        (dir, store)
    }

    #[test]
    fn new_store_starts_empty() {
        let (_dir, store) = temp_store();
        assert!(store.get_all().is_empty());
    }

    #[test]
    fn add_inserts_at_front() {
        let (_dir, mut store) = temp_store();
        store.add(make_transcription("1", "first"));
        store.add(make_transcription("2", "second"));
        let items = store.get_all();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "2");
        assert_eq!(items[1].id, "1");
    }

    #[test]
    fn add_truncates_at_5000() {
        let (_dir, mut store) = temp_store();
        for i in 0..5002 {
            store.add(make_transcription(&i.to_string(), "text"));
        }
        assert_eq!(store.get_all().len(), 5000);
    }

    #[test]
    fn delete_removes_by_id() {
        let (_dir, mut store) = temp_store();
        store.add(make_transcription("a", "hello"));
        store.add(make_transcription("b", "world"));
        store.delete("a").unwrap();
        let items = store.get_all();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "b");
    }

    #[test]
    fn delete_nonexistent_id_is_ok() {
        let (_dir, mut store) = temp_store();
        store.add(make_transcription("a", "hello"));
        assert!(store.delete("nonexistent").is_ok());
        assert_eq!(store.get_all().len(), 1);
    }

    #[test]
    fn clear_removes_all() {
        let (_dir, mut store) = temp_store();
        store.add(make_transcription("1", "one"));
        store.add(make_transcription("2", "two"));
        store.clear().unwrap();
        assert!(store.get_all().is_empty());
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("transcriptions.json");

        {
            let mut store = TranscriptionStore::new(path.clone());
            store.add(make_transcription("rt1", "round trip"));
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
        assert!(csv.starts_with("id,created_at,duration_ms,word_count,llm_applied,text,raw_text\n"));
    }

    #[test]
    fn export_csv_contains_data() {
        let (_dir, mut store) = temp_store();
        store.add(make_transcription("csv1", "hello world"));
        let csv = store.export_csv();
        assert!(csv.contains("csv1"));
        assert!(csv.contains("hello world"));
    }

    #[test]
    fn export_csv_escapes_quotes() {
        let (_dir, mut store) = temp_store();
        let mut t = make_transcription("q1", r#"she said "hello""#);
        t.raw_text = Some(r#"she said "hi""#.into());
        store.add(t);
        let csv = store.export_csv();
        // Quotes should be doubled inside CSV fields
        assert!(csv.contains(r#"she said ""hello"""#));
        assert!(csv.contains(r#"she said ""hi"""#));
    }

    #[test]
    fn export_csv_with_llm_fields() {
        let (_dir, mut store) = temp_store();
        let mut t = make_transcription("llm1", "cleaned text");
        t.raw_text = Some("raw uh text".into());
        t.llm_applied = true;
        store.add(t);
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
            store.add(make_transcription("p1", "persistent"));
            store.add(make_transcription("p2", "data"));
        }

        let store2 = TranscriptionStore::new(path);
        let items = store2.get_all();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "p2");
        assert_eq!(items[1].id, "p1");
    }
}
