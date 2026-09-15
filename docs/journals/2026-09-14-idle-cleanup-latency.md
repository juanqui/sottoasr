# Idle Cleanup Latency: Diagnosis and Prewarm

- **Version:** 1.0
- **Date:** 2026-09-14
- **Status:** Approved

## 1. Symptom

First dictation after startup or a long idle period takes multiple seconds
extra at stop ("cleaning…"), while follow-up dictations are fast.

## 2. Evidence

Log pairing (idle gap between consecutive cleanup batches → that batch's
elapsed ms), same resident sidecar pid throughout:

| Gap | Batch ms |
|---|---|
| 133 min | 6443 |
| 62 min | 5814 |
| 54 min | 3729 |
| 28 min | 2795 |
| ≤15 min | ≤1990 |
| <2 min | ~1100–1700 |

- The stop-path latency timer starts AFTER `ensure_running` returns: spawn +
  model load are not the cost.
- Idle sidecar RSS ≈ 0.02 GB vs ~1.5–2.2 GB while active; `vm.swapusage`
  ~8 GB used on a 16 GB machine. macOS demotes/pagers-out the resident
  process's weights; nothing in app code drops the handle (the one
  "Resident cleanup process exited" line in the log is a separate event).
- Every-4h spawn lines are the updater's transient `check_model_update`
  engine — noise, not the culprit.
- MLX 0.32.2 exposes no mlock/pin residency API; `warm_model()` short-circuits
  on the stale `_warmed` bool and would NOT page weights back — only a real
  generation does.

## 3. What was tried

- Considered: periodic keep-warm heartbeat (rejected: spawns CPU/Metal work
  every N minutes forever, wakes a sleeping machine's GPU, no user
  correlation); `mx.warm_residency`-style APIs (absent); preventing swap via
  `setsid`/`posix_madvise(MADV_WILLNEED)` from Python on demand (no handle
  to the weights from the driver layer); lowering `mx.set_memory_limit`
  (doesn't stop eviction of idle pages).
- Chosen: fire a speculative tiny generation through the resident handle
  exactly when recording starts — cost lands inside the window where capture
  is audio-only anyway (spec `docs/specs/2026-09-14-idle-cleanup-prewarm.md`).

## 4. Race found and closed

A stop during the prewarm's generation hits the `llm_operation` busy gate,
whose existing answer is "Cleanup is busy preparing or loading; original text
preserved" — a SILENT skip of a real correction, forbidden. Closed with a
bounded 10 s handoff wait, gated on a synchronously-set `llm_prewarming`
flag so no other busy path changes behavior.

## 5. Side quest: harness rot

The dual-head (Retype/Replace) refactor left `src-tauri/sidecar/
test_llm_cleanup.py` checking retired single-head globals — 20 failures,
breaking `scripts/pre-release-check.sh` and CI. Ported: `_heads` dict patch,
`mode` on every batch request, combined canonical pin check, per-mode warm
expectations, measured head lengths (retype 520 / replace 407 tokens).
34/34 green (3 real-model tests skip without the cached artifact).

## 6. Result

223 Rust lib tests + 34 sidecar tests green; clippy clean. Shipped as 0.10.1.
Expected feel: first-dictation stop latency returns to the ~1.1–1.7 s warm
band regardless of idle time.
