//! Shared LLM cleanup helper used by both the production hotkey pipeline
//! (`hotkeys/manager.rs`) and the unit-testable pipeline (`pipeline.rs`).
//!
//! This module owns the single authoritative flow for running transcript
//! cleanup: ensure the sidecar is running, call it under a timeout, kill
//! orphaned subprocesses on failure, and report a structured
//! `LlmCleanupStatus`. See docs/specs/2026-04-11-llm-cleanup-reliability.md
//! §4.1 for the rationale.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::llm::engine::{ensure_running, is_zombie_error, kill_orphan};
use crate::llm::validation::{validate, MAX_CLEANUP_BYTES};
use crate::models::LlmCleanupStatus;
use crate::state::AppState;

/// Failure-recovery deadline for a blocked sidecar. Normal local cleanup is
/// much faster; input and output sizes are bounded separately. A timeout always
/// preserves the original transcript and terminates the orphaned process.
pub const LLM_CLEANUP_TIMEOUT: Duration = Duration::from_secs(30);

/// Run LLM cleanup on `raw`. Returns `(proposal_or_original, status)`.
///
/// - A validated changed result has status `Applied` and is the delivered text.
/// - On any failure (spawn error, sidecar error, panic, timeout), the raw
///   text is returned unchanged and status describes the failure mode.
/// - The sidecar handle is always put back into `state.llm_engine` unless it
///   is a zombie (see `is_zombie_error`), in which case it is dropped so the
///   next call respawns.
/// - On timeout, `kill_orphan()` is called to SIGKILL the subprocess that is
///   still owned by the blocking task.
///
/// Preconditions: caller has already checked `settings.llm_cleanup_enabled`.
/// This function does NOT check the enabled flag — callers that need to skip
/// cleanup entirely should return `Disabled` without calling this.
pub async fn run_cleanup(
    state: &AppState,
    raw: &str,
    protected_terms: &[String],
) -> (String, LlmCleanupStatus) {
    if raw.len() > MAX_CLEANUP_BYTES {
        return (
            raw.to_string(),
            LlmCleanupStatus::Unavailable {
                reason: "Transcript exceeds cleanup limit; original text preserved".into(),
            },
        );
    }
    if raw.trim().is_empty() {
        return (raw.to_string(), LlmCleanupStatus::NoChanges);
    }
    // This conservative detector only abstains on reliable non-English results.
    // Short ambiguous phrases still require model judgment and source validation.
    if whatlang::detect(raw)
        .is_some_and(|info| info.is_reliable() && info.lang() != whatlang::Lang::Eng)
    {
        return (
            raw.to_string(),
            LlmCleanupStatus::Unavailable {
                reason:
                    "Cleanup supports English; detected another language and preserved the original"
                        .into(),
            },
        );
    }

    // Setup must never keep finished dictation waiting on package downloads.
    let Ok(_operation) = state.llm_operation.try_lock() else {
        return (
            raw.to_string(),
            LlmCleanupStatus::Unavailable {
                reason: "Cleanup is busy preparing or loading; original text preserved".into(),
            },
        );
    };
    // Hold the lifecycle lock through timeout recovery, including startup.
    match tokio::time::timeout(
        LLM_CLEANUP_TIMEOUT,
        run_cleanup_inner(state, raw, protected_terms),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => {
            kill_orphan(state);
            (
                raw.to_string(),
                LlmCleanupStatus::TimedOut {
                    elapsed_ms: LLM_CLEANUP_TIMEOUT.as_millis() as u64,
                },
            )
        }
    }
}

async fn run_cleanup_inner(
    state: &AppState,
    raw: &str,
    protected_terms: &[String],
) -> (String, LlmCleanupStatus) {
    // Ensure a live sidecar handle is available.
    let mut llm = match ensure_running(state).await {
        Ok(handle) => handle,
        Err(e) => {
            return (raw.to_string(), LlmCleanupStatus::Unavailable { reason: e });
        }
    };

    // Run cleanup under an outer timeout. `spawn_blocking` gives the move
    // closure ownership of the handle, then returns it plus the result.
    let text_for_cleanup = raw.to_string();
    let started = Instant::now();
    let cleanup_result = tokio::time::timeout(
        LLM_CLEANUP_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            let r = llm.cleanup(&text_for_cleanup);
            (llm, r)
        }),
    )
    .await;

    match cleanup_result {
        Ok(Ok((llm_back, Ok(proposal)))) => {
            // SUCCESS — put the handle back.
            let mut guard = state.llm_engine.lock().await;
            *guard = Some(llm_back);
            drop(guard);
            match validate(raw, &proposal, protected_terms) {
                Ok(cleaned) if cleaned != raw => {
                    let elapsed_ms = started.elapsed().as_millis() as u64;
                    log::info!("LLM cleanup applied source deletions in {}ms", elapsed_ms);
                    (cleaned, LlmCleanupStatus::Applied { elapsed_ms })
                }
                Ok(_) => (raw.to_string(), LlmCleanupStatus::NoChanges),
                Err(reason) => {
                    log::warn!("LLM cleanup rejected: {}", reason);
                    (raw.to_string(), LlmCleanupStatus::Failed { reason })
                }
            }
        }
        Ok(Ok((llm_back, Err(e)))) => {
            // Sidecar returned an error. Check if the underlying subprocess is dead.
            if is_zombie_error(&e) {
                // The handle is useless. Drop it so the next call respawns,
                // and clear the cached PID so kill_orphan is a no-op.
                drop(llm_back);
                state.llm_pid.store(0, Ordering::SeqCst);
                state.llm_loaded.store(false, Ordering::SeqCst);
                log::warn!(
                    "LLM cleanup failed with zombie error ({}), dropping handle",
                    e
                );
            } else {
                // Normal error — handle is still alive, put it back.
                let mut guard = state.llm_engine.lock().await;
                *guard = Some(llm_back);
                log::warn!("LLM cleanup failed: {}, using raw text", e);
            }
            (raw.to_string(), LlmCleanupStatus::Failed { reason: e })
        }
        Ok(Err(panic)) => {
            // The blocking task itself panicked. The handle is lost inside
            // the panicked task. Kill any orphaned subprocess and bail.
            log::error!("LLM cleanup task panicked: {}", panic);
            kill_orphan(state);
            (
                raw.to_string(),
                LlmCleanupStatus::Failed {
                    reason: format!("panic: {}", panic),
                },
            )
        }
        Err(_timeout) => {
            // Outer timeout fired. The blocking task is still running and
            // still owns the handle. Kill the subprocess by PID so it stops
            // consuming Metal memory immediately.
            let elapsed_ms = started.elapsed().as_millis() as u64;
            log::warn!(
                "LLM cleanup timed out after {} ms, killing subprocess",
                elapsed_ms
            );
            kill_orphan(state);
            (raw.to_string(), LlmCleanupStatus::TimedOut { elapsed_ms })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bare_hesitations_in_reported_sentence_reach_the_backend() {
        let raw = "This is a test to see um if this can um remove all the um uh yeah, those things from the sentences.";
        let state = AppState::new_with_backends(
            Box::new(MockAudioCapture::sine_wave()),
            Box::new(MockAsrEngine::with_text("unused")),
            Some(Box::new(crate::test_support::MockLlmBackend::proposal(
                "This is a test to see if this can remove all those things from the sentences.",
            ))),
            Box::new(MockPasteBackend::new()),
            Settings::default(),
        );
        let (proposal, status) = run_cleanup(&state, raw, &[]).await;
        assert_eq!(
            proposal,
            "This is a test to see if this can remove all those things from the sentences."
        );
        assert!(matches!(status, LlmCleanupStatus::Applied { .. }));
    }

    #[tokio::test]
    async fn reliable_non_english_input_is_preserved_without_inference() {
        let state = AppState::new_with_backends(
            Box::new(MockAudioCapture::sine_wave()),
            Box::new(MockAsrEngine::with_text("unused")),
            Some(Box::new(crate::test_support::MockLlmBackend::failing(
                "must not run",
            ))),
            Box::new(MockPasteBackend::new()),
            Settings::default(),
        );
        let raw = "Um diese Uhrzeit fährt kein Bus mehr, deshalb nehmen wir ein Taxi.";
        let (text, status) = run_cleanup(&state, raw, &[]).await;
        assert_eq!(text, raw);
        assert!(matches!(status, LlmCleanupStatus::Unavailable { .. }));
    }

    #[tokio::test]
    async fn oversized_text_preserves_source_without_waiting_for_setup() {
        let state = AppState::new_with_backends(
            Box::new(MockAudioCapture::sine_wave()),
            Box::new(MockAsrEngine::with_text("unused")),
            None,
            Box::new(MockPasteBackend::new()),
            Settings::default(),
        );
        let _setup = state.llm_operation.lock().await;
        let raw = "é".repeat(MAX_CLEANUP_BYTES);
        let result = run_cleanup(&state, &raw, &[]).await;
        assert_eq!(result.0, raw);
        assert!(matches!(result.1, LlmCleanupStatus::Unavailable { .. }));
    }

    #[tokio::test]
    async fn active_setup_preserves_dictation_without_waiting() {
        let state = AppState::new_with_backends(
            Box::new(MockAudioCapture::sine_wave()),
            Box::new(MockAsrEngine::with_text("unused")),
            None,
            Box::new(MockPasteBackend::new()),
            Settings::default(),
        );
        let _setup = state.llm_operation.lock().await;
        let raw = "Please um, preserve the last sentence and number 859.";
        let result = tokio::time::timeout(Duration::from_millis(50), run_cleanup(&state, raw, &[]))
            .await
            .unwrap();
        assert_eq!(result.0, raw);
        assert!(matches!(result.1, LlmCleanupStatus::Unavailable { .. }));
    }

    use crate::llm::engine::LlmBackend;
    use crate::models::Settings;
    use crate::test_support::{MockAsrEngine, MockAudioCapture, MockPasteBackend};
    use std::sync::{mpsc, Arc};

    struct PausedCleanup {
        started: Option<tokio::sync::oneshot::Sender<()>>,
        resume: mpsc::Receiver<()>,
    }

    struct ClosedProtocol;

    impl LlmBackend for ClosedProtocol {
        fn cleanup(&mut self, _text: &str) -> Result<String, String> {
            Err("Sidecar protocol closed: response exceeded protocol limit".into())
        }

        fn request_raw(&mut self, _: &serde_json::Value) -> Result<serde_json::Value, String> {
            unreachable!("protocol mock only supports cleanup")
        }
    }

    #[tokio::test]
    async fn closed_protocol_is_dropped_so_next_cleanup_can_spawn() {
        let state = AppState::new_with_backends(
            Box::new(MockAudioCapture::sine_wave()),
            Box::new(MockAsrEngine::with_text("unused")),
            Some(Box::new(ClosedProtocol)),
            Box::new(MockPasteBackend::new()),
            Settings::default(),
        );
        let raw = "Please um, preserve this entire final sentence.";
        let (text, status) = run_cleanup(&state, raw, &[]).await;
        assert_eq!(text, raw);
        assert!(matches!(status, LlmCleanupStatus::Failed { .. }));
        assert!(state.llm_engine.lock().await.is_none());
        assert_eq!(state.llm_pid.load(Ordering::SeqCst), 0);
        assert!(!state.llm_loaded.load(Ordering::SeqCst));
    }

    impl LlmBackend for PausedCleanup {
        fn cleanup(&mut self, text: &str) -> Result<String, String> {
            self.started.take().unwrap().send(()).unwrap();
            self.resume.recv_timeout(Duration::from_secs(2)).unwrap();
            Ok(text.replace("um, ", ""))
        }

        fn request_raw(&mut self, _: &serde_json::Value) -> Result<serde_json::Value, String> {
            Err("No protocol requests expected during cleanup".into())
        }
    }

    #[tokio::test]
    async fn lifecycle_waits_until_cleanup_restores_the_resident_handle() {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let state = Arc::new(AppState::new_with_backends(
            Box::new(MockAudioCapture::sine_wave()),
            Box::new(MockAsrEngine::with_text("unused")),
            Some(Box::new(PausedCleanup {
                started: Some(started_tx),
                resume: resume_rx,
            })),
            Box::new(MockPasteBackend::new()),
            Settings::default(),
        ));
        let recording_state = state.clone();
        let cleanup = tokio::spawn(async move {
            run_cleanup(
                &recording_state,
                "Please um, keep the entire final instruction.",
                &[],
            )
            .await
        });
        started_rx.await.unwrap();
        // A resident handle is temporarily absent during inference. Lifecycle
        // code must wait instead of installing/replacing a different child.
        assert!(state.llm_engine.lock().await.is_none());
        let lifecycle = state.llm_operation.lock();
        tokio::pin!(lifecycle);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut lifecycle)
                .await
                .is_err()
        );
        resume_tx.send(()).unwrap();
        let (text, status) = cleanup.await.unwrap();
        assert_eq!(text, "Please keep the entire final instruction.");
        assert!(matches!(status, LlmCleanupStatus::Applied { .. }));
        let _operation = lifecycle.await;
        assert!(state.llm_engine.lock().await.is_some());
    }
}
