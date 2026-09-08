# Cleanup recovery, model readiness, and History

- **Version:** 1.0
- **Date:** 2026-09-08
- **Status:** Implemented

## Contents
1. Summary
2. Problem statement
3. Design overview
4. Detailed design
5. Edge cases
6. File changes
7. Testing strategy
8. Migration plan
9. Security considerations
10. Cost analysis
11. Implementation tasks
12. Implementation status

## 1. Summary
Distinguish model failures from rejected proposals, recover unhealthy cleanup processes, retain useful diagnostics, show live ASR/cleanup readiness, and make narration history easier to search and inspect.

## 2. Problem statement
The locally reported 84.736-second narration completed inference, then failed source validation at 23:20:48 UTC. Replaying it with the installed sidecar reproduces rejection: the proposal inserts a word and removes other meaningful words alongside valid filler deletions. Short probes immediately afterward still succeed. This does not yet reproduce the separate work-computer report of permanent failures after several requests.

The installed code maps unexpected Python exceptions to a generic `operation_failed` response and keeps that resident process alive. A persistent native/runtime error can consequently recur indefinitely. Logs use the plugin defaults: a 40,000-byte file and `KeepOne`, discarding prior diagnostics on rotation. Settings reads cached loaded flags; History groups rejection and runtime failures under a tooltip labeled skipped.

Evidence: local log and History inspected read-only; private replay outputs are under `/tmp/experiments/sotto-resident-cleanup/`. No private narration is included in this repository.

## 3. Design overview
Keep the pinned model, prompt, runtime, local storage, and original-text preservation. Validate completed proposals conservatively; retain independently valid filler/repeat deletions when unrelated edits fail validation. Restart unhealthy processes on the next request without repeated inference delays on the current recording.

Display two compact model cards above Settings tabs with textual status and green only for loaded readiness. Show preparation, disabled, unavailable, and failure states distinctly. History gets filters, date grouping, readable outcome details, and explicit original/transcript copy actions.

## 4. Detailed design
### Cleanup and recovery
- Preserve the frozen full-proposal validator and its parity tests.
- Add a bounded fallback for completed rejected proposals: align exact words with LCS, isolate deletion runs, reconstruct candidates from original source, and pass each candidate through the full existing validator with the complete protected context. Do not adopt generated wording or punctuation. Ambiguous/repeated alignment must not authorize unguarded deletions. Validate the combined result again. Reject if no independently valid edit survives.
- Distinguish `Rejected` from execution `Failed`, retaining backward compatibility for old history entries.
- Unexpected runtime errors, timeouts, malformed responses, and missing/wrong model identity retire the sidecar. A later request loads and warms a fresh process. Ordinary validation rejection never unloads a healthy model.
- Log one transcript-free completion summary per request, including request ID, input bytes, duration, outcome and controlled reason. Python logs exception type and bounded stack locations, without exception values, locals or source lines.
- Retain five 2 MB log archives plus the active file; move recurring audio-level messages to debug.

### Settings readiness
- Status snapshots report ASR initializing/error and cleanup enabled/busy/live readiness. Determine cleanup liveness from owned child handles, never arbitrary PID signaling or inference in a status query.
- Refresh on lifecycle and transcription events and every five seconds only while Settings is visible. Coalesce in-flight refreshes and remove listeners/timers on teardown. Unknown/error reads must never display stale green readiness.
- Both cards show the model name, a status label and concise explanation. Cleanup last outcome is separate from model readiness. Recovery uses existing preparation controls; ASR retry uses existing initialization.

### History
- Keep bounded pagination and full-history search, including originals.
- Add All / AI cleaned / Cleanup issues outcome filters and local date grouping. Display result counts and clear-filter action.
- Expanded entries have selectable text, visible cleanup outcome/reason, full date/time, explicit transcript/original/difference views and copy actions. Preserve confirmed deletion and export behavior, legacy suggestions and raw text.

## 5. Edge cases
Do not retry automatically forever, accept truncated generation, remove meaningful protected content, or claim a rejection means the model is unloaded. Preserve disabled behavior, cancellation, settings drafts, download progress, old history schemas, late events, failed reads, and disposal. Avoid polling hidden windows or blocking status on inference.

## 6. File changes
`llm/{cleanup,engine,validation}.rs`, sidecar and protocol tests, model/state/status commands, app logging/startup, audio logging, Settings readiness component/store, History components and tests, frontend IPC types, and this specification/evidence journal. No model artifacts, keys, download paths, release versions or user data changes.

## 7. Testing strategy
Replay the reported narration and alternating short probes in a single resident installed process. Inject a persistent runtime exception after several successful requests to prove retirement/recovery. Exercise protected words, facts, literal mentions, Unicode, additions, reorderings, ambiguous repeats and partial proposals against the fallback; retain 672 frozen validator parity cases. Test status lifecycle/stale responses and History filters/copy/navigation. Run Rust build/tests/strict Clippy, Python tests, frontend tests/type check/build, and inspect browser-rendered Settings/History using synthetic data.

## 8. Migration plan
Additive status fields and a new serialized cleanup outcome; old entries remain readable. No data rewrite, cache deletion or implicit model change. Work-computer diagnosis remains provisional without its logs.

## 9. Security considerations
Processing and diagnostics stay local. No transcript text, hashes, exception values or model output in routine logs. Replays using the user's narration remain private outside git. Full original transcriptions remain in existing History.

## 10. Cost analysis
LCS fallback is bounded by the existing 1,024-word/32 KB limits. Reuse existing validation and inference; add no model call. Status uses cheap local snapshots. Log disk use is bounded to roughly 12 MB.

## 11. Implementation tasks
- [x] Inspect local failed narration and map failure/status/logging paths.
- [x] Complete resident replay and fault reproduction; record evidence.
- [x] Review assumptions, completeness, and actionability sequentially.
- [x] Implement guarded partial cleanup, failure retirement and diagnostics with tests.
- [x] Implement truthful live model dashboard with tests.
- [x] Improve History navigation and explanations with tests.
- [x] Review implementation, verify all checks and rendered UI, document limits.

## 12. Implementation status
Implemented and verified. See [investigation evidence](../journals/2026-09-08-cleanup-resident-failure-investigation.md) for replay results, controlled fault testing, reviews, validation and explicit limits. Added roomier default window sizes after reviewing the old dimensions. No model/prompt changes or additional inference calls. Local installation and release were deferred until the subsequent user-authorized v0.8.4 release.

References: [Tauri logging builder](https://docs.rs/tauri-plugin-log/latest/tauri_plugin_log/struct.Builder.html), [rotation strategies](https://docs.rs/tauri-plugin-log/latest/tauri_plugin_log/enum.RotationStrategy.html); verified against installed dependency source. MLX runtime behavior is evaluated through the pinned installed code and controlled fault tests rather than assumed from unrelated reports.

### Review 1 — assumption validation
The replay separates content rejection from process health: successful short requests follow repeated reported-text rejections in the same process. The permanent-workstation explanation is a reproducible code-path hypothesis, not an observed workstation diagnosis. The fallback is source reconstruction, not acceptance of the model's rewrite: replacements/additions are excluded, and each deletion plus the combined candidate must pass the original validator. The status dashboard must not equate model files on disk with readiness.

### Review 2 — completeness and safety
The fallback runs only after complete successful generation, never after a protocol error or deadline. Token alignment is bounded; replacement runs retain their entire source. Existing full-context protections and final combined validation remain authoritative. Add regression tests where literal filler words, negations, numbers and repeated content sit next to valid deletions. Status reads use the owned-process registry so an exited child cannot remain green; disabled configuration is reported separately from a temporarily resident model. Snapshot errors clear stale readiness. Retain old `failed` entries and infer legacy validation rejection only for known validator reason strings.

### Review 3 — clarity and implementation order
Implement recovery/diagnostics first with controlled failing-backend tests. Next add `validate_cleanup` as a wrapper around unchanged `validate`, returning reconstructed source or a rejection; independently test bounded LCS deletion-only runs and final composition. Then wire persisted rejection status and UI presentation, followed by live readiness snapshots and History navigation. No model/prompt revision change or additional inference call is needed. Verification must distinguish controlled recovery tests from the unconfirmed work-computer root cause. All three review passes are complete; implementation may begin.
