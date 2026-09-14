//! Shared LLM cleanup helper used by both the production hotkey pipeline
//! (`hotkeys/manager.rs`) and the unit-testable pipeline (`pipeline.rs`).
//!
//! This module owns the single authoritative flow for running transcript
//! cleanup: ensure the sidecar is running, dispatch bounded head-batched
//! requests under per-request deadlines, kill orphaned subprocesses on
//! failure, and report a structured `LlmCleanupStatus`. See
//! docs/specs/2026-04-11-llm-cleanup-reliability.md §4.1 for the reliability
//! rationale and docs/specs/2026-09-13-batched-stop-path-correction.md for
//! the batch wire contract.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::llm::correction;
use crate::llm::engine::{
    BatchItem, ensure_running, is_responded_timeout, is_zombie_error, kill_orphan,
};
use crate::models::LlmCleanupStatus;
use crate::state::AppState;

/// Failure-recovery deadline for a blocked sidecar. Normal local cleanup is
/// much faster; input and output sizes are bounded separately. A timeout always
/// preserves the original transcript and terminates the orphaned process.
pub const LLM_CLEANUP_TIMEOUT: Duration = Duration::from_secs(30);

/// Run LLM cleanup on `raw` through the batched stop-path planner
/// (`correction::plan_cleanup`).
///
/// - A validated changed result has status `Applied` and is the delivered text.
/// - Planning is per window: a failed, rejected or timed-out window leaves
///   only its own region raw while independently validated edits elsewhere
///   still apply; only a wholesale failure (no sidecar at all) returns the raw
///   text unchanged with status describing the failure mode.
/// - The sidecar handle is always put back into `state.llm_engine` unless it
///   is a zombie (see `is_zombie_error`), in which case it is dropped so the
///   next call respawns.
/// - Per-request timeouts and orphan recovery live inside
///   `sidecar_cleanup_batch`; there is deliberately NO whole-transcript size
///   refusal and NO whole-cleanup outer timeout: window texts are bounded
///   and chunks are count-16, so one slow or broken request can only cost
///   its own chunk.
///
/// Preconditions: caller has already checked `settings.llm_cleanup_enabled`.
/// This function does NOT check the enabled flag — callers that need to skip
/// cleanup entirely should return `Disabled` without calling this.
pub async fn run_cleanup(
    state: &AppState,
    raw: &str,
    protected_terms: &[String],
) -> (String, LlmCleanupStatus) {
    let started = Instant::now();
    let request_id = crate::llm::engine::next_job_id();
    let result = run_cleanup_attempt(state, raw, protected_terms).await;
    // Status contains controlled reasons, never model output or transcript text.
    log::info!("Cleanup request={} input_bytes={} elapsed_ms={} outcome={:?}",
        request_id, raw.len(), started.elapsed().as_millis(), result.1);
    result
}

async fn run_cleanup_attempt(
    state: &AppState,
    raw: &str,
    protected_terms: &[String],
) -> (String, LlmCleanupStatus) {
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
    correction::plan_cleanup(state, raw, protected_terms).await
}

/// Run ONE bounded batched sidecar request: the full spawn/reuse, startup,
/// timeout, zombie, and handle-restore discipline inside a single outer
/// deadline that COVERS `ensure_running` (two load attempts + backoff can
/// otherwise hang unbounded), the request, and handle restoration — the same
/// bound the pre-incremental whole-text path enforced. On expiry the
/// orphaned subprocess is SIGKILLed. There is deliberately NO whole-transcript
/// timeout: the planner dispatches ≤16-window chunks, each bounded like this.
/// The caller must hold `llm_operation` for the entire call. Returns one
/// typed item per input text, or the wholesale failure status; the caller
/// owns authority derivation and text preservation.
pub(crate) async fn sidecar_cleanup_batch(
    state: &AppState,
    texts: Vec<String>,
) -> Result<Vec<BatchItem>, LlmCleanupStatus> {
    match tokio::time::timeout(LLM_CLEANUP_TIMEOUT, sidecar_batch_request(state, texts)).await {
        Ok(result) => result,
        Err(_) => {
            // Old semantics preserved: the whole attempt (startup included)
            // gets the outer deadline, then the orphan is killed. A request
            // cancelled mid-`ensure_running` may leave a child the kill
            // sweep catches by cached PID, exactly as before.
            log::warn!(
                "LLM cleanup attempt exceeded {} ms (startup included), killing subprocess",
                LLM_CLEANUP_TIMEOUT.as_millis()
            );
            kill_orphan(state);
            Err(LlmCleanupStatus::TimedOut {
                elapsed_ms: LLM_CLEANUP_TIMEOUT.as_millis() as u64,
            })
        }
    }
}

async fn sidecar_batch_request(
    state: &AppState,
    texts: Vec<String>,
) -> Result<Vec<BatchItem>, LlmCleanupStatus> {
    // Ensure a live sidecar handle is available.
    let mut llm = match ensure_running(state).await {
        Ok(handle) => handle,
        Err(e) => return Err(LlmCleanupStatus::Unavailable { reason: e }),
    };

    // Run the batch under an outer timeout. `spawn_blocking` gives the move
    // closure ownership of the handle, then returns it plus the result. The
    // chunk's owned copy is materialized ONCE here — the genuine IPC
    // boundary (spec §4.3 ownership taste), never a per-window re-clone.
    // Move the chunk once into the blocking pool; the request
    // builder borrows it from here on (no second copy).
    let count = texts.len();
    let started = Instant::now();
    let cleanup_result = tokio::time::timeout(
        LLM_CLEANUP_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            let r = llm.cleanup_batch(&texts);
            (llm, r)
        }),
    )
    .await;
    match cleanup_result {
        Ok(Ok((llm_back, Ok(items)))) => {
            // Completed batch — put the handle back; the caller derives
            // authority per item. A valid response with per-item typed
            // failures RETAINS the handle (spec §4.1).
            let mut guard = state.llm_engine.lock().await;
            *guard = Some(llm_back);
            drop(guard);
            let elapsed_ms = started.elapsed().as_millis() as u64;
            log::info!("LLM cleanup batch of {count} responded in {elapsed_ms}ms");
            Ok(items)
        }
        Ok(Ok((llm_back, Err(e)))) => {
            // The request errored as a whole. Classify the endpoint.
            if is_responded_timeout(&e) {
                // The sidecar enforced its own generation deadline and reported
                // it cleanly; on the current build/machine that class was
                // observed to leave a healthy process (docs/journals/
                // 2026-09-12-correction-baseline-experiments.md §4, with its
                // caveats). Keep the resident handle so the next chunk (or
                // recording) does not pay spawn+load+warm in its stop path.
                let mut guard = state.llm_engine.lock().await;
                *guard = Some(llm_back);
                log::warn!("LLM cleanup hit the generation deadline, keeping the resident process");
                Err(LlmCleanupStatus::TimedOut {
                    elapsed_ms: started.elapsed().as_millis() as u64,
                })
            } else if is_zombie_error(&e) {
                // The handle is useless (dead pipe, or an untrustworthy
                // response the engine already terminated + unregistered —
                // spec §4.1 retire path). Drop it so the next chunk respawns,
                // and clear the cached PID so kill_orphan is a no-op.
                drop(llm_back);
                state.llm_pid.store(0, Ordering::SeqCst);
                state.llm_loaded.store(false, Ordering::SeqCst);
                log::warn!("LLM cleanup retired the sidecar: {e}");
                Err(LlmCleanupStatus::Failed { reason: e })
            } else {
                // Typed endpoint decline (e.g. invalid_request, E7): the
                // process answered validly — the handle is still alive, put
                // it back.
                let mut guard = state.llm_engine.lock().await;
                *guard = Some(llm_back);
                log::warn!("LLM cleanup failed: {}", e);
                Err(LlmCleanupStatus::Failed { reason: e })
            }
        }
        Ok(Err(panic)) => {
            // The blocking task itself panicked. The handle is lost inside
            // the panicked task. Kill any orphaned subprocess and bail.
            log::error!("LLM cleanup task panicked: {}", panic);
            kill_orphan(state);
            Err(LlmCleanupStatus::Failed {
                reason: format!("panic: {}", panic),
            })
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
            Err(LlmCleanupStatus::TimedOut { elapsed_ms })
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bare_hesitations_in_reported_sentence_reach_the_backend() {
        // The planner sends WINDOW KEYS, never the whole transcript, so the
        // mock must behave per-text: a window-local stripper stands in for
        // the sidecar. What the pipeline observes is the batch delivery: the
        // fillers gone, the retained words byte-exact.
        // >20 words ⇒ the planner dispatches MULTIPLE overlapping windows;
        // every filler is a clean interior token so the window-local reply
        // is validator-derivable over each key's own bytes.
        let raw = "This is a test to see um if this can um remove all the fillers from the final spoken sentences today which had um hesitation inside them.";
        let backend = crate::test_support::MockLlmBackend::run(|text| {
            Ok(text.replace(" um ", " "))
        });
        let state = AppState::new_with_backends(
            Box::new(MockAudioCapture::sine_wave()),
            Box::new(MockAsrEngine::with_text("unused")),
            Some(Box::new(backend)),
            Box::new(MockPasteBackend::new()),
            Settings::default(),
        );
        let (proposal, status) = run_cleanup(&state, raw, &[]).await;
        assert!(matches!(status, LlmCleanupStatus::Applied { .. }), "{status:?}");
        assert!(!proposal.contains(" um "), "fillers must reach and pass the backend: {proposal:?}");
        assert!(proposal.contains("This is a test to see"), "retained copy survives: {proposal:?}");
        assert!(
            proposal.contains("hesitation inside them"),
            "tail survives: {proposal:?}"
        );
    }

    /// Regression (2026-09-12): a long dictation hit the sidecar's 10 s
    /// generation alarm; the old mapping killed the process and made the next
    /// recording pay spawn+load+warm. A responded deadline must report
    /// TimedOut AND keep the resident handle.
    #[tokio::test]
    async fn responded_deadline_reports_timeout_and_keeps_the_handle() {
        let state = AppState::new_with_backends(
            Box::new(MockAudioCapture::sine_wave()),
            Box::new(MockAsrEngine::with_text("unused")),
            Some(Box::new(crate::test_support::MockLlmBackend::failing(
                crate::llm::engine::RESPONDED_TIMEOUT_ERROR,
            ))),
            Box::new(MockPasteBackend::new()),
            Settings::default(),
        );
        let raw = "Please um, keep the entire final instruction intact.";
        let (text, status) = run_cleanup(&state, raw, &[]).await;
        assert_eq!(text, raw);
        assert!(matches!(status, LlmCleanupStatus::TimedOut { .. }));
        assert!(
            state.llm_engine.lock().await.is_some(),
            "a responded deadline must not retire the healthy sidecar"
        );
        assert!(state.llm_loaded.load(Ordering::SeqCst));
    }

    /// A zombie (pipe-dead) error still retires the handle: the two timeout
    /// classes must not blur.
    #[tokio::test]
    async fn runtime_restart_error_still_retires_the_handle() {
        let state = AppState::new_with_backends(
            Box::new(MockAudioCapture::sine_wave()),
            Box::new(MockAsrEngine::with_text("unused")),
            Some(Box::new(crate::test_support::MockLlmBackend::failing(
                "Cleanup runtime requires restart (operation_failed); original text preserved",
            ))),
            Box::new(MockPasteBackend::new()),
            Settings::default(),
        );
        let raw = "Please um, keep the entire final instruction intact.";
        let (text, status) = run_cleanup(&state, raw, &[]).await;
        assert_eq!(text, raw);
        assert!(matches!(status, LlmCleanupStatus::Failed { .. }));
        assert!(state.llm_engine.lock().await.is_none());
        assert!(!state.llm_loaded.load(Ordering::SeqCst));
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
    async fn huge_text_with_busy_setup_preserves_source_without_waiting() {
        // The bounded planner removed the whole-text 32KB refusal: size is
        // never a reason to skip cleanup. What still must hold for a text far
        // past the old limit is that a BUSY engine setup (operation lock
        // held, no engine) preserves the source immediately — the planner's
        // try_lock gate runs before any tokenization of the huge input.
        use crate::llm::validation::MAX_CLEANUP_BYTES;
        let state = AppState::new_with_backends(
            Box::new(MockAudioCapture::sine_wave()),
            Box::new(MockAsrEngine::with_text("unused")),
            None,
            Box::new(MockPasteBackend::new()),
            Settings::default(),
        );
        let _setup = state.llm_operation.lock().await;
        let raw = "é".repeat(MAX_CLEANUP_BYTES * 2);
        let result = tokio::time::timeout(Duration::from_millis(500), run_cleanup(&state, &raw, &[]))
            .await
            .expect("busy-setup gate must answer immediately, size notwithstanding");
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
        fn cleanup_batch(
            &mut self,
            _texts: &[String],
        ) -> Result<Vec<crate::llm::engine::BatchItem>, String> {
            Err("Sidecar protocol closed: response exceeded protocol limit".into())
        }

        fn request_raw(&mut self, _: &serde_json::Value) -> Result<serde_json::Value, String> {
            unreachable!("protocol mock only supports cleanup_batch")
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
        fn cleanup_batch(
            &mut self,
            texts: &[String],
        ) -> Result<Vec<crate::llm::engine::BatchItem>, String> {
            self.started.take().unwrap().send(()).unwrap();
            self.resume.recv_timeout(Duration::from_secs(2)).unwrap();
            Ok(texts
                .iter()
                .map(|t| crate::llm::engine::BatchItem::Proposal(Some(t.replace("um, ", ""))))
                .collect())
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
