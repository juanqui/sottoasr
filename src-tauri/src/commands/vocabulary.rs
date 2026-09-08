//! Optional auxiliary ASR model lifecycle. Dictation itself never downloads.

use tauri::{AppHandle, Emitter, Manager, State};

use crate::asr::{engine::{with_engine, prepare_and_attach, PreparedVocabulary}, vocabulary::DOWNLOAD_SIZE_MB};
use crate::models::Settings;
use crate::state::AppState;

#[derive(Default)]
pub struct VocabularyRuntime {
    pub loaded: bool,
    pub preparing: bool,
    pub error: Option<String>,
}

#[derive(Clone, serde::Serialize)]
pub struct VocabularyStatus {
    pub supported: bool,
    pub downloaded: bool,
    pub loaded: bool,
    pub preparing: bool,
    pub download_size_mb: u64,
    pub error: Option<String>,
}

fn cached_model_available() -> bool {
    if !cfg!(feature = "asr-fluidaudio") {
        return false;
    }
    let Some(directory) = dirs::data_dir() else { return false; };
    let directory = directory.join("FluidAudio/Models/parakeet-ctc-110m-coreml");
    if !["AudioEncoder.mlmodelc", "MelSpectrogram.mlmodelc"].iter()
        .all(|name| directory.join(name).is_dir())
    {
        return false;
    }
    let read_json = |name: &str| -> Option<serde_json::Value> {
        serde_json::from_slice(&std::fs::read(directory.join(name)).ok()?).ok()
    };
    let Some(vocabulary) = read_json("vocab.json") else { return false; };
    let Some(tokenizer) = read_json("tokenizer.json") else { return false; };
    vocabulary.is_object() && tokenizer_is_parseable(&tokenizer)
}

fn tokenizer_is_parseable(tokenizer: &serde_json::Value) -> bool {
    tokenizer["model"]["type"] == "BPE"
        && tokenizer["model"]["vocab"].as_object()
            .is_some_and(|vocabulary| vocabulary.values().all(|value| value.as_i64().is_some()))
        && tokenizer["model"]["merges"].as_array()
            .is_some_and(|merges| merges.iter().all(serde_json::Value::is_string))
}

async fn status(state: &AppState) -> Result<VocabularyStatus, String> {
    let downloaded = tokio::task::spawn_blocking(cached_model_available)
        .await.map_err(|error| format!("Unable to inspect vocabulary model: {error}"))?;
    let runtime = state.vocabulary_runtime.lock().unwrap_or_else(|error| error.into_inner());
    Ok(VocabularyStatus {
        supported: cfg!(feature = "asr-fluidaudio"),
        downloaded,
        loaded: runtime.loaded,
        preparing: runtime.preparing,
        download_size_mb: DOWNLOAD_SIZE_MB,
        error: runtime.error.clone(),
    })
}

#[tauri::command]
pub async fn get_vocabulary_status(state: State<'_, AppState>) -> Result<VocabularyStatus, String> {
    status(&state).await
}

#[tauri::command]
pub async fn prepare_vocabulary_model(app: AppHandle, state: State<'_, AppState>) -> Result<VocabularyStatus, String> {
    if !cfg!(feature = "asr-fluidaudio") {
        return Err("Vocabulary assistance is unavailable for this ASR backend".into());
    }
    if state.vocabulary_terms.lock().unwrap_or_else(|error| error.into_inner()).is_empty() {
        return Err("Save at least one vocabulary term before preparing its model".into());
    }
    start_preparation(app, true);
    status(&state).await
}

fn start_preparation(app: AppHandle, allow_download: bool) {
    let state: State<'_, AppState> = app.state();
    {
        let mut runtime = state.vocabulary_runtime.lock().unwrap_or_else(|error| error.into_inner());
        if runtime.preparing || runtime.loaded || !cfg!(feature = "asr-fluidaudio") {
            return;
        }
        runtime.preparing = true;
        runtime.error = None;
    }
    tauri::async_runtime::spawn(async move {
        let state: State<'_, AppState> = app.state();
        let _operation = state.vocabulary_operation.lock().await;
        let terms_empty = state.vocabulary_terms.lock().unwrap_or_else(|error| error.into_inner()).is_empty();
        let result = if terms_empty {
            Ok(false)
        } else {
            prepare_and_attach(&state.asr_engine, &state.vocabulary_terms, move || PreparedVocabulary::load(allow_download)).await
        };
        // A completed download must not resurrect a cleared word list.
        let app_for_unload = app.clone();
        let cleared = with_engine(&state.asr_engine, move |engine| {
            let state: State<'_, AppState> = app_for_unload.state();
            let empty = state.vocabulary_terms.lock().unwrap_or_else(|error| error.into_inner()).is_empty();
            if empty { engine.unload_vocabulary(); }
            Ok(empty)
        }).await.unwrap_or(false);
        // A new Save may arrive after a cleared list declined attachment but
        // before this task releases its single-flight flag. Honor that intent.
        let restart = matches!(result, Ok(false)) && !cleared;
        {
            let mut runtime = state.vocabulary_runtime.lock().unwrap_or_else(|error| error.into_inner());
            runtime.preparing = false;
            runtime.loaded = matches!(result, Ok(true)) && !cleared;
            runtime.error = if cleared { None } else { result.err() };
        }
        if let Ok(status) = status(&state).await {
            let _ = app.emit("vocabulary-status", status);
        }
        if restart { start_preparation(app.clone(), true); }
    });
}

/// Called after preferences have been persisted and acknowledged in memory.
/// This synchronous scheduling boundary does not delay Settings Save on download.
pub fn settings_saved(app: AppHandle, previous: &Settings, current: &Settings) {
    let state: State<'_, AppState> = app.state();
    *state.vocabulary_terms.lock().unwrap_or_else(|error| error.into_inner()) = current.vocabulary.clone();
    if previous.vocabulary == current.vocabulary {
        return;
    }
    if !current.vocabulary.is_empty() {
        start_preparation(app, true);
        return;
    }
    tauri::async_runtime::spawn(async move {
        let state: State<'_, AppState> = app.state();
        let _operation = state.vocabulary_operation.lock().await;
        let app_for_unload = app.clone();
        let unloaded = with_engine(&state.asr_engine, move |engine| {
            let state: State<'_, AppState> = app_for_unload.state();
            let empty = state.vocabulary_terms.lock().unwrap_or_else(|error| error.into_inner()).is_empty();
            if empty { engine.unload_vocabulary(); }
            Ok(empty)
        }).await.unwrap_or(false);
        if unloaded {
            let mut runtime = state.vocabulary_runtime.lock().unwrap_or_else(|error| error.into_inner());
            runtime.loaded = false;
            runtime.error = None;
        }
        if let Ok(status) = status(&state).await {
            let _ = app.emit("vocabulary-status", status);
        }
    });
}

/// Startup restores an existing auxiliary model without requesting a download.
pub fn restore_cached(app: AppHandle) {
    let state: State<'_, AppState> = app.state();
    if !state.vocabulary_terms.lock().unwrap_or_else(|error| error.into_inner()).is_empty() {
        start_preparation(app, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_rejects_tokenizers_that_the_sdk_cannot_parse() {
        let mut tokenizer = serde_json::json!({"model": {"type": "BPE", "vocab": {"q": 1}, "merges": []}});
        assert!(tokenizer_is_parseable(&tokenizer));
        tokenizer["model"]["vocab"]["q"] = serde_json::json!("invalid-id");
        assert!(!tokenizer_is_parseable(&tokenizer));
        tokenizer["model"]["vocab"]["q"] = serde_json::json!(1);
        tokenizer["model"]["merges"] = serde_json::json!([42]);
        assert!(!tokenizer_is_parseable(&tokenizer));
        tokenizer["model"]["merges"] = serde_json::json!([]);
        tokenizer["model"]["type"] = serde_json::json!("Unigram");
        assert!(!tokenizer_is_parseable(&tokenizer));
    }
}
