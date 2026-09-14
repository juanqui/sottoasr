# Local Correction Reliability Experiments (Baseline, Windows, Timeout Recovery, Real-ASR Replay)

- **Version:** 1.1
- **Date:** 2026-09-12
- **Status:** Complete — live-path replay, planner probe and lifecycle qualification executed (§14–§16); throwaway harnesses removed after evidence capture.

## Table of Contents

1. [Question](#1-question)
2. [Protocol and corpus](#2-protocol-and-corpus)
3. [Production timing baseline](#3-production-timing-baseline)
4. [True-alarm timeout recovery](#4-true-alarm-timeout-recovery)
5. [Validator behavior probes](#5-validator-behavior-probes)
6. [Window-sizing sweep (extended deadline)](#6-window-sizing-sweep-extended-deadline)
7. [Contextual versus isolated windows](#7-contextual-versus-isolated-windows)
8. [Real ASR fixtures and seam/cache evidence](#8-real-asr-fixtures-and-seamcache-evidence)
9. [Reproduction](#9-reproduction)
10. [Limitations and corrected claims](#10-limitations-and-corrected-claims)
11. [Follow-up: multi-minute real-ASR evidence](#follow-up-multi-minute-real-asr-evidence-same-day)
12. [Patch-2a driver-flow verification](#patch-2a-driver-flow-verification-real-production-validationrs-throwaway-harness)
13. [Validator CPU probes: prepared context, compact repeats, capture retention](#validator-cpu-probes-prepared-context-compact-repeats-capture-retention-same-day)
14. [Real-model planner probe (production `plan_cleanup`, real sidecar)](#14-real-model-planner-probe-production-plan_cleanup-real-sidecar-same-day)
15. [Paced live replays through production functions](#15-paced-live-replays-through-production-functions-same-day)
16. [Lifecycle cancel exercise and scope freeze](#16-lifecycle-cancel-exercise-and-scope-freeze-same-day)
17. [GPU-paced replay matrix and post-hoc scoring](#17-gpu-paced-replay-matrix-and-post-hoc-scoring)
18. [Faithful Rust recomposition and final-vs-gold scoring](#18-faithful-rust-recomposition-and-final-vs-gold-scoring)
19. [Acceleration profile (rev3): decode-bound at batch 16](#19-acceleration-profile-rev3-decode-bound-at-batch-16)
20. [AppHandle cancel orchestration smoke and controlled mutation regressions](#20-apphandle-cancel-orchestration-smoke-and-controlled-mutation-regressions)
21. [v0.9.1 local release run and outstanding bundled-smoke failure](#21-v091-local-release-run-and-outstanding-bundled-smoke-failure)

## 1. Question

Reported symptom: AI cleanup is unreliable for dictations longer than about 30 seconds. This experiment set measures *where* the current single full-transcript cleanup request breaks on this machine (Apple M4, pinned MiniCPM5-2B-MLX sidecar), whether a timed-out sidecar can be reused without restart, how the frozen Rust validator behaves at the limits, and how bounded source windows trade latency, quality, and validator risk. The governing spec is [Incremental Voice Correction](../specs/2026-09-12-incremental-voice-correction.md). No production files were modified; all model traffic stayed local; the corpus is synthetic.

## 2. Protocol and corpus

Two distinct measurement modes, deliberately separated:

- **Production mode:** the unmodified `src-tauri/sidecar/llm_cleanup.py` run as a subprocess (real 10-second `SIGALRM` alarm), plus the prebuilt release-smoke adapter that imports production `llm/validation.rs` (`validate_cleanup`) — never a copy. Every proposal was re-validated against its own source through the real Rust guard.
- **Extended-deadline experiment mode:** the same module imported in-process with `REQUEST_TIMEOUT_SECONDS` patched to 120 s inside the experiment only, to observe what long generations *would* produce. Rows from this mode are labeled as such and are not production-comparable for pass/fail.

Corpus (`/tmp/experiments/sotto-correction/make_cases.py`, ids carry word counts): `w13_short_ok`, `w20_literal_keep`, `w108_repetition` (emphasis versus stutter), `w121_self_correct` (markers, restarts, mid-sentence ending), `w199_unicode`, `w208_list`, `w259_narrative6`, `w322_narrative7`, `w892_over_wordlimit` (5,552 B), `w1252_over_word_cap`, and a 38,871 B input for the byte cap. All contain fidelity markers (`KX-4471`, `forty-two`, `3.14`, `Room 212`, `Dr. Ferreira`, `not before Thursday`, `thirty-first of August`).

## 3. Production timing baseline

Resident sidecar, one request at a time, results in `/tmp/sotto-correction-baseline.jsonl`:

| Case | Words | Wall (ms) | Outcome |
|---|---:|---:|---|
| `w13_short_ok` | 13 | 976 | Applied |
| `w20_literal_keep` | 20 | 1,121 | Applied (literal `um` preserved) |
| `w121_self_correct` | 121 | 3,216 | Applied |
| `w108_repetition` | 108 | 2,830 | sidecar ok → guard **Rejected**: “Alignment count limit exceeded” |
| `w208_list` | 208 | 5,739 | Applied |
| `w199_unicode` | 199 | 6,367 | Applied |
| `w259_narrative6` | 259 | 6,961 | Applied |
| `w322_narrative7` | 322 | 9,137 | Applied |
| `w892_over_wordlimit` | 892 | 10,072 | **Failed: `timeout`** (the reported failure signature) |
| `w1252_over_word_cap` | 1,252 | 10,023 | Failed: `timeout` |
| 38,871 B | 6,244 | 0 | Failed: `text_limit` refusal by design |

Decode behavior (extended mode, tokens measured with the model's own tokenizer): steady ~50 output tokens/s; wall ≈ 470 ms + 21.1 ms × output tokens. The 10 s alarm crosses at roughly 450 output tokens, i.e. approximately 330–360 input words / 2,100–2,400 bytes on this machine. Repeated fresh-process production runs of `w322` (the near-crossing case) measured 9,137 / 9,555 / 8,397 / 8,343 ms — all four succeeded, but with only 0.4–1.7 s of margin, and the same case measured 8,069 ms in-process and 11,462 ms once in extended-alarm mode (that row could not have passed production). Near the crossing the honest description is high run-to-run variance with a thin remaining margin, not an observed coin-flip: no production-mode `w322` failure has been measured; failures are deterministic only clearly above the crossing (892 words timed out on every attempt). A fresh process load plus warm took 1,789–2,926 ms (four samples), confirming the ≈2.6 s restart cost observed in user logs. Below the crossing the output budget (`2·input+32`) never bound: output tokens always approximately equaled input tokens, so there is no partial-truncation mode at these sizes — failure arrives only as alarm timeout or guard rejection.

Manual semantic inspection (`/tmp/sotto-correction-semantic-inspect.txt`): delivered text on every accepted case is pure source deletion; the model's attempted curly-to-straight quote changes, `Okay um`→`Okay,` rewrites, and dropped self-corrections (`notebook, I mean the blue` → keeping green only) were all correctly refused or filtered by the validator, which delivered only the safe deletion subset. Marker counts were preserved in 100% of accepted runs.

## 4. True-alarm timeout recovery

`/tmp/sotto-correction-true-timeout-health.json` (production subprocess, unmodified 10 s alarm): an 892-word request raised the real `TimeoutError` (`error_code: "timeout"`, wall 10,020 ms). Without restarting the sidecar, five subsequent short cleanups returned **bit-identical text to the pre-timeout reference** in 1,027–1,096 ms, and a 121-word cleanup also succeeded. On this machine and build, `generator.close()` plus `mx.clear_cache()` leaves native generation healthy after an alarm-cancelled generation. Caveats kept deliberately narrow: one machine, one model build, five follow-up probes, no private audio; the Rust path SIGKILLs the subprocess mid-alarm on timeout today, so the clean-close sequence measured here has never actually run under production timing. This supports demoting sidecar `timeout` from the zombie class (return `TimedOut`, keep the handle) while keeping kills for genuine pipe death; treat it as strong machine-local evidence, not a universal safety proof.

## 5. Validator behavior probes

Through the production adapter (`/tmp/sotto-correction-guard.json`):

- Identity proposals short-circuit even above 1,024 words; any *edit* above the word cap is refused (`Word limit or word addition`).
- Salvage works as designed: a proposal with one hallucinated rewrite late still contributed its earlier valid deletions (`salvage_hallucination_late` accepted).
- Alignment blow-up is real: 490-word repeated-stutter inputs can exhaust the 128-alignment limit, and one genuine model proposal at only 108 words (`w108_repetition`) was refused wholesale. A refused proposal loses every edit in that result.
- Word-order rewrites (`Friday, no, Monday` → `Monday`) are rejected by design; the frozen policy only authorizes deletions.

## 6. Window-sizing sweep (extended deadline)
`run_window_sweep.py`: full versus 40/60/80/120-word sentence-split windows (and word-count splits for a punctuationless variant), each window cleaned and validated against its own source; reassembly checked for marker loss. **This mode patched the alarm to 120 s, added no left/right context overlap, and used a simple splitter — it measures relative per-window cost and validator behavior only.** Rows: `/tmp/sotto-correction-windows.json`.

- Per-window maximum wall: 40 w → 1.6–2.5 s, 60 w → 2.0–3.2 s, 80 w → 2.2–3.8 s, 120 w → 3.0–4.8 s. Every windowed configuration keeps ≥ 2× headroom under the production 10 s alarm; 40–80 words is the recommended target zone.
- Chunking removed the whole-result alignment rejection observed at full size for `w108_repetition` (all 40/60/80-word windows accepted and applied). At 120 words the repetition case failed the same alignment limit. **Correction (same day):** an earlier write-up framed a related claim as a measured output-*quality* delta (an arbitrary-flag case, "i have a um a pen", coming out worse chunked) — that claim is retracted; it was never a measured quality difference, and quality stays the frozen qualification benchmark's territory. What IS measured here stands unchanged: validator acceptance coverage — the 108-word stutter case fails whole-pass (fail-close to raw) yet validates and applies cleanly at 40–80-word windows (marker fidelity 0 losses both ways).
- Punctuationless word-cut windows: one raw fallback in nine windows at 40 words; none at 60–120. Raw fallback never lost a marker (0 fidelity losses across 25 configurations).
- Total GPU work while windowed measured 1.2–1.5× of a single full request, which during recording overlaps the microphone instead of the stop path.

## 7. Contextual versus isolated windows

`run_context_compare.py` (extended mode): ~40-word targets with up to 30 words of left and 20 words of right context fed as one ordinary cleanup request, versus the isolated target. Contextual requests cost 1.6–1.8× per request (2.0–3.0 s versus 1.5–1.8 s) and the model proposed deletions inside the *context* regions as well (positional walks found 48 and 88 out-of-target deletion positions across the two cases). Acceptance rates cannot distinguish the modes because a rejected window safely falls back to raw. Consequence adopted into the spec design: filtering contextual results to target-contained ranges is necessary, and the filtered subset must be re-validated in full-window context before applying — half-window application is unsafe. The mid-sentence restart `Friday, no, Monday` was preserved (not resolved) in both modes, matching the frozen policy.

## 8. Real ASR fixtures and seam/cache evidence

Synthetic speech fixtures were generated with macOS `say` at 48 kHz mono (`seg0` 5.4 s, `seg1` 35.5 s, `combined` 40.9 s) and transcribed with the **production FluidAudio path** via the existing `examples/asr_fixture` (models already cached; no downloads). Findings (`/tmp/sotto-correction-asr-fixtures.jsonl`, `-e1-windows.jsonl`, `-e1-cachehit.txt`, `-realasr-cleanup.json`):

- Real ASR of the 40.9 s recording (121 words) cleaned in 3,797 ms in production mode, guard accepted; the delivered text is a pure-deletion subset — fillers and stutters gone, `no, Monday` restarts, `3.14%`, `KX4471`, `31st of August`, `42`, `room 212`, `Dr. Ferrara` (an ASR mishearing of Ferreira — cleanup correctly preserves ASR errors) all retained.
- Window ASR diverges from the authoritative full pass: exact-substring cache-hit proxy on real text scored 3/5 closed spans hit, 1 case-only miss, 1 hard miss near the overlap region; naive punctuation splitting produced a truncated `...because Dr.` candidate, confirming the spec requirement to reuse the validator's abbreviation guard for span closing. Window-interior hit expectations of roughly 60–80% with no guarantees were relayed to the spec owner.
- Seam protection probes (real validator, `/tmp/sotto-correction-quote-seam.txt`, `/tmp/sotto-correction-quote-interior.txt`): deletions inside a quote are rejected whenever a quote CHARACTER is visible in the validated source — including an unpaired opener at a window edge (protected to EOF) or a closer at the start — and in-window in-quote deletions are rejected wholesale. BUT protection is not seam-complete: a window lying wholly inside a quote, containing no quote characters, shows zero protection spans and its filler deletions ACCEPT against the window while the same edits against the full quoted source are rejected (`Protected span changed`). Consequence: window-local validation is quote-blind for quote-interior windows, so a global re-check of every cached edit against protection spans computed on the FINAL text is mandatory, not redundant. Under the frozen whole-result policy a quote-touching window additionally loses ALL its edits — the motivation for per-edit filtering (`validate_cleanup_with_edits` prefilter + full-window revalidation of the filtered subset).
- ASR-side costs on this machine (production FluidAudio, `asr_fixture`): the full 5-minute pass took 1.83 s of processing (RTF ≈ 0.006), each 20 s window 0.18 s — the stop-path ASR tail is small next to cleanup costs (the fixture harness reports per-file processing time only).
- Revised-source scenario (real validator): replaying yesterday’s accepted proposal against a final whose text drifted (one duplicated region + one word change) was rejected wholesale (`Protected span changed`) — the frozen validator cannot partially re-anchor stale proposals, so stop-path reuse must come from per-span cached edits re-validated in full-window context, not proposal-level replay (confirmed by construction, not inferred).

Cache-mapping policy measurement (director's correction, `/tmp/sotto-correction-offsetmap.txt`): monotone in-order replay of cached spans against the final text (first match at/after previous offset) accepted only 3/32 unique keys on the 5-minute fixture because overlapping windows revisit old source regions; mapping EVERY unique exact key to all its final-text occurrences independently, sorting matches by final offset, and discarding overlaps recovers 14/32 keys covering 20% of final bytes. Repeated dictation content (same sentence at offsets 857 and 4453) shows one cache key legitimately maps to several final offsets, so cached results are reusable per unique key but the final-offset selection must be independent of recording arrival order. Sampling caveat: these numbers come from 17 s window advance; the planned 6 s scheduler tick with 20 s windows produces roughly 3× denser keys per region (higher coverage, more duplicate-offset collisions) and must be measured live rather than extrapolated.

Scheduler-density measurement (planned parameters at the text level, `/tmp/sotto-correction-sched-cache.txt`, window slices in `audio/sw*.wav`, transcriptions `-sched-windows.jsonl`): the same 5-minute fixture sliced at 20 s windows advancing every 6 s (48 windows, production FluidAudio) yields 65 unique closed-span keys (2.1 per window) whose independent-offset mapping selects 25 non-overlapping final-text occurrences covering 45% of final bytes — roughly double the 20% coverage of the 17 s-advance sampling above (monotone in-order replay remains near-useless: 5/65 best-case). These are text-level numbers only (the real driver's key set and span rules differ) and bound nothing; the live-path run supersedes them. Throughput arithmetic from the same data: at a 6 s tick, two overlapping window contents become due per tick while a contextual ~40-word request costs 2–3 s, so a single sidecar can stay ahead only if the driver processes at most one window per tick and lets coverage lag the microphone by one window — queue behavior (drop stale, keep right edge) is what keeps-up-or-falls-behind in practice and must be logged live (queue depth per tick, last-completion offset at stop).

Gate-ordering proof for the stop path (`/tmp/sotto-correction-gate-e2e.txt`, real validator, text-level emulation of the planned cache gate): replaying an in-quote cached deletion against the final text makes the whole composed proposal fail (`Protected span changed`); dropping the overlapping run BEFORE composing and then validating the kept-runs-only candidate ACCEPTS and still delivers the non-overlapping outside-quote filler deletion. The global gate must therefore be per-run overlap filtering computed on `protection_spans(F)` before composition (the implementation’s per-deletion-run design), not post-hoc proposal replay and not a blanket target discard. Note the harness `delivered_target` field is my token-join slice and strips punctuation attached to dropped tokens — only the accept/reject decision and which words remain are evidence here.

## 9. Reproduction

Harness and all logs live outside the repository (`/tmp/experiments/sotto-correction/`, index at `/tmp/sotto-correction-README.md`). Key commands:

```bash
# Production-mode baseline + guard over the corpus (repo root):
cd /tmp/experiments/sotto-correction && \
"$HOME/Library/Application Support/com.sottoasr.app/llm-venv/bin/python3" \
  run_baseline.py /tmp/sotto-correction-baseline.jsonl

# True-alarm timeout + same-process reuse health (production sidecar, unmodified):
"$HOME/Library/Application Support/com.sottoasr.app/llm-venv/bin/python3" true_timeout_health.py

# Real ASR of synthetic fixtures (models already cached; requires the app's cached CoreML models):
cd src-tauri && cargo build --example asr_fixture --features asr-fluidaudio && \
  ./target/debug/examples/asr_fixture /tmp/experiments/sotto-correction/audio/*.wav
```

## 10. Limitations and corrected claims

- Timings are single-machine interactive measurements (M4, app venv), not controlled throughput benchmarks; concurrent app sidecar was running but inference was strictly serial per process.
- The sweep and context-comparison modes patched the alarm to 120 s; their absolute pass/fail rows (e.g. full 322-word accepted at 11,462 ms) are impossible in production and must be cited only as relative-cost evidence. Production-comparable numbers are the baseline tables only.
- “Deterministically fails” applies only clearly above the measured crossing; near ~300–360 words the margin thins to < 2 s with ≥ 15 % run-to-run variance — treat failures there as possible-but-unobserved, not as a measured rate.
- “Safe” timeout reuse is observed behavior on this build/machine for five follow-up requests, not a universal proof; the follow-up production-path verification (and permanent tests) belong to the implementation.
- Marker-count fidelity is not semantic fidelity; manual diff inspection covered the seven accepted full cases plus the sweep reassemblies. Larger adversarial quality sets belong to the frozen qualification benchmark, not this journal.
- The live recording-path smoke (retain-all collector, incremental scheduling, cache-hit and stop-latency measurements against a real session) is pending the implementation's harness surface; this journal will gain a follow-up section when it lands.

## Follow-up: multi-minute real-ASR evidence (same day)
A 5.0-minute synthetic speech fixture (`audio/long_combined.wav`, 857 written words spoken by macOS `say`, 48 kHz mono) was transcribed through the production FluidAudio path: 841 words / 4,735 bytes of real ASR text. Sent as one full production cleanup request it failed exactly like the user report: `timeout`, wall 10,025 ms; the same sidecar process then answered a short cleanup correctly in 1,055 ms (second independent recovery sample, same build).

The same recording was sliced into the spec's candidate parameters (20 s windows, 3 s overlap, 17 slices) and each window transcribed through production FluidAudio. Closed spans (abbreviation-guarded terminators, ≥6 right-context tokens, ≥4 words) yielded 38 candidates from the windows; measured against the authoritative full-pass text: 14/38 (36%) exact byte-substring matches, 26/38 (68%) case-insensitive. The gap is capitalization/prose-segmentation drift between window and full passes (real ASR, not synthetic text). Consequence for the cached-edit design: with exact-byte cache keys, multi-minute stop paths should expect a substantial minority of hits and reprocess the rest; the spec's no-guarantee wording is confirmed on real audio, and the 60–80% interior-hit reading from the 40 s fixture did not hold at 5 minutes. Artifacts: `/tmp/sotto-correction-e1-long.jsonl`, `-e1-long-windows.jsonl`, `-e1-long-cachehit.txt`, `-longprod.txt`.

Sequential in-order replay of the cached-span strategy against the authoritative text (the operation the stop path actually performs — find each cached span at/after the previous offset) hit only 3/38 spans at 5 minutes; the unordered any-substring count of 14/38 overstates reuse. Exhaustively reprocessing every uncovered region post-stop with span-sized production requests measured 19 serial requests, individual spans all ≤ 3.0 s (safe under the 10 s alarm; 18/19 guard-accepted), total 31,381 ms — versus today's single 841-word request timing out at 10,025 ms with zero edits delivered. The same process that had just true-timed out served all nineteen follow-up requests correctly (third independent reuse sample). Interpretation held deliberately narrow: exhaustive post-stop reprocess preserves correctness but is not a latency win unless during-recording work keeps ahead of the microphone; measured per-window cleanup (40-word span ≈ 1.2–1.8 s) shows that is feasible at ≥6 s intervals with one in-flight request. Artifacts: `/tmp/sotto-correction-stoplatency-sim.txt`.

Context token overhead (real tokenizer, production prompt assembly, `/tmp/sotto-correction-context-token-overhead.txt`): the frozen system-plus-five-shot prompt is a fixed 380 tokens per request — a 15-token tail target pays 25× prompt overhead, explaining the ~1.0 s wall floor of any request. Left-30/right-20 context around ~40-word targets raised input body tokens from 841 to 1,731 (+106%) over 16 real spans; because the output budget is `2·input+32`, that overlap is worst-case worth ≈ 2×Δ extra decode tokens (≈ +2.2 s per request at the measured ~50 tok/s, matching the 1.6–1.8× per-request wall increase in §7). The same budget formula explains the byte-scale cap: a 1,024-word span implies ~2,100 output tokens ≈ 42 s of decode, far over the 10 s alarm — a ~45–80-word span cap measured 1.2–3.0 s and keeps ≥ 3× headroom. Retained-audio accounting for the collector design: 48 kHz f32 mono = 11.52 MB/min (the 5-minute fixture = 57.6 MB; a 23 s window-plus-overlap buffer = 4.4 MB).

## Patch-2a driver-flow verification (real production `validation.rs`, throwaway harness)

A throwaway binary #[path]-imported the actual production `llm/validation.rs` (the same technique as the release-smoke adapter) and exercised the intended driver flow end-to-end against REAL sidecar proposals for L≤30 + target(≈40w) + R≤20 windows on three corpus cases (`/tmp/sotto-correction-driverflow.json/.txt`): full validate → subset deletion flags to the target token range → `deletions_candidate` → revalidate the subset inside the FULL window → slice the corrected target. Results: 10/11 contextual windows passed every stage (`revalidate: ACCEPT`), one (heavy-stutter window at 44 words) hit the frozen alignment limit at the first stage and fail-closes to raw — confirming span size alone does not eliminate alignment risk and the raw fallback is the load-bearing safety valve. Context-region edits were present in 8/11 proposals (up to 5 deletions in left, 3 in right per window), independently confirming that target-subset filtering plus full-window revalidation is required before delivery. A build gap was observed at handoff: `#![deny(warnings)]` made the new surface a non-test `cargo check` error until the driver calls exist (reported; resolving in the driver patch — unit and adapter paths were green).

Vocabulary-term seam probe (same harness, `/tmp/sotto-correction-protection-seam.txt`): a multi-word protected term (`blue spacers KX-4471`) is protected when fully inside a window and from the second word onward when the window begins mid-term, but a window containing only the term's first word (`pass me the blue spacers`) yields zero protection spans — and where ASR fillers break the term inside the source (`the blue um spacers KX`), the filler deletion was accepted because the term no longer matches contiguously. Consequence for the driver: cached/delivered edits must not be able to delete the opening word of a multi-word vocabulary term visible at a window edge; either seed window targets to term boundaries or check `protection_spans` on the FINAL text over the target region, not just the window, before applying a deletion at a term's first word. Quote seams, by contrast, are fail-closed on both fragments (§8), so this asymmetry is specific to vocabulary terms.

## Validator CPU probes: prepared context, compact repeats, capture retention (same day)

Reviewer-side measurements on the **pure-CPU validator path** (scratch release-profile Rust harness, separate cargo target; synthetic corpora; M4, single machine). These qualify nothing about sidecar latency, stop-path wall time, or native generation health, and do not touch the §2 production-mode versus extended-120 s-sweep distinction — no model traffic in any probe below.

- **Prepared-context compose cost + allocation VOLUME** (`/tmp/sotto-correction-perf2.txt`; the `DeletionContext` port that landed in `validation.rs`): on a 43,918 B / 6,940-token corpus replaying all 6,940 single-deletion runs, per-run compose fell ~8.3–8.7 ms → ~0.2 ms (35–36× wall), with accepted-run counts and terminal outputs equal (319 accepted, outputs byte-equal; equality gates: 982 corpus vectors + 600 random vectors, 0 diffs). Allocation *volume* (not peak RSS) dropped 896,770,977 → 105,171,343 bytes (8.5×) and 3,569,893 → 19,998 allocations (178.5×). Isolated prepare/compose numbers — not real stop latency.
- **Compact repeat groups, before/after the port** (`/tmp/sotto-repeatstress-final.txt`, `/tmp/sotto-repeatstress-filtered.txt`, `/tmp/sotto-postport.txt`): the materialized `Vec<Vec<Range>>` repeat scan is quadratic on filler-dominated text — stock `DeletionContext::prepare` measured 5.7–9.5 ms at 512 words, 90–150 ms at 2,048, 203–347 ms at 3,072 (alternating-word and comma-gap variants same class; period-break text ~linear in stock at ~1–1.6 ms). After the `RepeatGroup{width,start,end}` + dominance-filter port, the SAME production `prepare` path measures 0.5 ms @512, 1.95 ms @2048, 2.9 ms @3072 filler / 1.7 ms @3072 alternating, and group volume collapses 2,047 → 1 (filler) and 4,086 → 6 (alternating) at 2,048 words. Caveat kept honest: on punctuation-broken inputs the compact form pays ~2–3× stock's (already ~linear) cost from fixed per-width buffers; inputs with interior periods were already cheap in stock.
- **Behavioral equivalence of the port** (`/tmp/sotto-postport.txt`, ported tree vs the release-epoch stock implementation, **0 mismatches** on verdict *and* output string, itemized by probe type — units overlap across probes so no aggregate is quoted): 21,728 allowed-authorization masks; 6,000 protected-span witness lists incl. multiword terms; 21,781 + 5,467 random full-vector verdicts (terms off / on); 2,016 fixture proposal + sparse-pair vectors over the fixture corpus (672 cases / 505 unique sources) with their authority term sets; 6,000 all-true vectors (guard-removal restoration semantics); 20,000 block-union masks. The dominance filter intentionally drops suffix subgroups *internally* (elementwise group equality deliberately not claimed); equality is asserted at the acceptance/output layer, plus one permanent regression (`bad_gap_maximal_run_does_not_shadow_ordinary_suffix_run`: `the the.\nthe the` → deletes the second trailing `the`).
- **Capture retention tests** (`/tmp/sotto-capture-retention.txt`): `cargo test --lib audio::capture::` 12 passed / 0 failed — 3 new (collector retention equals raw drain including stop-tail; collector snapshots read-only over retained PCM; cancelled recording never replays prior PCM into the next one, via sender-injected scripted capture) + 9 pre-existing RMS/error tests. Scope: the audio-slot collector half only — not the LLM-side quiesce/join lifecycle (open, replay-harness territory) and no stop-latency claim.
- **Status (superseded same day):** the live-path runs landed — see §15–§16.

## 14. Real-model planner probe (production `plan_cleanup`, real sidecar) (same day)

Throwaway in-crate `#[ignore]` harness (`real_model_planner_probe`, since removed; log `/tmp/sotto-probe-run1.txt`, outputs `/tmp/experiments/sotto-correction/probe_out_{case}_{cfg}.txt`). The REAL installed sidecar behind a counting passthrough; mock ASR stands in for audio only; production `plan_cleanup` with a fresh session handle IS the stop-path planner. Corpus `/tmp/experiments/sotto-correction/probe_cases.json`; sidecar load 3930 ms once.

| case | cfg | requests | sidecar sum ms | max req ms | wall ms | fillers in→out | literal keeps |
|---|---|---|---|---|---|---|---|
| short13 | 40/30/20 | 1 | 1019 | 1019 | 1037 | 2→0 | 1/1 |
| short13 | 20/12/8 | 1 | 1015 | 1015 | 1018 | 2→0 | 1/1 |
| w108_stutter | 40/30/20 | 6 | 11897 | 2147 | 11989 | 3→2 | 4/4 |
| w108_stutter | 20/12/8 | 8 | 11101 | 1586 | 11153 | 3→1 | 4/4 |
| w892_long | 40/30/20 | 32 | 64595 | 2611 | 64958 | 36→29 | 7/7 |
| w892_long | 20/12/8 | 39 | 57933 | 2008 | 58137 | **36→0** | 7/7 |

Readings kept honest (director-confirmed corrections): `prompt_bytes` in the probe lines is the SUM over requests (t40 11063 B / t20 6453 B); PEAK single request was 464 B (t40) vs 247 B (t20) — both far under the 4 KB soft gate. `w892_long` is a 5,548 B input: “892” counts whitespace words, 988 is the validator-token count; it was never the >32 KB refusal case. The historical long-dictation alarm (§2/§3) was the Python-side 10 s itimer, not `text_limit` nor the Rust 30 s outer timeout. Double-count fidelity counts are strict-adjacent-only. **Outcome: 20/12/8 won on fidelity and per-request latency at equal wall; it became the production default.** Serial no-cache wall (58.1 s at 39 requests) showed request-bounding alone does not solve long-dictation stop latency — cache coverage is the load-bearing claim, measured next.

## 15. Paced live replays through production functions (same day)

Throwaway in-crate `#[ignore]` harnesses (since removed): WAV decoded → 100 ms drift-corrected paced capture backend → real `start_recording_capture` + collector + real `run_driver` (rolling-window CoreML ASR) + real `run_worker` (REAL sidecar, lazy-spawned mid-recording: both runs started with `llm_loaded=false`, cold path) → manager-ordered stop → final full-file ASR → `run_cleanup_with` → then the SAME planner with no cache for comparison. Artifacts per run: `/tmp/experiments/sotto-correction/replay/{name}/{windows.jsonl,final.txt,out_cached.txt,out_nocache.txt}`. Config = production default 20/12/8. Logs `/tmp/sotto-replay-40s.txt`, `/tmp/sotto-replay-5min.txt`.

| metric | combined_30s (40.9 s) | long_combined (299.8 s) |
|---|---|---|
| ASR init (harness, once) | 15056 ms (first CoreML init in process) | 336 ms |
| PCM identity | 1,963,804 samples == WAV, err None | 14,391,517 == WAV, err None |
| at stop: dispatched / windows done / dropped / queue | 9 / 3 / 0 / 2 | 141 / 41 / 14 / 6 |
| quiesce | 406 ms | 0 ms |
| stop → capture finish + join (`stop_complete_ms`) | 532 ms | 1133 ms |
| final full-file ASR | 382 ms | 2489 ms |
| post-stop CLEANUP (`plan_ms`) | 8804 ms | 112725 ms |
| live sidecar during recording | 6 settled, cold spawn mid-recording, 0 transport failures | 135 settled (max 5445 ms = cold-spawn first request), cold spawn mid-recording, 0 transport failures |
| pre-drain cache snapshot (token-union over ALL uniquely-located targets; newest-first overlap skips NOT applied → locatability UPPER bound, ≥ actual phase-1 coverage) | this run's snapshot fired post-drain (harness bug fixed before the 5-min run) → printed located=0/0.0%, NOT restatable; counts above derive only from the 6 settled lines + settled log | located=26 / 109 keys absent-or-non-unique / union upper-bound 226/844 (26.8%) / sum of SETTLED requests' elapsed_ms (successful requests only; includes the cold-spawn first request; rejected/failed requests excluded) 253,911 |
| cached vs no-cache output | byte-identical (613 B) | byte-identical (4652 B) |


Honest readings: (1) at 5 min post-stop CLEANUP fell −33% (112.7 s vs 169.4 s on the same planner; fresh dispatches 67→46 — 68 planned windows, one duplicate fresh question reused → 21 dispatches eliminated) with identical output; this compares the cleanup phase only, not the whole stop-to-output path. The pre-incremental whole-text path at that size hit the model-side timeout; 112.7 s remains a REAL limitation when the live cache mostly misses. (2) At 40 s the bounded planner's cleanup (8.8 s) is SLOWER than the historical single whole-text request on short fixtures (3.8 s, §3): the win there is bounded request size and reliability, not speed — no universal-speedup claim. Per-request latency differs by context: probe fixtures peaked 2611/2008 ms (§14); live 5-min settled requests peaked 5445 ms (cold-spawn first request). (3) The locatability bottleneck is ASR-text equality, not the matching rule: of 135 settled live answers only 26 keys occur uniquely word-aligned in the final transcript (their target union = the 26.8% locatability upper bound; actual phase-1 replayed 22 after newest-first overlap deferral — the upper bound is an over-count, coverage of the selected set is lower); exact matching was deliberately NOT relaxed. (4) Phase-1 overlap-skips: 4 targets (20+16+2+5 = 43 tokens ≈ 5.1% upper bound); partial-clipping of older entries measured insignificant → not implemented. (5) No-cache comparisons are against the SAME new incremental planner only. (6) Fidelity on the 5-min fixture (director read of final.txt/out_cached.txt): numbers, dates, negations, self-repairs and quoted wording preserved; 2 fillers retained; ASR name errors unchanged — conservative cleanup, not a rewrite. 19 runs / 21 tokens removed per the stop-complete log.

## 16. Lifecycle cancel exercise and scope freeze (same day)

Throwaway `#[ignore]` exercise (since removed; log `/tmp/sotto-lifecycle.txt`, 37.03 s): live driver+worker joins against real window ASR, sidecar stub hanging past the 30 s outer deadline so cancel lands inside the non-cancellable leg. Manager ordering asserted end-to-end: claim transition → take slot + `request_stop` → `finish_recording_capture` → `quiesce`. Measured: quiesce 29,782 ms (bounds 25–45 s asserted from the measurement, not restating the timeout constant); ASR mutex free +128 ms after quiesce (follow-up transcription ok); sidecar lock free; task list drained exactly once; cancelled recording's persisted PCM == decoded WAV content+order with only zero padding after; next recording: generation advances, fresh handle with EMPTY cache/empty window log, stale-channel marker drained by the production start path, 0 leaked samples, prior session cache untouched.

**Scope freeze after this qualification:** no further sizing/window tuning (directed), no partial-clip phase-1 change (measured <5.1% upper bound), no fuzzy/case-fold key relaxation. Harness code (probe, paced replay, lifecycle exercise) and telemetry-only handle fields (`window_log`, `started_at`, `cache_len`/`failed_len`/`window_transcripts`) removed post-evidence; 14 kept correction regressions + 17 validation regressions green; full `cargo test` 212 passed; `cargo build` and `cargo clippy --all-targets -- -D warnings` clean (`/tmp/sotto-final-{build,clippy,test}.log`).

## 17. GPU-paced replay matrix and post-hoc scoring

GPU-backed (MLX sidecar real inference) paced replay matrix executed through
the shipped planner/backend path: **26/26 matrix rows + 6/6 extension rows
admitted** (native batch-16 warm/cold arms plus all-keys arms), one serve
process, one `AppState`, no pattern-kills. Artifacts (ephemeral `/tmp` paths;
result JSON kept only under gitignored `benchmarks/` scratch, per plan):
`/tmp/experiments/sotto-replay-build/source-review/posthoc-summary.json`
(per-row timings), scored rows `posthoc-scored.jsonl` +
`posthoc-scored-extension.jsonl` (base scorer as-executed sha
`6593f109f2928780631a26f03d49f331ddacc88497cc557d4f3a8f6d16a520c1`), scored
against five SHA-bound source-only golds (`sourcegold-*.json`).

Headline warm stop→final times (median of repeat warm runs): 30 s fixture
381 ms; v2_30s_pos 1.903 s; v2_180s_dense 8.899 s (cleanup 7.462 s);
v2_600s_brisk 20.009 s (cleanup 16.020 s); v2_600s 7.938 s (cleanup 4.391 s).
Warm == cold output identity held 16/16. These timings bound the *paced*
scenario (windows arrive in real time during capture); they are not
hardware-peak claims.

## 18. Faithful Rust recomposition and final-vs-gold scoring

The paired-fidelity phase was reworked to run the **actual shipped Rust**
composition/adjudication (byte-twin of `plan_cleanup` in a private copy crate;
9 arms through the real compose/adjudicate functions), replacing the earlier
Python-gated recompositions, which remain UNADMITTED. Keys byte-exact; mask
no-work proof cross-checked with an all-active twin (`allactive-override.diff`,
head_len 365 pinned). Artifacts:
`/tmp/experiments/sotto-paired-rust/out/paired_rust.json`, provenance pin
`/tmp/experiments/sotto-paired-rust/provenance.json`
(`src/llm/correction.rs` = `21518ac32170b36b196f1cc8aacae3d5657468460df4d68b06d12cbe019cd2ff`,
byte-identical to repo at qualification time). Applied-run counts: serial
15/68/158, native1-allkeys 17/66/161, native16-allkeys 17/66/162.

Byte-residual join-gap audit: **32/32 PASS_JOIN_GAP** — every residual byte
diff sits in a join gap spanning deleted words (164 commas + 1 124 spaces);
18,650 zero-delete gaps byte-identical
(`source-review/residual-audit-joingap-32.json`).

Final-vs-gold scoring of the 9 actual-Rust finals (`out/rust-final-scoring-v2.json`,
Director-corrected naming — control column `matrix_native16` = matrix warm
final = production bs16; `original_serial`; `native1_allkeys`;
`native16_allkeys`): per-occurrence exact source byte-spans, never collapsed
to unique surfaces. Disclosed recall tradeoff vs `original_serial`: 30 s
matrix gains 2 hesitation occurrences (`um@[230,232]` tok 45, `uh@[355,357]`
tok 71); 180 s matrix misses 2 (`uh@[495,497]` tok 102, `um@[1133,1135]`
tok 235); 600 s matrix gains 4 / loses 1, net +3 (163→166 ops). False-positive
deletions, keep-one violations, semantic failures and other-op deltas: zero
across all 12 rows; 12/12 value-parity controls against the admitted
matrix-scored rows; universal checks 9 + 3 pass. The earlier
global-`windows[0]` scoring pass is explicitly INVALID (unique-surface collapse
on "uh"/"um"); superseded, not reused.

The v2 pass ran via driver `score_rust_finals.py` (as-executed sha
`833c39c9dd13fc1c67c2e79d87c7a259d0e00fb0a33e40abf1ed5c30223c312a`, importing
the §17 base scorer).

## 19. Acceleration profile (rev3): decode-bound at batch 16

Investment closed with configuration levers measured **neutral-or-worse**;
bs16/pss64 retained; no production change came out of profiling. Canonical
report `/tmp/sotto-acceleration-profile.md` (rev3 sha
`78cd30e9932e2432c8f7501403b808d672920dfb01d0248df8259df7b40d53d2`; probe
artifacts `/tmp/experiments/sotto-accel/probe{2..8}.json`) — retained raw,
including its heuristic stitching passages, which are **appendix-scope
qualitative notes, not source-coverage claims**: local byte coordinates are
per-key and not comparable across keys, so there is no grounded union
"source-coverage" figure (the 980 target-word sum is per-key over the 54
dispatched requests, not the 1 485-word transcript; BPE and word units are
never divided into each other). The journal carries only the exact
per-request summed bytes/words (report §8 table: 600 s input 10 104 B /
target 4 764 B; 180 s 4 354 / 2 061; 30 s 803 / 408) and the qualitative
overlap cost (suffix/prefix context re-sent per key; replies ≈1.6× target
words median). Bottleneck attribution: decode-at-bs16 dominates
(66–68 ms/step) > tail-prefill > fixed ≤40 ms/dispatch. Failed alternatives
(all measured, kept here so nobody re-tries them silently): KV
`cache_limit` 128/512/1024 neutral; pss 128 slower-or-tied; re-chunking
slower; BF16 vs FP16 no gap; speculative decoding retired (draft mismatch);
batched encode immaterial. `traceSource` captured per-call source ranges
only — it cannot attribute per-kernel GPU time on this stack, so no
hardware-peak/utilization claims are made anywhere in this work. Divergence
accounting (reply/target median 1.6×) answered; target-only output remains an
UNTESTED opportunity, not a claim.

## 20. AppHandle cancel orchestration smoke and controlled mutation regressions

Two honest-scope additions, both private-copy only (`/tmp/experiments/sotto-replay-build/crate`;
shipped binary untouched — `cancel_smoke.rs` is an added module + bin shim in
the copy crate).

**(a) Cancel orchestration smoke** (`src/cancel_smoke.rs`, source sha
`650c7cfb4eb120e7d52a85401597f7e6fd5541cd19c2680fa5ff738a24611fbb`, binary
sha `2f66245e81573bcb221fc3ae2d59e848df0c1918c7cc661abe82bca50fa57d3c`,
evidence `out/cancel-smoke.ndjson` + `out/cancel-smoke-stdout-v2.ndjson`,
rc=0): minimal ORCHESTRATION tests through the real manager/commands/state and
an actual Tauri event loop with **MOCKED backends** — mocked ASR (canned
string), typed no-change LLM echo mock (`Proposal(Some(key))` ⇒ successful
`{"kind":"no_changes"}` rows, NOT the invalid `Proposal(None)` path the first
run mistakenly used and which stands preserved as failed-coverage evidence),
mock paste recorder. Real-inference backend behavior is covered by the 32
paced rows of §17, not here. Covered: cancel accepted during Recording
(placeholder + full-audio rows, no paste); cancel REJECTED while
Transcribing/CleaningUp at both commands guard and handler guard (no events,
no extra writes; exactly one row+paste from the surviving run); labeled
pre-stop wrong-generation fault injection (claim rejected, zero side effects);
REAL in-flight stale-result suppression crossing both shipped gates — job-id
rotation observed discarding at the post-ASR gate and after the cleanup call
returned (log-observer gate-crossing proof; zero late history/paste/status
writes — the post-cleanup arm's key was already in flight, so its dispatch is
counted; status-cache non-overwrite proved with a labeled sentinel).
OBSERVED behavior recorded honestly: both suppression early-returns leave the
state machine stuck at Transcribing/CleaningUp until a later recording's
claim bumps it; the smoke asserts this, labels the subsequent `set_state(Idle)`
as a test-only reset (fault-injection limit of the exercise, not an approval
of it), and modifies no production behavior. Cancel shortcuts = unparseable
sentinel ⇒ 14 registration-failure + 7 unregistration-failure logs prove zero
OS hotkey capture; history redirected to a scratch dir (hard-fail if unset).

**(b) Controlled duplicate-owner-omission mutation.** The historical
same-key-owner RED log (`/tmp/redo_f844_keylocal_red.log`) predates retained
construction provenance — its patch/source authorship is NOT independently
verifiable and is labeled as such. Replacement, run once on the private copy:
saved patch `out/red-f844-keylocal.patch` (sha
`16e8dfe5f0a1c1d42c7cb7f17d86921921fc633d6654ca3c8888bfe6e350a144`) skips
every duplicate same-key owner in Phase D (a PLAUSIBLE duplicate-coverage bug
shape — **not** a faithful reproduction of the historical
reuse-the-first-owner's-local-authority bug; the in-patch comment claiming
"historical shape" is retracted here). Under it the behavior regression
`duplicate_key_owners_each_derive_their_own_authority` FAILS
(`out/red-f844-keylocal-RED.log`: surviving `um` tail); restoring the frozen
file byte-identically (sha `21518ac3…`, verified equal to repo and to the
§18 provenance pin) makes it PASS (`out/red-f844-keylocal-GREEN.log`). The
regression's standing value is proven against the REAL current source plus the
32 native-inference rows; the mutation demonstrates only that the test bites
a plausible regression, and no claim is made that it reproduces the historical
failure's mechanism.

## 21. v0.9.1 local release run and outstanding bundled-smoke failure

Release evidence lives in `benchmarks/llm/results/2026-09-13-release-evidence/`
(gitignored; MANIFEST.md carries all shas and scope notes). Gates after the
docs/version edits: repo `cargo test` 215/0; scripted clippy lane
(`--all-targets -D warnings`, default features) rc=0; frontend `npm run check`
0 errors / `npm run build` rc=0; sidecar unittest 34 OK;
`pre-release-check.sh --auto-only` 10/10 (deviation acknowledged: the stock
script ran, so its dirty-tree listing is in the log; no rerun). The literal
`--all-features` clippy is NOT a repo-required gate and fails rc=101 on 6
pre-existing dead symbols of the masked-off parakeet lane (backends are
mutually exclusive lanes); the supported alternate lane
`--no-default-features --features custom-protocol,asr-parakeet,llm-cleanup`
passes. The build is local-only: Developer ID signed, notarization skipped,
updater artifacts suppressed via CLI `--config` only (shipped
`tauri.conf.json` unchanged); installed over 0.9.0 with the old app backed up
at `/Applications/SottoASR-0.9.0-backup.app`. Fresh 0.9.1 startup logged ASR
ready + sidecar warm from the installed bundle + immutable head 365 tokens,
BUT with material WARNs: `AX functional check returned error code: -25212` /
"Paste may not work until app restart" — post-restart paste is NOT verified.

**Bundled release smoke on the installed app = 14/15 and remains an
OUTSTANDING acceptance failure** (suite contract: `reported_sentence` must
match gold exactly; the other 14 pass). Primary bytes (archived
`release-run/bundled-smoke-091.json` + `reported-sentence-comparison.json`):
the recorded sidecar reply removed all three `um`s, `uh`, `yeah,` AND `those`
while keeping `the`; the shipped validator reconstructed raw-minus-the-two-
earliest-`um`s (20 words), which is neither the 16-word gold nor the full raw
⇒ `smoke_pass:false`. A CPU-only diagnostic running the shipped
`validation.rs` functions in the terminal whole-text authority's order on the
RECORDED reply reproduces the smoke's delivered text exactly — but it is
explicitly NOT the whole production `plan_cleanup` (no multi-window planning
or Phase D composition). No unauthorized survivor, no protected-payload loss.
Whether the delivered-vs-gold delta is the disclosed g5/g7 class is NOT
asserted, and the cause of the reply difference vs the 0.8.3 baseline (both
its sidecar AND validator differ from 0.9.1's) is UNKNOWN. No gold change,
no contract weakening, no input special-casing was used to report this.

