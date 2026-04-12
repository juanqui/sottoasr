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
use crate::models::LlmCleanupStatus;
use crate::state::AppState;

/// Minimum word count required before we run the LLM cleanup. Inputs under
/// this threshold are returned unchanged with status `SkippedTooShort`.
pub const MIN_CLEANUP_WORDS: usize = 5;

/// Outer timeout for a single cleanup call. Raised from the prior 120 s so
/// long dictations have enough headroom; a 15-minute recording at the measured
/// 243 tok/s throughput finishes in ~15 s, so 300 s gives 20× safety margin
/// for cold starts, Metal cache cascades, and adversarial inputs.
/// See docs/specs/2026-04-11-llm-cleanup-reliability.md §4.2.
pub const LLM_CLEANUP_TIMEOUT: Duration = Duration::from_secs(300);

/// Run LLM cleanup on `raw`. Returns `(text_to_paste, status)`.
///
/// - On success, `text_to_paste` is the cleaned text and status is `Applied`.
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
pub async fn run_cleanup(state: &AppState, raw: &str) -> (String, LlmCleanupStatus) {
    // Skip short inputs — the sidecar itself also skips these, but we want
    // to report the skip as a structured status so the UI can show a badge.
    if raw.split_whitespace().count() < MIN_CLEANUP_WORDS {
        return (raw.to_string(), LlmCleanupStatus::SkippedTooShort);
    }

    // Ensure a live sidecar handle is available.
    let mut llm = match ensure_running(state).await {
        Ok(handle) => handle,
        Err(e) => {
            return (
                raw.to_string(),
                LlmCleanupStatus::Unavailable { reason: e },
            );
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
        Ok(Ok((llm_back, Ok(cleaned)))) => {
            // SUCCESS — put the handle back.
            let mut guard = state.llm_engine.lock().await;
            *guard = Some(llm_back);
            let elapsed_ms = started.elapsed().as_millis() as u64;
            log::info!(
                "LLM cleanup: {} -> {} chars in {}ms",
                raw.len(),
                cleaned.len(),
                elapsed_ms
            );
            (cleaned, LlmCleanupStatus::Applied { elapsed_ms })
        }
        Ok(Ok((llm_back, Err(e)))) => {
            // Sidecar returned an error. Check if the underlying subprocess is dead.
            if is_zombie_error(&e) {
                // The handle is useless. Drop it so the next call respawns,
                // and clear the cached PID so kill_orphan is a no-op.
                drop(llm_back);
                state.llm_pid.store(0, Ordering::SeqCst);
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
            (
                raw.to_string(),
                LlmCleanupStatus::TimedOut { elapsed_ms },
            )
        }
    }
}
