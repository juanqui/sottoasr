use tauri::State;
use crate::state::AppState;
use crate::models::Transcription;

// In-memory transcription store (persisted via tauri-plugin-store)
static TRANSCRIPTIONS: std::sync::LazyLock<tokio::sync::Mutex<Vec<Transcription>>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(Vec::new()));

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
    Ok(())
}

#[tauri::command]
pub async fn clear_transcriptions() -> Result<(), String> {
    let mut store = TRANSCRIPTIONS.lock().await;
    store.clear();
    Ok(())
}

pub async fn add_transcription(transcription: Transcription) {
    let mut store = TRANSCRIPTIONS.lock().await;
    store.insert(0, transcription);
    // Keep only max_history items (default 500)
    if store.len() > 500 {
        store.truncate(500);
    }
}
