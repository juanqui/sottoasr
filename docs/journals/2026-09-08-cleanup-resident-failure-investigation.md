# Resident cleanup failure investigation

- **Version:** 1.0
- **Date:** 2026-09-08
- **Status:** Implemented

## Contents
1. Findings
2. Reproduction and measurements
3. Changes and review
4. Verification and limits

## 1. Findings
The reported local narration lasted 84.736 seconds. Its 23:20:48 UTC log entry records a validation rejection after successful inference, not a dead model. The proposal mixed useful filler removal with an inserted word and removal of meaningful words. The original was preserved, but the UI called this a generic cleanup failure.

A separate lifecycle defect is confirmed by source inspection and fault injection: `operation_failed` did not count as a dead/unhealthy sidecar, so the application retained the process after unexpected native/runtime exceptions. A fault that left that process unhealthy could recur on every subsequent recording. This is a plausible explanation for the work-computer report, not proof of its exact trigger.

The configured logger used defaults of 40,000 bytes and `KeepOne`. Frequent audio-level messages consumed that small budget, and rotation discarded earlier diagnostics. These defaults were checked in the installed crate and the [official logging builder reference](https://docs.rs/tauri-plugin-log/latest/tauri_plugin_log/struct.Builder.html).

## 2. Reproduction and measurements
On the local M4/32 GB Mac, used the installed 0.8.3 bundled sidecar and existing managed Python runtime/cache. Sent four short probes, then alternated the user's reported narration and the probe for twelve more requests in the same resident process. The actual user's narration and model proposals remain outside git under `/tmp/experiments/sotto-resident-cleanup/`.

| Case | Requests | Original validator | Request latency, min / median / max |
|---|---:|---|---|
| Public short filler probe | 10 | 10 accepted | 0.985 / 1.0235 / 1.108 seconds |
| Reported narration | 6 | 6 rejected | 5.568 / 5.692 / 5.784 seconds |

All sixteen inference requests completed. Short probes continued succeeding immediately after long-text rejections. This did not reproduce a naturally occurring permanent MLX fault on this Mac.

Revalidated the same sixteen completed proposals through the changed production Rust adapter: all sixteen now yield validated cleanup. In the reported narration, the fallback retains only allowed source deletions and preserves meaningful words and original punctuation that the generated proposal attempted to change. One sampled validation, including starting the adapter process, took 9.07 ms. It adds no model call; these timings are not a new model-quality qualification or an energy benchmark.

A production `LlmEngine` JSON-pipe test supplies three successful responses, then a persistent `operation_failed` response from a real subprocess. The fixed path preserves the recording, terminates/reaps that process, clears readiness and removes its handle. A fresh test subprocess then handles the next request successfully. The real-model startup/load path is separately exercised by the resident replay; the injected recovery test does not claim to induce a real Metal fault.

## 3. Changes and review
- Preserve the frozen full validator. For completed rejected proposals, bounded word alignment identifies deletion-only runs; each candidate is reconstructed from source and checked with all original context/protections. The combined candidate passes the same validator again. Generated replacements, additions and punctuation are never copied by the fallback.
- Retire unhealthy cleanup processes on unexpected runtime errors; recognize already-exited resident processes before reuse. Validation rejection keeps a healthy process warm.
- Log a transcript-free request summary and bounded exception code locations without exception values/source/locals. Keep five 2 MB archives plus the active log; move periodic audio levels to debug.
- Add `rejected` as a distinct persisted outcome. Display legacy known validator failures as rejected edits without rewriting history.
- Display ASR and cleanup readiness independently of download state and last cleanup outcome. ASR initialization/error state covers startup and onboarding; cleanup readiness checks owned-child liveness. Status refreshes are coalesced, limited to visible Settings, and refreshed on events/focus. A preparation acknowledgement invalidates older polling replies.
- Add History cleanup filters, local date groups, full expanded timestamps, visible failure reasons, selectable text, deferred difference calculation and separate original-copy feedback. Preserve pagination, search, export and confirmed deletion.
- Visual review at the old 520×600 Settings size showed too little room for controls below the dashboard. New default Settings is 640×760; History is 680×760. Both remain resizable. Reviewed production frontend builds with synthetic IPC data in Chrome.

Three sequential specification reviews preceded implementation. Implementation review additionally caught and fixed a preparation/poll ordering race, stale status after a read failure, missing original-copy feedback, and misleading save-after-preparation messaging.

## 4. Verification and limits
- 185 Rust tests pass, including 672-case frozen-validator parity, partial-result policy checks across that corpus, pipeline preservation, owned-process liveness and injected runtime-fault recovery.
- 152 frontend tests pass, including filters, explicit copy provenance, readiness distinctions, stale-response handling and listener cleanup.
- 20 Python tests pass, including transcript-free exception diagnostics.
- Strict Clippy, Rust build, Svelte/TypeScript checks and Vite production build pass. Final checks are captured in `/tmp/sotto-recovery-*` logs.
- Chrome review verified model cards, navigation from dashboard to Dictation, History issue filtering, expanded diagnostics and no History console errors. This uses the real production UI with synthetic IPC responses; it is not a native installed-app smoke test.

The work computer's exact failure mechanism remains unconfirmed without its retained diagnostics. The model can still make poor proposals; syntactic source preservation cannot establish semantic correctness. This change improves safe partial cleanup and runtime recovery, rather than claiming all model errors are eliminated.

No user settings, history, model files or runtime packages were changed. The investigation was verified before release. The subsequent user-authorized release rolls these changes into v0.8.4, with direct publication from main and local installation of the verified signed artifact. Implementation is on `fix/cleanup-recovery-dashboard`; see the [specification](../specs/2026-09-08-cleanup-recovery-and-model-dashboard.md).
