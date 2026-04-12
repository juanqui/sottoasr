# LLM Cleanup Reliability Fix — Implementation Notes

- **Date:** 2026-04-12
- **Spec:** [2026-04-11-llm-cleanup-reliability.md](../specs/2026-04-11-llm-cleanup-reliability.md)
- **Status:** Implemented (training data + retrain still pending)

## Summary

Implemented all Rust + UI tasks from the LLM cleanup reliability spec. The
training-data side (4K paragraph_formatting samples + HF upload) was kicked off
in parallel against AWS Bedrock Haiku 4.5 and is running in a background
worker. Retraining is out of scope and will happen after the dataset uploads.

## What was implemented

### Status surface — `LlmCleanupStatus`
- New `LlmCleanupStatus` enum in `src-tauri/src/models.rs` with variants
  `Applied { elapsed_ms } | SkippedTooShort | Disabled | Unavailable { reason }
  | Failed { reason } | TimedOut { elapsed_ms } | Idle`. Externally tagged
  serde so the frontend can `switch (status.kind)`.
- `Transcription` got a `llm_cleanup_status` field with `#[serde(default)]`,
  preserving backward compat with older `transcriptions.json`.
- `LlmStatus` (the settings panel struct) got a `last_cleanup_status` field
  populated from `state.llm_last_status`.
- TypeScript types added in `src/lib/utils/tauri.ts`.

### Sidecar lifecycle — `ensure_running` / `kill_orphan` / zombie detection
- `LlmEngine` now stores its child PID, exposed via `child_pid()`.
- `AppState` got `llm_pid: AtomicI32` and `llm_last_status: TokioMutex<LlmCleanupStatus>`.
- New helpers in `src-tauri/src/llm/engine.rs`:
  - `ensure_running(state)` — fast-path returns the cached handle, slow-path
    spawns + loads the model with one retry on persistent failure.
  - `kill_orphan(state)` — SIGKILLs the cached PID via `libc::kill` and
    clears the cache. macOS-only, no-op elsewhere.
  - `is_zombie_error(err)` — heuristic for "broken pipe / closed stdout"
    failure modes that mean the subprocess has died and the handle should be
    dropped instead of put back.
- Added `libc = "0.2"` to `src-tauri/Cargo.toml` (was transitively available
  but now explicit).

### Shared cleanup helper — `llm::cleanup::run_cleanup`
- New `src-tauri/src/llm/cleanup.rs` module owns the canonical cleanup flow:
  short-input skip → ensure_running → cleanup with timeout → put-back-or-drop
  → return `(text, status)`. Both the production hotkey path
  (`hotkeys/manager.rs`) and the testable pipeline (`pipeline.rs`) call this
  one function. Eliminates the duplicate inline cleanup blocks.
- `LLM_CLEANUP_TIMEOUT` raised from 120 s to 300 s, matching the long-input
  capacity unlocked in §4.2 of the spec.
- Zombie-handle protection: when the cleanup result is an error matching
  `is_zombie_error`, the handle is dropped and `llm_pid` cleared so the next
  call respawns instead of looping on a dead subprocess.

### Long-context unlock
- `src-tauri/sidecar/llm_cleanup.py:88` formula changed from
  `max(256, int(input_words * 1.5))` to
  `min(16384, max(4096, int(input_words * 2.5)))`. Math is documented inline.
- `MAX_RECORDING_SECS` raised from 12 min to 20 min in
  `src-tauri/src/hotkeys/manager.rs` and `src-tauri/src/pipeline.rs`.
  `MAX_AUDIO_BUFFER_SAMPLES` follows. The frontend countdown banner constant
  in `src/lib/components/overlay-pill.svelte` was bumped to match.

### UI status badge
- `overlay-pill.svelte` listens for `llm-cleanup-status` events and replaces
  the `Cleaning up...` spinner label with a brief badge:
  - **Cleaned** (green ✓) for `Applied`, dwell 800 ms.
  - **Cleanup unavailable / Cleanup failed / Cleanup timed out** (orange ⚠)
    for the failure modes, dwell 2000 ms.
  - No badge for `SkippedTooShort` / `Disabled` / `Idle`.
- Rust holds the overlay open via `tokio::time::sleep(badge_dwell_ms)` after
  paste so the badge has time to register. Paste happens before the sleep, so
  user-visible text appears at the cursor with no added latency — only the
  hide animation is delayed.
- `history-item.svelte` shows a "Raw transcript" warning badge with a
  tooltip describing the failure (e.g., "Cleanup unavailable: spawn task panic")
  for past entries that hit `Failed` / `Unavailable` / `TimedOut`.

### Training data
- `training/scripts/generate_synthetic.py` got a new `paragraph_formatting`
  CATEGORY (weight 0.01 — only triggered via `--category paragraph_formatting`),
  matching `_get_category_instructions()` block, single-object batch handling
  (mirrored from `long_transcript`), and validation requiring `\n\n` and 2–6
  paragraphs. Implemented by a background agent.
- Bedrock token rotated in `.env` after the original returned 403 on every
  endpoint. New token verified against `global.anthropic.claude-haiku-4-5-20251001-v1:0`.
- 4,000-sample generation + HuggingFace dataset upload running in the
  background as of this entry's timestamp.

### Benchmark coverage
- Added 12 hand-crafted `paragraph_formatting` rows to
  `benchmarks/llm/generate_dataset.py` (`para_01..para_12`). Outputs are
  117–183 words across 4–7 paragraphs split on natural discourse boundaries
  (topic shifts, time refs, enumerated points, theme switches).
- Recovered 25 rows that had been hand-edited directly into `dataset.csv` but
  were missing from the generator script (10 `dictation_commands`,
  12 `preserve_wording`, 3 extra `long_dictation`). Script is now the single
  source of truth at 147 rows / 13 categories.

## Verification

```text
cargo clippy --all-targets -- -D warnings    # clean
cargo test                                   # 78 passed
npx vitest run                               # 90 passed
npm run check                                # 0 errors (10 pre-existing warnings)
npm run build                                # built in 242ms
```

The 78 Rust tests include:
- 4 new `LlmCleanupStatus` serde tests in `models::tests`.
- 4 new `is_zombie_error` tests in `llm::engine::tests`.
- 1 new pipeline integration test
  (`test_short_input_emits_skipped_too_short`) verifying the new status flow.
- All existing pipeline tests updated to assert the new status field.

## Deviations from the spec

- **Task 5 numbering:** the spec listed 17 tasks. Tasks 11–13 are the
  training-data work (paragraph category in `generate_synthetic.py`, 4K
  Bedrock run, HF upload) — I delegated those to a background agent rather
  than blocking on them sequentially. That agent had a 403 on the original
  Bedrock token; the user provided a fresh token mid-session and the agent
  resumed.
- **Task 15 (rerun the 3× benchmark):** deferred to after retraining. Running
  the benchmark now would only re-confirm the existing baseline.
- **Spec didn't call out a version bump:** none was performed. The
  implementation lands on `0.6.3` and a release will bump per the normal
  release-prep flow.

## Things to test manually before shipping

1. **Long recording:** dictate continuously for ≥15 min. Confirm the warning
   countdown appears at the 19-min mark (60 s before the new 20-min cap),
   recording auto-stops at 20 min, and cleanup completes without truncation.
2. **Cleanup timeout:** temporarily add a `time.sleep(400)` to
   `cleanup_chunk()` in `llm_cleanup.py`, dictate a phrase, watch the overlay
   show **Cleanup timed out** for 2 s and `ps -ef | grep llm_cleanup` confirm
   the subprocess is killed.
3. **Spawn failure:** rename the venv python binary, dictate a phrase, watch
   the overlay show **Cleanup unavailable** with the error message visible
   in the next history entry's tooltip.
4. **Short input:** dictate "ship it". Confirm the overlay hides immediately
   (no badge) and the history entry has no `Raw transcript` warning.
5. **Status field round-trip:** dictate a normal phrase, open the history
   window, confirm the new entry shows `AI Cleaned`. Open
   `~/Library/Application Support/com.sottoasr.app/transcriptions.json` and
   verify the entry has a `llm_cleanup_status: { kind: "applied", detail: { elapsed_ms: ... } }` field.
