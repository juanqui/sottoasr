//! The choice between the local speech model and a cloud speech service.
//!
//! One engine object is installed at a time. A change of the engine setting
//! replaces the object, so with a cloud engine no local engine exists and no
//! local model can be in memory.

use std::sync::atomic::Ordering;

use tauri::{AppHandle, Emitter, Manager, State};

use crate::aws::profiles::{self as aws_profiles, AwsProfile};
use crate::asr::aws_transcribe::{self, ConnectionTest};
use crate::asr::engine::create_engine;
use crate::models::{AwsTranscribeSettings, Settings, SpeechEngine};
use crate::state::AppState;

#[derive(serde::Serialize)]
pub struct AwsRegion {
    code: &'static str,
    name: &'static str,
}

/// What the Settings window offers for AWS Transcribe.
#[derive(serde::Serialize)]
pub struct AwsTranscribeOptions {
    profiles: Vec<AwsProfile>,
    /// Why the AWS files could not be read.
    profiles_error: Option<String>,
    regions: Vec<AwsRegion>,
    languages: &'static [&'static str],
}

#[tauri::command]
pub async fn get_aws_transcribe_options() -> Result<AwsTranscribeOptions, String> {
    let (profiles, profiles_error) = match aws_profiles::list().await {
        Ok(profiles) => (profiles, None),
        Err(error) => (Vec::new(), Some(error)),
    };
    Ok(AwsTranscribeOptions {
        profiles,
        profiles_error,
        regions: aws_transcribe::REGIONS.iter().map(|&(code, name)| AwsRegion { code, name }).collect(),
        languages: aws_transcribe::languages(),
    })
}

/// Makes one request with the options in the Settings window, saved or not.
/// It sends one second of silence and does not use the installed engine.
#[tauri::command]
pub async fn test_aws_transcribe(options: AwsTranscribeSettings) -> Result<ConnectionTest, String> {
    let tested = aws_transcribe::test_connection(&options).await;
    match &tested {
        Ok(test) => log::info!("AWS Transcribe connection test passed in {:.1}s ({})", test.seconds, test.region),
        Err(error) => log::warn!("AWS Transcribe connection test failed: {error}"),
    }
    tested
}

/// Whether a Save changed which engine must be installed: the engine itself,
/// or the options of the cloud engine while it is selected.
fn engine_changed(previous: &Settings, current: &Settings) -> bool {
    previous.speech_engine != current.speech_engine
        || (current.speech_engine == SpeechEngine::AwsTranscribe && previous.aws_transcribe != current.aws_transcribe)
}

/// Called after preferences are saved. The change runs as a task, so that
/// Save does not wait for a model load that holds the engine.
pub fn settings_saved(app: AppHandle, previous: &Settings, current: &Settings) {
    if !engine_changed(previous, current) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let state: State<'_, AppState> = app.state();
        let cloud = install_engine(&state).await;
        log::info!("Speech engine changed: {}", crate::commands::setup::backend_name(&state));
        if !cloud {
            let _ = app.emit("asr-init-started", serde_json::json!({ "backend": crate::commands::setup::backend_name(&state) }));
        }
        // Local: verifies, downloads if necessary, and loads. Cloud: reports ready.
        if let Err(error) = crate::commands::setup::init_asr(app.clone(), state).await {
            log::error!("The speech engine did not start: {error}");
        }
    });
}

/// Puts the engine for the current settings in place of the installed one
/// and drops that one, which releases a local model. A transcription or a
/// model load that holds the engine finishes first. Returns whether the new
/// engine is a cloud engine.
///
/// The settings are read with the engine held, so two changes in quick
/// succession end in the engine of the later one.
pub(crate) async fn install_engine(state: &AppState) -> bool {
    let mut installed = state.asr_engine.lock().await;
    let settings = state.settings.lock().await.clone();
    let cloud = settings.speech_engine == SpeechEngine::AwsTranscribe;
    let previous = std::mem::replace(&mut *installed, create_engine(&settings));
    state.cloud_speech.store(cloud, Ordering::SeqCst);
    // A cloud engine can transcribe at once. A local engine must load first.
    state.is_model_loaded.store(cloud, Ordering::SeqCst);
    *state.asr_init_error.lock().unwrap_or_else(|error| error.into_inner()) = None;
    {
        // The vocabulary model was attached to the engine that is dropped.
        let mut vocabulary = state.vocabulary_runtime.lock().unwrap_or_else(|error| error.into_inner());
        vocabulary.loaded = false;
        vocabulary.error = None;
    }
    drop(installed);
    // Dropping a local engine frees its CoreML models: not on an async worker.
    let _ = tokio::task::spawn_blocking(move || drop(previous)).await;
    cloud
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asr::engine::{AsrEngine, AsrResult};
    use crate::test_support::{MockAudioCapture, MockPasteBackend};
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    /// A local engine with a model "in memory" until it is dropped.
    struct Loaded(Arc<AtomicBool>);

    impl Drop for Loaded {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }

    impl AsrEngine for Loaded {
        fn init(&mut self) -> Result<(), String> { Ok(()) }
        fn is_ready(&self) -> bool { true }
        fn transcribe_file(&mut self, _: &str) -> Result<AsrResult, String> { Err("not used".into()) }
        fn transcribe_samples(&mut self, _: &[f32], _: u32) -> Result<AsrResult, String> { Err("not used".into()) }
        fn is_model_available(&self) -> bool { true }
        fn backend_name(&self) -> &'static str { "loaded local model" }
    }

    fn cloud_settings(profile: &str) -> Settings {
        Settings {
            speech_engine: SpeechEngine::AwsTranscribe,
            aws_transcribe: AwsTranscribeSettings { profile: Some(profile.into()), ..Default::default() },
            ..Default::default()
        }
    }

    fn state_with_loaded_model() -> (AppState, Arc<AtomicBool>) {
        let in_memory = Arc::new(AtomicBool::new(true));
        let state = AppState::new_with_backends(
            Box::new(MockAudioCapture::silence()),
            Box::new(Loaded(Arc::clone(&in_memory))),
            None,
            Box::new(MockPasteBackend::new()),
            Settings::default(),
        );
        (state, in_memory)
    }

    async fn installed_name(state: &AppState) -> &'static str {
        state.asr_engine.lock().await.backend_name()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_change_to_the_cloud_engine_releases_the_local_model_and_is_ready_at_once() {
        let (state, in_memory) = state_with_loaded_model();
        state.vocabulary_runtime.lock().unwrap().loaded = true;
        *state.asr_init_error.lock().unwrap() = Some("an earlier load failed".into());

        *state.settings.lock().await = cloud_settings("work");
        assert!(install_engine(&state).await);

        assert!(!in_memory.load(Ordering::SeqCst), "the local engine was dropped");
        assert_eq!(installed_name(&state).await, "AWS Transcribe");
        assert!(state.cloud_speech());
        assert!(state.is_model_loaded.load(Ordering::SeqCst), "nothing to load: dictation can start");
        assert!(state.asr_init_error.lock().unwrap().is_none());
        assert!(!state.vocabulary_runtime.lock().unwrap().loaded, "the vocabulary model went with the engine");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_change_back_to_the_local_engine_waits_for_its_model() {
        let (state, _) = state_with_loaded_model();
        *state.settings.lock().await = cloud_settings("work");
        install_engine(&state).await;

        *state.settings.lock().await = Settings::default();
        assert!(!install_engine(&state).await);
        assert!(!state.cloud_speech());
        assert_ne!(installed_name(&state).await, "AWS Transcribe");
        assert!(!state.is_model_loaded.load(Ordering::SeqCst), "a dictation is refused until the model is loaded");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_change_waits_for_the_engine_and_ends_in_the_latest_settings() {
        let (state, in_memory) = state_with_loaded_model();
        // A transcription or a model load holds the engine.
        let busy = state.asr_engine.clone().lock_owned().await;
        *state.settings.lock().await = cloud_settings("first");
        let change = install_engine(&state);
        tokio::pin!(change);
        assert!(futures_util::poll!(change.as_mut()).is_pending());
        assert!(in_memory.load(Ordering::SeqCst) && !state.cloud_speech(), "nothing changes while the engine is in use");

        // The user saves again before the first change could run.
        *state.settings.lock().await = Settings::default();
        drop(busy);
        assert!(!change.await, "the engine of the later settings is installed");
        assert!(!state.cloud_speech());
    }

    #[test]
    fn only_a_change_that_selects_another_engine_replaces_it() {
        let local = Settings::default();
        let cloud = cloud_settings("work");
        assert!(engine_changed(&local, &cloud));
        assert!(engine_changed(&cloud, &local));
        assert!(engine_changed(&cloud, &cloud_settings("other")));
        assert!(!engine_changed(&cloud, &cloud));
        // AWS options edited while the local engine stays selected.
        let edited = Settings { aws_transcribe: cloud.aws_transcribe.clone(), ..Default::default() };
        assert!(!engine_changed(&local, &edited));
        let unrelated = Settings { sound_cues: true, ..cloud.clone() };
        assert!(!engine_changed(&cloud, &unrelated));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_options_list_the_regions_and_languages_of_the_service() {
        let options = get_aws_transcribe_options().await.unwrap();
        assert_eq!(options.regions.len(), 20);
        assert!(options.regions.iter().any(|region| region.code == "us-west-2" && region.name == "US West (Oregon)"));
        assert!(options.languages.contains(&"en-US"));
    }
}
