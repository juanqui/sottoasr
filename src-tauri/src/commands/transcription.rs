use tauri::State;
use crate::state::AppState;
use crate::models::Transcription;
use std::path::PathBuf;

/// Persistent transcription store — survives app restarts and reinstalls.
/// Stored at ~/Library/Application Support/com.sotto.app/transcriptions.json
static TRANSCRIPTIONS: std::sync::LazyLock<tokio::sync::Mutex<Vec<Transcription>>> =
    std::sync::LazyLock::new(|| {
        let items = load_from_disk().unwrap_or_default();
        log::info!("Loaded {} transcriptions from disk", items.len());
        tokio::sync::Mutex::new(items)
    });

/// Get the persistent storage file path.
fn storage_path() -> Result<PathBuf, String> {
    let data_dir = dirs::data_dir().ok_or("Could not determine data directory")?;
    let app_dir = data_dir.join("com.sotto.app");
    std::fs::create_dir_all(&app_dir)
        .map_err(|e| format!("Failed to create app data dir: {}", e))?;
    Ok(app_dir.join("transcriptions.json"))
}

/// Load transcriptions from the persistent JSON file.
fn load_from_disk() -> Result<Vec<Transcription>, String> {
    let path = storage_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let data = std::fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read transcriptions file: {}", e))?;
    serde_json::from_str(&data)
        .map_err(|e| format!("Failed to parse transcriptions file: {}", e))
}

/// Save transcriptions to the persistent JSON file.
async fn save_to_disk(items: &[Transcription]) -> Result<(), String> {
    let path = storage_path()?;
    let data = serde_json::to_string_pretty(items)
        .map_err(|e| format!("Failed to serialize transcriptions: {}", e))?;
    std::fs::write(&path, data)
        .map_err(|e| format!("Failed to write transcriptions file: {}", e))?;
    Ok(())
}

#[tauri::command]
pub async fn get_transcriptions() -> Result<Vec<Transcription>, String> {
    let store = TRANSCRIPTIONS.lock().await;
    Ok(store.clone())
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
    store.retain(|t| t.id != id);
    save_to_disk(&store).await?;
    Ok(())
}

#[tauri::command]
pub async fn clear_transcriptions() -> Result<(), String> {
    let mut store = TRANSCRIPTIONS.lock().await;
    store.clear();
    save_to_disk(&store).await?;
    Ok(())
}

/// Export all transcriptions as CSV.
#[tauri::command]
pub async fn export_transcriptions_csv() -> Result<String, String> {
    let store = TRANSCRIPTIONS.lock().await;
    let mut csv = String::from("id,created_at,duration_ms,word_count,llm_applied,text,raw_text\n");
    for t in store.iter() {
        let text_escaped = t.text.replace('"', "\"\"");
        let raw_escaped = t.raw_text.as_deref().unwrap_or("").replace('"', "\"\"");
        csv.push_str(&format!(
            "{},{}.,{},{},{},\"{}\",\"{}\"\n",
            t.id, t.created_at, t.duration_ms, t.word_count, t.llm_applied,
            text_escaped, raw_escaped,
        ));
    }
    Ok(csv)
}

pub async fn add_transcription(transcription: Transcription) {
    let mut store = TRANSCRIPTIONS.lock().await;
    store.insert(0, transcription);
    // Keep only max items
    if store.len() > 5000 {
        store.truncate(5000);
    }
    if let Err(e) = save_to_disk(&store).await {
        log::error!("Failed to persist transcription: {}", e);
    }
}
