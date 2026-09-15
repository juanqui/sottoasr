# Cleanup Mode: Retype vs Replace (experimental)

- **Version:** 1.0
- **Date:** 2026-09-14
- **Status:** In Review

## 1 · Summary

Add an Advanced setting `llm_cleanup_mode` letting users choose how the cleanup
model proposes edits: **Retype** (default; production behavior, now with the
measured post-transcript instruction tweak) or **Replace** (experimental; the
model emits only `OLD|||NEW` edit lines, which Rust applies verbatim to the
source before the frozen validator). Replace trades cleanup quality for ~4×
faster short-window correction (measured 0.9–1.1 s vs 4.3 s on ≤60 s windows).

## 2 · Problem Statement

Phase-2 sweep + EDITFMT experiment (benchmarks/llm/results/2026-09-14-slm-sweep/
EDITFMT/EDITFMT-REPORT.html): edit formats lose quality (15/46 vs 19–20/46,
refusal attractor) but DELIM-format generation collapses 124→5 tokens, a real
latency win on short windows. Users with latency-first needs should be able to
opt in; Retype stays default so no one loses quality silently. The V1P
prompt-only change (+1 pass-text, zero regressions) applies to Retype for
everyone.

## 3 · Design Overview

```mermaid
flowchart LR
  A[plan_cleanup windows] -->|texts + mode| B[sidecar cleanup_batch]
  B -->|retype: cleaned text| C[per-key proposal]
  B -->|replace: OLD|||NEW lines| P[apply_edit_lines on key bytes] --> C
  C --> D[target_authority → frozen validator] --> E[compose + terminal authority]
```

- **Settings:** `llm_cleanup_mode: CleanupMode` (`"retype"` | `"replace"`,
  serde default `Retype`, lowercase wire). `PartialEq` includes it; old
  settings.json files deserialize unchanged.
- **Sidecar:** two prompt blocks (retype = D7 + post-transcript instruction
  exactly as benchmark V1P; replace = benchmark DELIM2 protocol verbatim).
  `cleanup_batch` requests carry `mode` (strict: missing/invalid ⇒
  `invalid_request`). `warm_model()` builds BOTH heads + warms BOTH prompt
  paths at load, so no mode switch ever pays head-build inside a batch alarm.
- **Prompt pin:** `PROMPT_SHA256` becomes the combined hash
  `sha256(canonical(retype) + "\n" + canonical(canonical(replace)))` over the
  four consumed fields; load response and `validate_loaded_model` keep the
  single-string wire shape.
- **Rust parse/apply:** `correction::apply_edit_lines(reply, key)` ports the
  validated benchmark parser byte-exactly (strip `<transcript>` tags; `<KEEP>`
  ⇒ echo; per line: blank skip, no `|||` ⇒ parse fail ⇒ whole reply unusable ⇒
  region stays raw; split at FIRST `|||`; NEW==`<D>` ⇒ delete; empty OLD ⇒
  no-op; sequential all-non-overlapping replacement). The reconstructed text
  is the proposal: the frozen validator + caps path is unchanged in Replace
  mode (proposal bytes = source bytes + model NEW spans, same contract the
  apply-harness proved).
- **Frontend:** Advanced page card "AI cleanup strategy" — radio pair
  Retype / Replace (experimental). Save-to-apply; no sidecar reload needed
  (both heads resident).

## 4 · Edge Cases

| Case | Handling |
|---|---|
| Replace reply contains prose/fallback lines (no `|||`) | parse fail → region stays raw (fail-closed), measured common under adversarial text |
| `<KEEP>` / empty reply | echo proposal → settled NO-CHANGE authority |
| `find` not in source | that edit no-ops; others still apply (measured: copy fidelity is byte-exact when attempted) |
| Mode switch mid-session | next recording's requests carry new mode; resident sidecar already warm on both |
| Old settings.json | serde default → Retype (no migration) |
| Malformed `mode` field on wire | typed `invalid_request` (handle retained) |
| Budget/deadline behavior | unchanged per-mode (budget formula is text-keyed) |

## 5 · File Changes

| File | Change |
|---|---|
| `src-tauri/src/models.rs` | `CleanupMode` enum + `Settings.llm_cleanup_mode` (default Retype, PartialEq) |
| `src-tauri/src/llm/engine.rs` | trait `cleanup_batch(&texts, mode)`, wire field, combined `PROMPT_SHA256`, mock/test updates |
| `src-tauri/src/llm/cleanup.rs` | thread mode from settings → planner → batch request |
| `src-tauri/src/llm/correction.rs` | `apply_edit_lines` + mode-gated pre-validation transform + unit tests |
| `src-tauri/sidecar/llm_cleanup.py` | `PROMPTS{retype,replace}`, per-mode head/warm, strict `mode` validation, combined sha |
| `src/lib/utils/tauri.ts`, `src/lib/stores/settings.svelte.ts` | Settings type + default |
| `src/lib/components/settings-advanced.svelte` | radio card (Replace labeled experimental) |
| version 5 files + `CHANGELOG.md` + `website/index.html` | 0.10.0 |

## 6 · Testing Strategy

- `cargo test`: parser contract (delete/comma-fold/multi-edit sequential,
  missing-find no-op, delimiterless⇒None, KEEP⇒echo, tag strip, empty-OLD);
  plan_cleanup Replace-mode end-to-end with edit-line mock (applied + rejected);
  settings roundtrip default + serde shape.
- Frontend `npm run check` + existing vitest suites.
- Sidecar smoke against the BUNDLED script: status/load (both heads warm),
  cleanup_batch retype + replace on a filler text.

## 7 · Security / Cost

No privacy change (local model, no new data flows). Replace cannot inject text
beyond the validator's source-bytes authority. Cost: one extra prompt cache
(~2 MB) resident; +~1 s cold load for the second head+warmup.

## 8 · Implementation Status

Implemented in 0.10.0. Deviations: none.
