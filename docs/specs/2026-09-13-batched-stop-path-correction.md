# Batched Stop-Path Correction (Native Immutable-Prefix Batch)

- **Version:** 0.2
- **Date:** 2026-09-13
- **Status:** **Approved** (2026-09-13). Review record: P1 (Director:
  assumptions/native-API/measurements — B2/B3 facts, wire enforcement,
  actual BatchGenerator API, static framing budgets) → v0.2-rev1; P2
  (latency-experiments PASS2-findings.md F1–F8 + Director-resolved lifecycle
  corrections) → v0.2-rev2…rev6; P3 (Director: six concrete fixes —
  stop-token/cache shapes, head build order, incremental per-UID byte cap,
  Result error channel, request_timeout mapping, intended-literal wording —
  each confirmed by a temp native constructor/insert/sampler probe) →
  v0.2-rev11. Implementation authorized; this file is the single
  authoritative source for the batched stop path.

## Table of Contents

1. [Summary](#1-summary)
2. [Problem Statement (pre-change baseline — historical)](#2-problem-statement-pre-change-baseline--historical)
3. [Design Overview](#3-design-overview)
4. [Detailed Design (wire contract, planner, mask, deadline)](#4-detailed-design-wire-contract-planner-mask-deadline)
5. [Edge Cases](#5-edge-cases)
6. [File Changes](#6-file-changes)
7. [Testing Strategy](#7-testing-strategy)
8. [Migration Plan](#8-migration-plan)
9. [Security Considerations](#9-security-considerations)
10. [Cost Analysis (honest, measured)](#10-cost-analysis-honest-measured)
11. [Implementation Tasks (ordered)](#11-implementation-tasks-ordered)
12. [Known Limitations and Open Qualification](#12-known-limitations-and-open-qualification)
13. [Revision Notes](#13-revision-notes)

## 1. Summary

The installed app's (0.9.0) stop path plans every uncovered token of the final
transcript `F`
into bounded windows and requests them **serially** (one sidecar IPC round per
window). Measured on the 5-minute dense fixture: 169.417 s uncached,
112.725 s with the live cache (§2). Real-world ground truth, reported exactly:
"Performance beyond horrible. Over 1 minute after nearly 3-minute voice
input." — the >1 min is the post-stop correction wait; the ~3 min is the
RECORDING duration, not a latency bound. This change replaces the
serial per-window requests with **native `mlx_lm` BatchGenerator dispatches
over an immutable shared prefix cache**, removes the live ASR-window
transcription and its LLM result cache
(the incremental phase-1 replay), and gates dispatches with a **provable
no-work mask** derived from the validator's own structures.

- The **judgment channel stays frozen-prompt generation**: the model decides
  edits; the frozen validator remains the sole authority. No deterministic
  caps, no candidate filters as acceptance, no classifier lane, no custom
  decode loop (the speculative-decoding arm is retired — §10 measurements).
- **Every candidate is still considered**: a window is skipped only when the
  mask *proves* no validator-admissible edit and no capitalization opportunity
  can land in its target; requests are deduplicated by exact key bytes,
  **eligibility per absolute window occurrence** — never by first-owner
  (§4.3). Coverage of arbitrary `F` is algorithmic, not fixture-shaped.
- The wire contract is fixed at **≤ 16 texts per IPC request**, with Rust
  looping **all** chunks (§4.1): batch count = ceil(active unique keys / 16),
  no upper bound, no silent budget skips.

## 2. Problem Statement (pre-change baseline — historical)

Describes the state BEFORE this change — the prior incremental path (spec
document Version 0.8 in `docs/specs/2026-09-12-incremental-voice-correction.md`
— a DOCUMENT version, NOT an app release; the installed app is 0.9.0 per
`package.json`/`tauri.conf.json`; its body is preserved unchanged, journal
`docs/journals/2026-09-12-correction-baseline-experiments.md`).

| # | Fact | Evidence |
|---|------|----------|
| B1 | Stop-path planning is **serial per window**: `plan_cleanup` awaits `sidecar_cleanup` one request at a time. | `llm/correction.rs` plan_cleanup; shipped journal §14–16. |
| B2 | 5-minute dense fixture stop-path cleanup: **169.417 s uncached / 112.725 s cached** (paced replays); 40 s fixture 8.8 s cached / 13.1 s uncached. User ground truth (exact): “Performance beyond horrible. Over 1 minute after nearly 3-minute voice input.” (>1 min = post-stop wait; ~3 min = recording duration.) | journal §14–16 (paced replays); user report 2026-09-12. |
| B3 | Live-cache reuse on ONE dense fixture was bounded by ASR text equality (26/135 settled keys uniquely locatable) — reuse is brittle at least on that workload; per-recording reuse rates beyond it are not measured here. | journal §12 (single-fixture observation). |
| B4 | Native head-batched decode beats every alternative measured on identical evidence; custom speculative decode loses. | §10 table; `/tmp/experiments/sotto-correction/{batch_prefix,all67_leg,alarm10b}.json` (artifacts journalized in `MEMO-latency-density.md`). |

## 3. Design Overview

```
ASR final F, terms
   └─ plan_windows (UNCHANGED original planner; packing variant retired:
      raw 73→54 but active stays 19 — changed questions, no real gain)
   └─ no-work mask (§4.3): possible = (HES ∪ repeat-member ∪ restart-member)
      ∧ ¬protected-hit, GLOBAL, plus caps-opportunity exposure scan
   └─ occurrence-eligibility dedup (§4.3): ask key k once iff ANY absolute
      window owning k is active
   └─ chunks of ≤16 keys, in window order → sidecar cleanup_batch (§4.1),
      Rust loops ALL chunks; one resident sidecar process (today's)
   └─ per-batch: head cache (immutable public prompt prefix, built at warm)
      + BatchGenerator(prefill/completion_batch_size = n EXPLICIT,
      prefill_step_size=64), per-window budget, 10 s alarm per request
   └─ replies → per-occurrence target_authority → contiguous runs → global
      compose_runs adjudication (ALL UNCHANGED — frozen validator authority)
   └─ deliver composed F (history retains original exactly as today)
```

Live path removed (§6): no incremental driver/worker, no result cache, no
phase-1 replay. `plan_cleanup` keeps its window-planning + authority
machinery; only its request primitive becomes batched. PCM history, cancel,
authoritative full-file ASR, bounded-window authority, global-`F` validation,
history, settings: **all preserved unchanged**.

## 4. Detailed Design (wire contract, planner, mask, deadline)

### 4.1 Wire contract (exact)

One resident sidecar process (today's spawn/load/warm lifecycle, today's
`{"action":"load"}` handshake, today's newline-framed one-JSON-line
protocol — line-size caps per §4.5). New action, one line in, one line out:

Request:
```json
{"action":"cleanup_batch","texts":["<key bytes>","…"]}
```
- `texts`: **valid range 0..=16 entries** — 0 is the DEFINED no-op handled
  BEFORE any model work: `{"ok":true,"results":[]}` (caller-side `NoChanges`;
  the planner avoids empty dispatches anyway, and the sidecar returns without
  touching the generator). 1..=16 each ≤ 4 096 B raw — the existing planner
  window cap (`MAX_REQUEST_BYTES_SOFT = 4096`, shrink→skip at plan time), now
  ALSO enforced server-side per item (`validate_batch_text` /
  `MAX_INPUT_BYTES`; new consistent work). The output cap is
  **`MAX_TEXT_BYTES = 32_000`** (streaming per-item byte guard + finalize
  recheck; there is no separate cleanup-bytes constant to invent — Rust
  validates proposals against its own `validation::MAX_CLEANUP_BYTES`). The
  active unique keys into groups of 16 (count-chunks; the static bound of
  §4.5 proves the line fits — no dynamic byte packer). The sidecar enforces:
  >16 entries or an over-cap item is an invalid request — a protocol error on
  that request (typed failure; regions raw).

Response (always per-item, index-addressed, **status-tagged union** — the
shipped llm_cleanup.py shape; an earlier illustrative sketch of this block
used per-item `ok`/`finish_reason` fields, superseded by rev16):
```json
{"ok":true,"results":[
   {"index":0,"status":"ok","text":"…","elapsed_ms":412},
   {"index":1,"status":"timeout"},
   {"index":2,"status":"failed","error_code":"text_limit"}]}
```
- `results` is a permutation of `0..n-1` — exactly one entry per index, in
  input order (sealing discipline rev16: slots preallocate before any
  alarm-able work, so an alarm mid-preprocessing/mid-finalize can never
  yield a short or duplicate-index list). `status` ∈ {`ok`, `timeout`,
  `failed`}: `ok` carries `text` (possibly `""` — §5 E1) + `elapsed_ms`;
  `timeout` is the sealed deadline ONLY (a stop completed before the
  deadline seals `ok` even if a sibling later trips it; deadline precedence
  is asserted by the finalize-time clock recheck, since native work can
  defer the Python alarm); `failed` carries `error_code` ∈ {`text_limit`,
  `incomplete_generation`, `context_limit`}.
- Any response the client cannot trust — not JSON, missing `results`, index
  set ≠ `0..n-1`, duplicate index, count ≠ n, unknown `status`, `ok` without
  string `text`, over-cap line, EOF mid-line — is a **protocol fault: retire
  the untrustworthy sidecar via the existing protocol-fault path** (kill +
  clear PID; restart on next use), all this batch's regions raw, `F`
  preserved. There is no "line-sync intact ⇒ keep handle" carve-out for
  schema violations: an endpoint that emits a contract-violating response
  may emit anything next. Distinct from this: a VALID response carrying
  per-item handled codes (`timeout` from the sealed deadline, `text_limit`,
  load/unavailable codes) RETAINS the handle exactly under today's
  `is_responded_timeout` retention rules.

Rust (`llm/engine.rs` owns the item type; `llm/cleanup.rs` the dispatcher):
```rust
// engine.rs — position-addressed (index == slot order in `texts`; the
// parser validates the wire permutation and emits ordered Vec<BatchItem>)
pub enum BatchItem { Proposal(Option<String>), TimedOut, Failed(String) }

// cleanup.rs — outer per-chunk deadline wrapper around the trait call
pub(crate) async fn sidecar_cleanup_batch(
    state: &AppState, texts: &[String]) -> Result<Vec<BatchItem>, LlmCleanupStatus>;
```
- On the trait (`llm/engine.rs::LlmBackend`),
  `fn cleanup_batch(&mut self, texts: &[String]) -> Result<Vec<BatchItem>, String>`
  — **`Result`, mirroring the current `cleanup -> Result<String, String>`
  contract**: the `Err(String)` channel carries whole-request protocol/
  transport faults (retire-class: bad JSON, index-set mismatch, over-cap
  line, EOF, write/read failure — the caller MUST see these to retire the
  handle; a bare `Vec<BatchItem>` cannot express them). Per-item
  `BatchItem::Failed` ≠ whole-request `Err`. Retention split is explicit:
  controlled outer typed `invalid_request` (E7, sidecar answered validly) ⇒
  handle RETAINED; untrustworthy `Err` (E4/§4.5) ⇒ handle RETIRED. Method
  **required — NO default implementation** (a default looping the old serial
  `cleanup()` is a prohibited compatibility shim). The serial `cleanup` trait
  method, `sidecar_cleanup`, and the Python single-`cleanup` ACTION are all
  REMOVED at cutover: every real caller, mock, fixture, warmup and bench
  migrates to `cleanup_batch` ([1] for a one-window request — the wire shape
  is always batch). No serial fallback path exists anywhere.
- **Client timeout mapping (CRITICAL, engine.rs `request_timeout`
  :53-60):** today's map is `download=900 / load=15 / cleanup=15 /
  check_update=10 / _ => 5`. The cutover MUST replace the `Some("cleanup")`
  arm with `Some("cleanup_batch") => 15` (10 s generation budget + protocol
  headroom) and delete the old arm — an UNMAPPED new action silently gets
  the **5 s default**, which would retire the sidecar mid-batch on the very
  first production run (every 16-batch takes 5.88–6.45 s measured; 7.98 s
  active19 total). `load=15` and the outer 30 s
  (`LLM_CLEANUP_TIMEOUT`/`kill_orphan`) stay unchanged. The 7.98 s real-path
  proof must run through the ACTUAL Rust client stack (spawn sidecar →
  load → chunks under the mapped 15 s), not only the Python harness.
- `is_responded_timeout` retention semantics apply per-batch exactly as today
  per-request: a valid batch response carrying timeouts RETAINS the handle;
  `cleanup_proposal`'s timeout→zombie escalation is not reintroduced.
- Callers after cutover: only `correction::plan_cleanup` (loop over chunks).

### 4.2 Sidecar generation internals

- Head cache: the frozen prompt's transcript-free public prefix (system +
  instruction, sentinel-split — token-common-prefix of two distinct-prompt
  encodings, exactly as measured at 365 tokens), materialized as
  `prefix_cache = make_prompt_cache(model)` advanced by a `max_tokens=0`
  pass over `head_ids` (proven recipe, run_all67.py:151-161).
  **Build ORDER inside the EXISTING `load` → `warm_model()` path:**
  weights resident → build `prefix_cache` + the empty detok template (§4.2
  detok bullet) → THEN the internal batch-warmup generation (which itself
  dispatches through the head) → mark warmed. Building after today's warmup
  work is WRONG — the warmup generation would need a missing head (or
  recurse). NO new protocol action. The `cleanup_batch` handler's
  ensure-warm check runs BEFORE the 10 s generation alarm arms; the old
  “in-request head rebuild under the alarm” fallback is REMOVED — cold
  model-startup cost belongs outside the generation timer and inside the
  caller's `load`/outer 30 s timers (which already cover it today).
  Consequence of load-time building: the measured cold-first-dispatch cost
  (11.58 s abandon at pss 64 / 17.67 s at 2048, `alarm10_cold.json`) never
  lands inside a batch alarm.
  Immutable: the batch merge COPIES the prefix into freshly allocated batch
  keys/values (left-padded `BatchKVCache`, mlx_lm/models/cache.py:1088-1118
  region), so `prefix_cache` is never mutated by any dispatch.
  **Prefix TOKEN-match miss (an item's full prompt ids don't start with
  `head_ids`):** dispatch that item with its FULL prompt and a FRESH cache
  (`caches=None`-equivalent for that row — the proven all67 shape: full
  ids + `make_prompt_cache(model)`), NOT raw/reject — the model judgment is
  unchanged; only the shared-prefix optimization is skipped for that row.
- Dispatch per group of `n` (1..=16), against the ACTUAL installed API
  (`mlx_lm/generate.py`, class `BatchGenerator`), EXACTLY the proven call
  shape of every qualifying run (run_all67.py:71-81):
  `BatchGenerator(model, sampler=sampler,
  stop_tokens=[[eos] for eos in tokenizer.eos_token_ids],
  prefill_step_size=64, prefill_batch_size=n, completion_batch_size=n)` —
  each EOS id is a stop SEQUENCE **`[eos]`** (a one-element list).
  `[list(eos) for eos in …]` is WRONG — the elements are INTS; `list(int)`
  raises `TypeError`. No constructor `max_tokens` override exists (no
  `BATCH_CAP`; per-prompt `insert(max_tokens=[…])` governs, as measured).
  Both batch sizes set EXPLICITLY to `n` (ctor computes
  `completion_batch_size = max(cbs, prefill_batch_size)`, prefill default 8 —
  leaving default silently runs completion at `max(n,8)`).
  Caches: `prefix_cache` is what
  `mlx_lm.models.cache.make_prompt_cache(model)` returns — a LIST of
  per-layer cache objects (the module-level factory lives in
  `mlx_lm/models/cache.py`; generate.py only consumes it via
  `_make_new_cache`). Insert takes `caches=List[List[Any]]` (per prompt, the
  per-layer list): head-matched items pass the SAME shared `prefix_cache`
  list — `[prefix_cache] * n` — never a nested `[[head]]` (double-wrapped
  list is a different, wrong type). Sharing is safe because the batch merge
  COPIES the prefix (`BatchKVCache`, mlx_lm/models/cache.py: left-padded
  batch keys/values) — the template object is never mutated by dispatch
  (proven: all67 ran 67/67 with one shared head).
- **Generation budget (frozen boundary, verbatim):** the per-prompt budget is
  the existing formula computed on the KEY TEXT, not on prompt/tail token
  counts: `budget_i = min(8192, max(128, 2*len(tokenizer.encode(text_i)) + 32))`
  (llm_cleanup.py:240 verbatim). The context-limit guard likewise keeps its
  current shape: `len(prompt_ids) + budget > context_limit` ⇒ reject — the
  FULL prompt (head included) + budget, never the tail alone. Larger budgets
  finishing at a normal EOS earlier proves nothing about the boundary; the
  boundary behavior itself is what is specified.
  `gen.insert(prompts=[tail_ids_i …], max_tokens=[budget_i …],
  caches=[prefix_cache if head-matched else fresh
  make_prompt_cache(model) per miss][…])` (miss handling per next bullet).
- **EOS-at-budget normalization (behavior, not a constant):** `BatchGenerator`
  marks a prompt `LENGTH` the instant its budget is consumed
  (generate.py:1840+), even when the final emitted token IS an EOS stop
  sequence (old `stream_generate` would have called that `stop`). Acceptance
  MUST normalize: a `LENGTH` item whose last token is an EOS stop id ⇒
  completed STOP (sealed `status:"ok"`); `length` ending on a NON-EOS token
  (true truncation) ⇒ sealed `failed` / `incomplete_generation` ⇒ never
  proposed (E3). Regression obligation: a real EOS-exactly-at-budget case
  earns this as a behavior test (observed accept/reject at the boundary),
  not a wiring/constant assertion.
- **Per-item detokenization (invariant, not a framework; VERIFIED
  2026-09-13, tokenizer-only probe):** each UID's output streams through its
  OWN stateful detokenizer — never one shared stateful instance across items,
  never re-decoding the whole token prefix per step. `tokenizer.detokenizer`
  is a property returning a FRESH `BPEStreamingDetokenizer` per access
  (tokenizer_utils.py:451, `_detokenizer_class(self)`; the `__getattr__`
  path instead yields the cached instance). Ctor cost = tokenmap over full
  vocab + byte decoder (class-level cached). **copy.copy(template) +
  `.reset()` per UID is SAFE (verified: `reset()` rebinds only
  `offset/_unflushed/text/tokens`; the copy SHARES `tokenmap` by identity;
  reset on a copy never mutates the held template; held-instance reuse after
  copies ⇒ exact).** Empirical: 2-way token-alternating Unicode interleave
  (accents/€/emoji vs German umlauts/€) ⇒ BOTH exact; `.text` never exposes
  a mid-multibyte partial (0/17 sampled states). Fresh-ctor vs reset-copy
  agree on `_maybe_trim_space` parity (both start `text=""`; no divergence
  observed). NO custom BPE decoder, no LUT reimplementation. **Per-UID
  output byte cap = the old streaming `MAX_TEXT_BYTES` 32 000 B rule, checked
  at EVERY segment AND after `finalize()` (:226 can still append).** Track
  incrementally — after each `add_token`,
  `running_bytes += len(detok.last_segment.encode("utf-8"))` — NEVER
  re-encode the whole accumulated output each step (avoids O(n²) work per
  item). Char-boundary-safe (add_token flushes only complete chars). When an
  item crosses the cap: REMOVE THAT UID from the generator
  (`BatchGenerator.remove([uid])` — verified API) and mark it `Failed
  ("text_limit")`; the OTHER items in the batch keep generating (per-item
  isolation, no batch-wide abort). Precedence: **deadline first; EOS-at-
  budget acceptance NEVER bypasses the byte cap** (an EOS-terminated reply
  over 32 000 B is still `text_limit`-failed, matching the old stream's
  output guard). Native partial non-EOS `length` stays rejected (E3). The
  qualification prototype decoded at final only — post-implementation a REAL
  behavior test must cover the per-segment limit hit (one UID removed
  mid-stream, siblings complete), finalize-append overrun, and Unicode/
  control interleave across segment boundaries (observed accept/reject +
  byte counts, not constant assertions).
- `prefill_step_size=64` (MEASURED: `alarm10b.json` — production-shaped
  alarm(10) over a 21.9 s batch workload fires at 10.02 s with pss 64 vs
  10.08 s at the 2048 default; steady-state ACTIVE-19 cost is noise-level
  7.98/8.07/8.09 s at 64/128/2048). Bounds the non-interruptible native
  prefill chunk (16×64 tokens vs 16×2048 at default); API knob, no new code.
- Greedy sampler identical to today; alarm: one `signal.setitimer` 10 s
  around the WHOLE batch request (same as today's per-request alarm semantics
  — the alarm bounds the IPC request, and the request is now ≤ 16 windows).

### 4.3 Planner: occurrence eligibility, dedup, no-work mask

`plan_windows` final-mode logic UNCHANGED (its `live` parameter + live-gate
config fields die with the removed phase-1 path — §6). New adapter-proven
predicate (validated at scale against the real planner via source-exact
includes; §10), shipped as the ONE `DeletionContext::work_mask` accessor
(§6):

```
possible[i] = (HES(i) ∪ repeat_member(i) ∪ restart_member(i)) ∧ ¬protected_hit(i)
```
Every term reuses `DeletionContext`'s own structures (frozen v6 fixture suite
green through the copied file) — no second tokenizer, no re-derivation.
Caps-opportunity scan: `expose=true` at each frozen sentence boundary; a
token with ASCII-lowercase first char while `expose` is a cap opportunity;
`expose &= possible[i]`. Window `w` is **active** iff its target contains any
`possible[i]` or cap-opportunity token; else skippable.

- **Scope:** the GLOBAL mask is a NECESSARY condition (eligibility proof for
  skipping), never a joint-legality proof. The final `DeletionContext`
  adjudication at composition remains the SOLE authority. The mask may only
  skip requests.
- **Occurrence eligibility (normative):** dedup key `k` is requested once iff
  ANY absolute window owning `k` is active; every ACTIVE occurrence keeps its
  own `target_authority` at its absolute offset and composes through the
  shared reply; occurrences of `k` with no active owner take identity
  (skipped) explicitly. **Never** dedup by first-occurrence eligibility —
  a later active copy of an identical interior key (e.g. one inside a long
  quoted span whose delimiters fall outside the ±12L/8R context window, one
  unquoted) must still get its legitimate edit. (Mixed-eligibility behavior
  live-drove through the real planner + authority + global guard —
  `mixed_dup_case_truekey.json`: the planner emitted two windows with
  BYTE-IDENTICAL keys (block start after sentence A), each covering the
  `…client um before…` token, one copy inside the quoted span, one outside;
  one shared reply (um deleted) composed to **quoted copy keeps `um`, live
  copy deletes it** (per-occurrence authority + global protection), status
  Applied. Earlier variants `mixed_dup_case(_keyed).json` differ only in key
  bytes. `LiteralGuard` counts the quoted value's occurrences source-wide
  (value not duplicated wholesale outside — verified count 1).
- **Complexity (normative):** O(tokens + windows) — reuse `DeletionContext`
  tokenization; ordered single cursor over boundaries; original targets
  partition `F`'s words (adapter-asserted), so activity per window is a
  linear `.any()` scan of its target range (or binary span bounds), never a
  materialised per-window vector. (The measurement adapter's
  `tw.collect`/repeated-find shapes are explicitly NOT the shipping shape.)
- **Ownership (cutover cleanup, taste — not a feature):** with the live cache
  gone, nothing requires `'static` owned key strings from the planner.
  `WindowRequest` borrows `&F` + key range where the existing thread boundary
  allows; dedup compares/keys on exact **borrowed** key bytes while each
  occurrence keeps its absolute owner position. Each query string is
  materialized **once** at the point `spawn_blocking`/IPC genuinely needs an
  owned copy (existing window `build_request` shape + chunk vectors) — never
  build-a-string-then-clone-again. No new `Arc`/interner/abstraction layer to
  shave tiny buffers; necessary owned IPC copies stay owned. No extra
  whole-`F` clones; work mask runs on `DeletionContext`'s single tokenization
  (no per-window re-tokenizing — see Complexity above).
- **Fail-safe:** any protection-analysis error ⇒ conservative **ALL windows
  active** (ask everything) or propagate the honest failure — never a silent
  no-work, never a panic, never a raw-skip caused by broken analysis.
- Counts reported separately (observability): active WINDOWS vs distinct
  active KEYS (on F: 19/19; the 6 duplicate keys each occur 2× with
  active-occurrence 0/2 — any-active path exercised trivially; contract is
  arbitrary-`F`).

### 4.4 Deadline and failure semantics (exact)

- No strict 10 s wall is promised: native work in flight is non-interruptible;
  the alarm fires at Python checkpoints — measured WARM in-generation
  FIRED_LAG ≤ 0.08 s at pss 64 (all interrupts inside generation at
  production window sizes, `alarm10b.json`); a COLD first dispatch in
  prefill/early generation abandoned at 11.58 s (lag 1.58 s; 17.67 s at
  pss 2048, `alarm10_cold.json`) — that is the OBSERVED cold cost (§4.2
  head-at-load avoids it), not a general bound. No hard-10 s wall guarantee
  exists in any state.
- **Completion policy:** a reply is valid only at its EOS (finish_reason
  `stop`). PARTIAL generations are NEVER accepted regardless of how many
  tokens exist (native continuous batching admits/exhausts windows at varying
  lengths, so completed windows CAN coexist with partials — acceptance is
  typed per item, not positional). Windows that reached `stop` BEFORE the
  deadline are retained and compose normally; anything completing after the
  deadline is discarded (the handler aborts the drain; results are sealed at
  the strike); unfinished items report `timeout`; their regions stay raw;
  other regions' edits still ship (existing per-window isolation).
- Batch-request timeout is a RESPONDED timeout (valid JSON with per-item
  statuses): handle retained (today's `is_responded_timeout` retention —
  the timeout→zombie escalation stays removed; a deadline miss ≠ dead pipe).
- Per-chunk: the 10 s alarm arms per batch request (≤ 16 windows each).
  Chunks beyond a strike still RUN (each gets its own deadline) — the whole
  cleanup may exceed 10 s across chunks exactly as the prior incremental path
  already does across serial windows; the honest bound is per-request, not
  per-`F` (§10 worst case).
- Sidecar kill happens at the EXISTING per-request call sites only:
  `sidecar_cleanup`'s outer `tokio::time::timeout(LLM_CLEANUP_TIMEOUT = 30 s)`
  expiry ⇒ `kill_orphan(state)` (cleanup.rs:119–140; the batched successor
  keeps the same outer-deadline + orphan-kill per CHUNK request), and
  `is_zombie_error` classification (engine.rs:425/809) ⇒ retire path. This
  30 s engine-kill is NOT a recording-cancel mechanism.
- **Cancellation contract (boring: PRESERVE existing behavior exactly — add
  nothing):** verified from source — `capture.rs` start requires
  `AppState::Idle` (capture.rs:244), the manager claims
  `Recording → Transcribing → CleaningUp`, so **a new recording CANNOT
  start while cleanup is running**; the stop-time cancel shortcut is
  unregistered at stop (manager.rs:403) — mid-recording cancel exists,
  mid-cleanup cancel does not. Stale-result guards: job ticket assigned
  manager.rs:445, outer `(job_id, recording_generation)` checks at 461/509
  discard an invalidated job’s result; the pipeline path guards by job only
  (pipeline.rs:105/148). `plan_cleanup` itself has NO staleness check — only
  `llm_operation.try_lock()` (busy ⇒ `Unavailable`, `F` preserved). The
  batch loop runs ALL chunks with no inner cancellation, same as the serial
  loop today; native ASR/in-flight batch work is not instantly cancellable.
  DECISIONS: no new job-token API, no per-chunk cancellation feature, no
  claim that “a new recording begins successfully during cleanup” (it is
  rejected until Idle). Ambient per-chunk `recording_generation` reads stay
  BANNED (would gate an OLD plan against a NEWER ticket — a flow the app
  never creates anyway). Faster, bounded cleanup shortens the window in
  which the app sits non-Idle — that is the user-visible win; scope stays
  boring.

### 4.5 Protocol framing and aggregate size budgets (batch migration contract)

The existing single-request line cap (`MAX_LINE_BYTES` = 256 KiB) cannot
carry a 16-text batch line, and an ASCII bench does not prove it can: JSON
escaping is ≤ 6× raw bytes per text (`\uXXXX` for control characters).
**Static bounds, chosen limits, one consistent contract:**
- Caps: per-item INPUT ≤ 4 096 B raw (planner `MAX_REQUEST_BYTES_SOFT`,
  enforced server-side per item as new work), per-item OUTPUT ≤ 32 000 B raw
  (**`MAX_TEXT_BYTES`** — the actual constant, llm_cleanup.py:23), ≤ 16 items.
  Worst-case request line ≤ 16 × (4 096 × 6 + ~8 B field overhead) + framing
  ≈ 0.393 MB; worst-case response line ≤ 16 × (32 000 × 6 + ~64 B metadata)
  + framing ≈ 3.08 MB. Therefore `MAX_REQUEST_LINE_BYTES = 1 MiB` and
  `MAX_RESPONSE_LINE_BYTES = 4 MiB` **statically dominate every valid
  planned batch** — 16 valid texts ALWAYS fit; there is no packer to write,
  no per-text `serde_json::to_string` pre-pass, and no reachable test for an
  unreachable packer. Chunks are plain count-of-16 over all active keys.
- Both limits are batch-framing constants shared Rust (`engine.rs`) /
  Python (`llm_cleanup.py`) applied by the single shared line codec after the
  clean cutover (old single-`cleanup` action is deleted, so no legacy 256 KiB
  cap survives anywhere; “keep MAX_LINE unchanged and call 16 arbitrary
  inputs valid” is explicitly NOT the contract). Enforcement is on buffered
  line readers (`read_until` growth / Python buffered readline) with an
  allocate-bounded guard: read up to limit+1 bytes, reject beyond it — no
  eager 1 MiB/4 MiB allocation per request.
- Fault typing (§4.1/§5 E4): over-limit or EOF-mid-line = untrustworthy
  stream ⇒ protocol-fault path (retire + kill + restart next use), `F`
  preserved, typed status; intact-line schema violation (duplicate/missing
  index etc.) ⇒ same retire path (a contract-breaking endpoint is not kept);
  valid response with per-item handled codes ⇒ retention per
  `is_responded_timeout`.

## 5. Edge Cases

| # | Edge case & contract |
|---|------|
| E1 | stop with ZERO output tokens ⇒ proposal `""` is a VALID reply (all-deletable window). Rust carries `BatchItem::Proposal(Option<String>)`: `Some("")` ≠ `None`; success is never tested by truthiness (Python: `if proposal:` forbidden). Empty deliverable NEVER reaches clipboard/paste (existing `!final_text.trim().is_empty()` guard, manager.rs ~:575 / pipeline.rs :192). **History per the EXISTING contract, fields unchanged, zero new behavior:** the row saves `text: ""`, `raw_text: Some(original ASR)` (`(final_text != raw_asr_text).then_some(raw_asr_text)` — manager.rs:553, pipeline.rs:171; `add_transcription` accepts empty text), `llm_applied: true`, `llm_cleanup_status: Applied` — the original is RETAINED via `raw_text`, NOT by suppressing the composed empty. (Adapter probe: f=“um um”, reply “” ⇒ Applied, output “” — keep-one is group-legality policy owned by the frozen suite.) |
| E2 | Duplicate key, mixed eligibility (§4.3): per-occurrence activity; shared reply; later active copy never hidden by first-owner dedup. Regression test in §7.5. |
| E3 | item sealed `failed` (length/abort: `incomplete_generation`) or `timeout`, or the response omits an index (protocol fault ⇒ E4 retire): item → `Failed`/`TimedOut`; region raw; handle retained on valid-response codes; never force-fed. |
| E4 | Malformed batch response (not JSON, bad schema, missing `results`, index set ≠ `0..n-1`, duplicate/missing index, count ≠ n): whole batch's regions raw, sidecar RETIRED via the existing protocol-fault path (kill + clear PID, restart next use) — an endpoint violating its response contract is not trusted to keep serving; `F` preserved, typed status, log once. Over-cap line / EOF mid-line: same retire path (§4.5). Distinct: a VALID response carrying per-item handled codes (sealed-deadline `timeout`, `text_limit`, …) retains the handle per `is_responded_timeout`. |
| E5 | 0-text batch: results `[]`, no inference. |
| E6 | Head cache is built at `load`→`warm_model()` BEFORE the batch-warmup generation (§4.2 build order) — the cold-first-dispatch cost (11.58 s abandon measured) never lands under a batch alarm. NO in-request head-rebuild fallback exists (removed from the design): the handler's ensure-warm check precedes the generation alarm; if warm genuinely failed, requests get today's load/`Unavailable` typed handling, not an alarm-killed rebuild. model-not-loaded: caller-level `Unavailable` as today; the batch path never treats “model is no-op / not resident” as a per-text failure. |
| E7 | >16 texts or an over-cap item arriving at the sidecar: contract violation (client bug) — typed `{"ok":false,"error_code":"invalid_request"}` on THAT request only, all its regions raw, handle RETAINED (the sidecar answered validly; nothing untrusted happened on the stream). No silent regrouping: enforcement, not accommodation — Rust chunking (§4.5) makes the case unreachable in correct operation. |
| E8 | Non-ASCII / emoji / CRLF in keys: bytes are opaque through the protocol (existing UTF-8 validation); no new assumption. |
| E9 | State during the batch-loop: a new recording start is REJECTED until `Idle` (capture.rs:244 — cleanup cannot be interrupted by, nor run concurrently with, a new recording; the stop-time cancel shortcut is unregistered at stop (manager.rs:403), so there is no user “cancel mid-cleanup” flow to design for, and none is claimed). The plan runs ALL chunks with no inner check (only `llm_operation.try_lock()` mutual exclusion; busy ⇒ `Unavailable`, `F` preserved — today's semantics). A job invalidated by any path (e.g. controlled invalidation in tests) has its RESULT discarded by the caller's outer `(job_id, generation)` guards (manager.rs:461/509; pipeline.rs:105/148 job-only) ⇒ no late paste, no stale UI, no history row for that discarded NORMAL result. **Cancel keeps its own contract unchanged:** a cancelled session SAVES history (`Transcription{cancelled:true, llm_applied:false}` pipeline_cancel.rs:303; empty placeholder :335) with zero paste — discard-of-stale ≠ deletion-of-cancelled-history. Ambient per-chunk generation reads BANNED (would gate an old plan against a newer ticket — a flow the app never creates). Native ASR/in-flight batch work is not instantly cancellable; no instant sidecar-kill on cancel (engine kill = 30 s outer timeout per chunk request only). PCM history unaffected. |
| E10 | `F` with NO words (empty/whitespace): zero windows ⇒ NoChanges, no dispatch. A 1-word `F` (e.g. “um”) is a normal one-window batch — its reply may be `""` (E1) and compose `F` to empty; the existing empty-output paste guard skips the paste, and history holds `text: ""` + `raw_text: Some(F)` per the unchanged E1 contract. “1-word ⇒ NoChanges” is WRONG and excluded. |

## 6. File Changes

| File | Op | Change |
|---|---|---|
| `src-tauri/sidecar/llm_cleanup.py` | modify | New `cleanup_batch` action (§4.1/§4.2): head cache built inside existing `load`/`warm_model()` (§4.2, no new action), actual-API `BatchGenerator` params — no ctor `max_tokens` (§4.2), frozen budget formula + EOS-at-budget normalization (§4.2), per-item results, alarm over the request, contract enforcement (0..=16 with 0 = early no-op before model work; per-item caps; `invalid_request`), `MAX_REQUEST_LINE_BYTES`/`MAX_RESPONSE_LINE_BYTES` framing (§4.5). **DELETE the single-`cleanup` action and `MAX_LINE_BYTES`** — after cutover there is one narrow batch-only line codec. |
| `src-tauri/sidecar/test_llm_cleanup.py` | modify | Migrate the old single-action tests to the batch action (1-text batches exercise the same generation paths). New: result-permutation contract, timeout sealing, malformed-JSON error shape, 0-text no-op BEFORE model work (assert generator never touched), `invalid_request` on >16/over-cap, line-limit enforcement via synthetic control-char worst cases (×6 escaping — NOT ASCII bench), per-prompt budget formula boundary at the KEY text, EOS-at-budget ⇒ STOP-normalized accepted vs non-EOS-at-budget ⇒ rejected (observed boundary behavior, §4.2), per-UID incremental 32 000 B cap (segment + finalize) ⇒ capped UID `remove`d + Failed, siblings COMPLETE (observed isolation), Unicode/control interleave (§4.2 detok invariant), `finish!=stop` partials marked and never proposed. |
| `src-tauri/src/llm/engine.rs` | modify | `LlmBackend::cleanup_batch` REQUIRED (no default); DELETE the serial `cleanup` trait method and every mock/fixture/warmup/bench caller (mocks implement `cleanup_batch`; 1-entry calls replace 1-window serial calls); `request_timeout` :53-60 — REPLACE `Some("cleanup") => 15` with `Some("cleanup_batch") => 15` (unmapped ⇒ 5 s default = first-run mid-batch retire, §4.1); batch line caps (§4.5, buffered bounded growth, constants mirrored in Python); retention/zombie classification (`is_responded_timeout`, `is_zombie_error`) UNCHANGED. |
| `src-tauri/src/llm/cleanup.rs` | modify | `sidecar_cleanup_batch` (outer per-chunk `LLM_CLEANUP_TIMEOUT` + `kill_orphan` structure preserved); count-of-16 chunking helper; DELETE `sidecar_cleanup` — no serial path survives. |
| `src-tauri/src/llm/correction.rs` | modify | `plan_cleanup`: occurrence-eligibility dedup + ≤16 chunk batch loop + mask gating (§4.3). **DELETE** incremental phase-1 replay, `CorrectionHandle`, driver/worker, cache types (`CacheKey`/`CacheEntry`/`gate_terms` map, `quiesce`) AND the cache-only helpers their callers die with: `unique_occurrence` (+its seam tests), `clip_uncovered` (replay-composition-only), the `live` parameter of `plan_windows`/`build_request` and the live-only config fields (`CorrectionConfig::{window, queue_cap, close_ctx_tokens}`) with their live-gate tests — no dead code, no belt-and-braces retention. `plan_windows` (final mode)/`build_request`/`WindowAuthority`/`target_authority`/`compose_runs`: UNCHANGED otherwise. |
| `src-tauri/src/llm/validation.rs` | modify | ONE narrow accessor: `DeletionContext::work_mask(text) -> Vec<bool>` (+ caps-opportunity exposure flag per token, or a returned pair) computed from the SAME structures `validate_inner` uses — the mask's H/R/protected definition is literally `validate_inner`'s candidate/protection union, no new regex policy, no five public getters exposing internals. NO policy change (frozen suite stays green untouched). |
| `src-tauri/src/audio/capture.rs` | modify | Remove collector wiring consumed only by the live driver; KEEP full-PCM retention for the authoritative finish path + cancel ordering (the collector's snapshot-publish side dies with its consumer). |
| `src-tauri/src/state.rs` | modify | Remove `incremental_correction` slot + lifecycle plumbing. |
| `src-tauri/src/hotkeys/manager.rs` | modify | Remove session-slot take/quiesce; stop/cancel ordering for capture + ASR unchanged otherwise. |
| `src-tauri/src/pipeline.rs` / `test_support.rs` | modify | Follow the deleted handle/slot (harness keeps calling `run_cleanup`, now cache-less). |
| `docs/journals/` | new entry | Implementation journal (append-only; old spec/journals never rewritten). |

## 7. Testing Strategy

Consumer-observable, plausible-bug-catching (contract regressions drive the
REAL planner/authority/global guard with controlled completed proposals — no
model in unit tests). Policy: tests assert state/behavior at boundaries,
never source text, wiring, constant values, or mock field copies; absence-
of-shape rules are one-time code-review items. Any EXISTING in-scope test
that pins wording/source/implementation is DELETED at cutover, not re-pinned.

1. **Wire contract:** parsed `results` permutation validated; `Some("")` vs
   `None` behavior (E1) through `plan_cleanup` to composed text + status.
2. **Batch protocol violations (E4):** duplicate/missing index → all batch
   regions raw, sidecar retired + restarted, `F` preserved; the NEXT
   chunk's batch composes normally on the fresh process (per-batch
   isolation).
2b. **Size budgets (§4.5):** static-bound tests, not packer tests: a valid
   16-text chunk of control-char-dense texts (worst ×6 escaping) round-trips
   under the 1 MiB request cap and its worst-case response under 4 MiB; a
   synthesized over-cap LINE (Rust→Python and Python→Rust) exercises the
   typed desync path: retire, `F` preserved, no partial-batch guess;
   >16-item request → typed `invalid_request`, handle retained (E7).
   ASCII-only fixtures must NOT be the bound evidence.
3. **Deadline sealing (E3/§4.4):** injected per-item timeout: completed-before
   items ship, partial/late items never do; status/log per-window outcomes.
4. **Mask fail-safe + coverage:** property test — for adversarial small `F`s,
   every token any validator rule could delete (H/R/CAP exposure) lands in an
   active window; analysis-error injection ⇒ all-active (never silent skip).
5. **Mixed-eligibility duplicate (E2, director-named):** two windows with
   IDENTICAL key bytes, one occurrence inside a long quoted span (delimiters
   outside ±12L/8R, value not fully duplicated elsewhere), one outside;
   controlled reply deleting the interior `um` ⇒ observable final text keeps
   the quoted copy's `um`, deletes the live copy's. Assert FINAL TEXT only.
   (Reference fixture validated against the real planner/authority/guard
   2026-09-13: `mixed_dup_case_truekey.json` — shared key `the room agreed
   … looked reaso[nable]` ×2, um-windows at absolute 189/602, quoted-keep +
   live-delete both held, status Applied.)
5b. **Lifecycle guards on the ACTUAL path (E9) — observable state-flow only:**
   (i) start rejected until `Idle` — drive `begin` through the ACTUAL
   capture backend while state is `CleaningUp` ⇒ refused, state unchanged;
   (ii) cancel while `Recording`, then a fresh run: the CANCELLED session
   keeps its contract history row — successful ASR ⇒
   `Transcription { cancelled: true, llm_applied: false, status: Idle }`
   saved (pipeline_cancel.rs:257–344, save at :303; too-short cancel saves
   the empty cancelled placeholder at :335) — assert the cancelled row
   RETAINED + the new session's own row, zero paste from the cancelled
   session, no cross-recording text leak. Cancel history policy is UNCHANGED
   by this spec; tests conform to it — never bend it to make a test pass;
   (iii) stale-ticket discard (distinct from (ii)): a NORMAL stop result
   whose caller ticket (job_id/generation) is control-invalidated after
   cleanup returns but before post-cleanup writes ⇒ dropped, NO paste and
   NO history write. Multi-chunk (>16 keys) covered by having the mocked
   sidecar observe the received batch count (behavior, not plumbing).
   Prohibited shapes (no ambient generation read in `plan_cleanup`, no
   per-chunk cancel API, no job-token plumbing) = ONE-TIME code review at
   implementation — never permanent source-grep/wiring tests.
6. **Existing suites green:** frozen v6 `validation.rs` fixture suite,
   `plan_cleanup` contract regressions (minus deleted handle/cache/live-gate
   cases), capture tests, migrated sidecar tests (single-action originals
   rewritten as 1-text batches, not kept).
7. **Runtime qualification (before release, §12):** paced production matrix
   (`sotto-replay` /tmp copied-crate harness, production modules via exact
   includes, real ASR + real batch sidecar) cold+warm across 30/60/120/180/
   300/450/600 s + v2 fixtures; the all-67 skip proof (§10 M4); g1..g10
   two-arm FINAL-vs-gold leg (serial vs batch on the SAME questions, scored
   ONLY on composed FINAL vs reviewed ledgers; every semantic loss reported
   and classified preexisting-vs-batch-regression; labels frozen — e.g. g5
   `The um option` stays Keep as reviewed; no regex/guard patches to hide
   model misses). **Scoring-rule caveat (VERIFIED against source-only gold):**
   per-index R scoring ALIASES across byte-identical `the`-class copies —
   phantom “unauthorized deletions” appear; score with GROUP-level counts +
   CAP-variant normalization (gold records allowed CAP variants;
   `score_F_ledger.py` implements the rule), never raw per-position matching.
   **Replay safety (grounded: `auto_paste=false` is NOT protection — the
   Copy branch still writes the real clipboard, pipeline.rs ~:229):** the
   replay spine routes through an observational `PasteBackend` test-sink
   (records text+action locally; zero NSPasteboard/CGEvent) + isolated
   history destination; physical paste is then UNVERIFIED and disclosed as
   such. Real ASR/LLM/capture/WAV stay real; only hardware-input/paste/
   history destinations are controlled. No user mic/speaker/clipboard/data
   writes in any qualification run.

## 8. Migration Plan

No schema, no settings migration. Feature-gate: none — clean cutover (the
batch arm degrades to `Failed` regions per window on any sidecar/protocol
problem; that IS today's per-window failure semantics, not a fallback to the
old path: retaining the old live-cache path as a hidden fallback would need a
separate explicit design and is NOT in this spec). Rollback = revert the
changeset. History/settings/PCM/source-authority behavior: byte-compatible
with the prior incremental path (app 0.9.0) by construction (unchanged modules).

## 9. Security Considerations

Local-only, unchanged: no network, no telemetry. Logs carry batch size, byte
counts, indices, elapsed ms, statuses — never transcript text. The head cache
is transcript-free by construction (public prefix) and process-memory only.
Batch lines are governed by the explicit aggregate budgets of §4.5
(`MAX_REQUEST_LINE_BYTES` 1 MiB / `MAX_RESPONSE_LINE_BYTES` 4 MiB, buffered
bounded growth, applied by the one shared line codec post-cutover — the old
single-action 256 KiB cap does not survive the clean cutover). The old flat
claim that 16 windows fit 256 KiB is FALSE under control-character escaping
and is superseded there.

## 10. Cost Analysis (honest, measured)

All numbers: MiniCPM5-2B-MLX revision 32f8dd5…, Apple M4, same tokenizer/
prompt; artifacts under `/tmp/experiments/sotto-correction/` (journalized).
| mechanism | workload | wall |
|---|---|---|
| serial (prior incremental path's stop lane, no cache) | 15 experimental windows | 29.0 s |
| serial production-shape | ACTIVE-19 of real F | 34.5 s |
| native batch, NO shared head | 15 win bs16 | 20.34 s |
| custom spec-decode K16 (retired arm) | 15 win | 7.32 s |
| **native head-batch, FINAL wire (16+3, pss 64)** | ACTIVE-19 of real F | **7.98 s** |
| **native head-batch, FINAL wire (16×4+3, pss 64)** | all 67 unique of real F | **26.41 s** (per-batch 5.88–6.45 s, every batch complete-before-10 s, 67/67 finish=stop) |
| prior incremental path (app 0.9.0) 5-min dense fixture | serial cached/uncached | 112.725 s / 169.417 s |

- Request volume on real F (844 words): 73 raw windows = 67 distinct keys;
  mask: **19 active windows / 19 distinct active keys** (29 possible + 4
  cap-opportunity tokens, zero coverage mismatch — every mask token inside an
  active target, asserted); duplicate split: all 6 dup keys active-occurrence
  0/2 (any-active rule must still be unit-tested — E2).
- **Skip proof (mask correctness, same-run replies — the director's method):**
  compose(ALL full-run replies) == compose(SAME-run active replies + identity
  ONLY on keys with no active owner): **byte-equal on F**; independent
  optimized-vs-full 19-run FINAL diffs: 0 on F (row recorded; batch-shape
  numerical variance is NOT conflated with mask soundness).
- Quality on F (**source-only reviewed gold, `/tmp/sotto-F-reviewed-gold.json`
  `F-source-gold-2026-09-13` — 33 positions (15 H + 14 R + 4 CAP), 29
  judgeable deletions, judged from SOURCE text alone, zero labels from model
  output; supersedes the earlier script-ceiling annotation; scored by
  `score_F_ledger.py`): recall **21/23** on both all67 and active19 arms —
  unauthorized 0, keep-one 6/6 legal; the sole 2 misses are H@312 + H@634
  (same “nothing else to add, um, for now” trailing shape, model-kept `um`,
  guard-admissible ⇒ judgment misses, reported, no 100 % cleanup claim);
  zero insert/replace anomalies, zero `no`-losses; caps
  (recomputed source-aware against `active19_composed_F.txt` AND
  `all67_composed_F.txt`, identical): **2 authorized flips** —
  `thanks`→`Thanks` and `someone`→`Someone` (source tok 430, context
  “top drawer Um someone must book” — the earlier “1 flip” row was a blind
  copy, corrected). The mask exposed 4 cap-opportunity tokens (235/430/451/
  773); the OTHER 2 (`confirm` at 451/773, contexts “invoice template 2 Um
  confirm the vendor”) were NOT CHANGED BY THE MODEL — the all-67-run
  replies contain no `Confirm` proposal at all (`all67_leg.json` grep);
  no causal claim is made about authority/validator behavior on them.
- LEG G held-out g1..g10 (serial vs N1-batch, composed FINAL vs frozen gold,
  span-anchored, `legG_composed.json`/`legG_ledgers.json`): finals
  byte-IDENTICAL both arms on all 10. **Preexisting frozen-model failures,
  named plainly (NOT zero-forbidden claims): g5 deleted the INTENDED-LITERAL
  word `um` in “She wants um the um option…”; g7 deleted intended-literal
  `hmm` in “…before the hmm deadline…” — the frozen guard does NOT protect
  these fillers as words (they were legitimate source-legal deletion
  candidates; gold says Keep), so these are full-word FALSE-POSITIVE
  deletions, more severe than missed cleanup; identical in the serial
  (prior-path) arm ⇒ known frozen-model judgment limitation, unchanged by
  this performance work.**
  g2: desired deletion GUARD-BLOCKED — candidate set EMPTY (all 11 tokens
  protection-masked; frozen policy genuinely prevents the um).
  g4: DIFFERENT class — the eligible final `um` (candidate H at tok 10) is
  NOT guard-blocked: adjudication-proven (controlled um-only reply composes
  **Applied**: “…remove this here.”). The model instead emitted an
  unsupported COMPOUND proposal deleting `um here` (word `here` is not a
  candidate) ⇒ guard correctly vetoed the whole proposal ⇒ eligible cleanup
  MISSED by model choice, not by policy. Both reported separately; labels
  frozen.
- **N16 mixed arm (bs16 numerics vs serial — CLOSED):** heldout g1..g10
  through the real N16 mixed batch: replies byte-equal to the serial arm
  10/10 (position-aware word-level) and 10/10 equal to the N1-batch arm;
  6/6 Unicode-control literals preserved; g5/g7 preexisting-identical in all
  arms. Caps-fidelity leg (15 reviewed caps_adversarial sources, real
  planner 1-window/case, ONE native batch n=15 pss64 1 389 ms 15/15 stop vs
  serial 14.3 s, real adapter compose, frozen labels): 14/15 serial==batch
  byte-equal — ALL 13 adversarial lowercase identities (`m`/`false`/`i`/
  `ms`/`iPhone`/`eBay`/`macOS`/`null`/`http`/…) kept untouched in BOTH arms
  (old capprop failures were the RETIRED deterministic lane — they do not
  transfer to the model); c12 `nobody`-flip miss in both (preexisting);
  c13 “Um, london…” the only divergence — batch `london`→`London` where
  serial keeps lowercase: a cap case where the batch reply is semantically
  better; classified cap, **0/15 new-batch literal issues, zero literal
  losses in either arm** (`capsfidelity_leg.json`).
- Honest tail: arbitrary adversarial `F` (filler-dense, zero reuse) ⇒ ~every
  window active ⇒ ceil(W/16) batches ≈ W/16 × ~6.5 s; the per-request 10 s
  alarm bounds EACH request, not the whole plan — identical in kind to the
  prior incremental path's per-window serial worst case, ~4.3× faster per
  window, and the honest
  alternative (silently dropping windows) is EXCLUDED by §1.
- RAM: one BatchGenerator group ≤ 16 × 4 KiB prompts + KV for the head (365
  tokens) + tails; no per-recording LLM cache survives (removed).

## 11. Implementation Tasks (ordered)

1. validation.rs ONE narrow `work_mask` accessor (shared with
   `validate_inner`'s structures; frozen suite green).
2. sidecar `cleanup_batch` + internals (head at warm, actual-API
   BatchGenerator params §4.2, sealing, contract enforcement) + python unit
   tests (old single-action tests migrated, not retained).
3. engine.rs REQUIRED trait `cleanup_batch` (no default); delete serial
   `cleanup` + migrate every mock/fixture/warmup/bench caller; `BatchItem`
   parsing; batch line caps.
4. cleanup.rs `sidecar_cleanup_batch`; delete `sidecar_cleanup` + all callers.
5. correction.rs: occurrence-eligibility dedup + chunk/loop + mask gating in
   `plan_cleanup`; delete handle/driver/worker/cache/quiesce (visibility
   patches where private items were copied into adapters are reverted to
   original visibility as items die).
6. capture.rs/state.rs/manager.rs/pipeline.rs cutover (slot + collector dead
   code removed; cancel/PCM ordering preserved).
7. Contract regressions §7.1–7.6 + full `cargo test`/`clippy` gates.
7b. **Rebuild ALL qualification tooling from the CHANGED source:**
   planner-adapter AND editguard-harness (both pin correction.rs line ranges
   + sha256 selfcheck — ranges shift with tasks 5–6) and the sotto-replay
   harness (its `sidecar_cleanup` seam becomes the batch seam), in the same
   cutover, BEFORE any §7.7/§12 matrix or ledger-score leg — otherwise
   plan/compose/cands measure the pre-change code.
   **Replay interface (v0.2-rev12 amendment, contract frozen with driver
   owner 2026-09-13 — replaces the old row vocabulary):** stdout carries
   EXACTLY ONE JSON object per completed run (NDJSON), logs/debug to stderr
   only; stdin accepts `{"run_id","fixture","terms"}` requests (cold = fresh
   process, one request then EOF — serve gracefully). Result fields:
   `run_id` (echoed), `mode` cold|warm, `ok`, lossless `f`/`final`
   (+`f_sha256`/`final_sha256` = sha256 of the UTF-8 bytes),
   `metrics{asr_init_ms,spawn_mode,stop_to_final_ms,asr_ms,cleanup_ms,
   status}`, `cands{n_h,n_r,n_s,n_cap,n_prot,items[]}` +
   `tokens{spans}` + `final_tokens{spans}` — spans = ordered
   `[start_byte,end_byte]` pairs from the FROZEN Rust `word_spans` (byte
   positions, strict-UTF-8 decodable; items carry
   `{kind,toks,group?,cid}`; `final_tokens` over the composed final, empty
   final ⇒ `spans:[]` which is VALID and scores — no Python-regex
   rescoring), `audio_proof` over the REAL capture channel at the FIXTURE's
   NATIVE rate (no implied 16 kHz, no hidden resampling — `wav.rs` writes
   float32 mono at the provided rate, duration is the integer floor
   `n*1000/rate` (`captured_duration_ms`), and the writer appends
   `floor(rate*750/1000)` zero samples of trailing silence which are NOT
   captured duration): fields `capture_sample_rate:int,
   expected_samples:int, captured_samples:int, source_frames:int (TTS input
   frames), pcm_ms:int, pad_samples:int (= floor(rate*750/1000)),
   pad_all_zero:bool, resampling:{used:false} (used:true + from_rate +
   to_rate + input_sha256 ONLY if the harness pre-resamples — prefer the
   original fixture rate through `sample_rate()`; disclose + hash, never
   hide), order:"mono-sequential", head_sha256 (sha256 over the first
   captured PCM frames, payload bytes as captured, proving WAV head ==
   capture buffer), tail_sha256 (last 1024 captured samples, padding
   excluded)`. Driver validates expected==captured, pcm_ms==floor
   (captured*1000/rate), pad arithmetic, all-zero padding, provenance
   completeness; never coerces counts. `fake:false`. Metrics like
   `stop_to_final_ms` stay off the writer padding (captured-duration
   semantics). Per-run `ok:false`+`err`
   preferred over abort; nonzero exit if any run errored after serving
   others. No settle-timeout or line-flattening parsing on either side.
   Any driver-side change re-smokes against the MOCK binary first (the mock
   previously caught stdin-write + row-split bugs) before any real GPU leg.
   **Measurement honesty (rev15, Director 2026-09-13, binding on the replay
   binary):** each result carries `llm_proof{resident_before_stop:bool,
   pid_alive_before_stop:bool}` — the ACTUAL observed sidecar handle/PID
   state sampled immediately BEFORE the stop event (not a mode flag:
   a `mode:"warm"` row whose LLM was not resident before stop is a binary
   bug, not a cold run). Warm lane: the driver completes load + head-cache +
   warmup and the binary PROVES residency before the first capture starts;
   a warm row can never silently degrade to cold. Cold lane: the LLM process
   and model are verifiably NOT loaded at stop, and its spawn+load+head+warm
   cost is INCLUDED inside `stop_to_final_ms`. ASR readiness precedes
   capture in BOTH lanes; `asr_init_ms` records that initialization
   separately (no FS-cache-cold claims, no cache purges). Capture
   start/stop timestamps come from Rust monotonic clocks;
   `stop_to_final_ms` spans the REAL pipeline result/sink — finish/drain +
   WAV→full ASR→cleanup→delivered text — never precomputed `F` nor summed
   separate legs. The physical paste is EXCLUDED and `sink` states
   explicitly what was measured (e.g. `clipboard|mock`).

8. Runtime qualification §7.7 (paced matrix vs v2 post-ASR fgold —
   `v2_fgold_postasr.json` supersedes script-ceiling gold; g1..g10 two-arm
   FINAL ledger DONE (§10); N16 mixed + caps-fidelity arms DONE (§10,
   0 new-batch issues); any batch regression → fix-forward, never
   guard-patch; preexisting failures are LEDGERED, never gated to zero).
9. Journal entry + this spec's §12 update (never silent).

## 12. Known Limitations and Open Qualification

- Draft-state gates: **F semantic-recall annotation CLOSED** — source-only
  reviewed gold `F-source-gold-2026-09-13` (`/tmp/sotto-F-reviewed-gold.json`,
  29 judgeable deletions judged from source alone, no model-output labels);
  ledger reconciles to **21/23** both arms, unauthorized 0, keep-one 6/6,
  sole misses H@312/H@634 (§10 row quotes the ledger file). RUNTIME
  QUALIFICATION (paced matrix + final-vs-gold scoring) NOW CLOSED — scoped
  to the 32 admitted rows + native-inference evidence set ONLY: paced cold/
  warm matrix rows DONE — 26/26 matrix + 6/6 extension
  rows admitted, warm==cold identity 16/16 (journal §17); v2 post-ASR fgold
  scoring pass DONE — actual-Rust finals scored vs SHA-bound golds with exact
  per-occurrence byte spans, tradeoff disclosed (+2H@30s, −2H@180s,
  +4/−1 net +3H@600s vs original_serial), false-positives/keep-one/
  semantic-fail zero on all 12 rows, 12/12 value-parity controls
  (journal §18). F alone already proves:
  skip-soundness, wire timings, alarm behavior, residual-miss honesty
  (21/23).
- **Known frozen-model false positives (named, not hidden): g5-type and
  g7-type deletions of INTENDED-LITERAL words (`um` after “She wants”, `hmm`
  before “deadline”) reach the composed FINAL — note precisely: the frozen
  guard does NOT protect these words (they are legitimate source-legal
  deletion candidates; only gold says Keep), which is exactly why the
  old source-legal false positives pass validation. They occurred in the
  prior incremental path's (app 0.9.0) serial lane identically. Performance
  work changes execution, not judgment
  policy: no new semantic-regression gate beyond the complete preexisting
  failure ledger (§10 LEG G row), no regex/gold patches, no raw-fast
  substitutes. The final release report to the user MUST name these two
  literal cases.**
- Batch-vs-serial near-tie perturbations are MEASURED as possible (one 15-win
  bs16-vs-bs1 text divergence observed; adjudicated inert by authority on
  that case). Replies are advisory inputs to authority, but **source-legal
  wrong literals/caps CAN pass the guard** (g5/g7 above) — safety evidence is
  the semantic ledger, not a guard guarantee.
- Alarm(10) per-batch at pss 64 bounds a single dispatch; whole-plan time is
  unbounded by design (coverage priority, prior spec v0.8 D8 lineage).
- The batch path is one sidecar process; no parallel sidecars (single ANE
  contention would defeat the point).
- **Release-run acceptance (v0.9.1 local install, 2026-09-13): bundled
  release smoke = 14/15 — OUTSTANDING, contract NOT fully green.** The
  `reported_sentence` case must match its gold EXACTLY (README line 32; the
  runner's pre-existing exact-only rule for this case): the recorded sidecar
  reply removed all fillers plus `yeah,`/`those` and kept `the`; the shipped
  validator reconstructed a 20-word text (raw minus the two earliest `um`s)
  ≠ 16-word gold; no unauthorized survivor, no protected-payload loss.
  Classification of the reply-vs-gold guard interaction is left open (NOT
  asserted as g5/g7), and the cause of the reply difference vs the 0.8.3
  baseline (whose sidecar AND validator both differ) is UNKNOWN. Evidence +
  exact bytes: `benchmarks/llm/results/2026-09-13-release-evidence/`
  (bundled-smoke-091.json, reported-sentence-comparison.json, MANIFEST
  sections "reported_sentence"/"Criterion locations"). No contract
  weakening, no gold change, no input special-casing was applied to
  turn this green.
- **Clippy lanes:** the scripted repo gate
  `cargo clippy --all-targets -- -D warnings` (default lane) passes; the
  alternate supported single-backend lane
  (`--no-default-features --features custom-protocol,asr-parakeet,llm-cleanup`)
  passes. The literal `--all-features` lane is NOT required by any repo file
  and FAILS rc=101: enabling both mutually-exclusive ASR backends makes the
  masked-off parakeet helpers dead code (6 pre-existing symbols, none touched
  by 0.9.1) under `-D warnings`. Documented, not silenced; no
  source/gate change made to hide it.

## 13. Revision Notes

- v0.1 2026-09-13: rewritten from `local://correction-spec-2026-09-13.md`
  draft incorporating review pass 1 (director): wire contract fixed
  ≤16/IPC+Rust loop (67 ⇒ 5 chunks — the draft's "4 batches" was
  single-dispatch shape, superseded by measured 16×4+3); occurrence-based
  eligibility with per-window authority (first-owner dedup banned); mask
  proof = same-run subumption, independent-run comparison = separate
  semantic-diff row; NoChanges ⇔ 0 active WINDOWS (caps included); partial
  acceptance = EOS-typed per item (lockstep claim deleted); advisory-reply
  safety = gate evidence, not guard guarantee; live-cache removal decided
  (clean cut, release gated on matrix; old path ≠ hidden fallback);
  E5/classifier/custom-spec arms deleted from normative text (research
  evidence only); pss=64 settled by alarm10b measurement; repo-authoritative
  file per spec-workflow (this file; local draft superseded by pointer).
  Measurement journal: `MEMO-latency-density.md` + `/tmp/experiments/
  sotto-correction/*.json`.

- v0.1-rev1 2026-09-13: review pass 1 technical items closed: B2/B3
  corrected (169.417 s uncached / 112.725 s cached; exact user quote restored
  — >1 min post-stop wait, ~3 min RECORDING duration, never a latency bound;
  B3 scoped to its single fixture); wire ENFORCES 1..=16 + per-item caps
  (typed `invalid_request`, no regroup-accommodation); trait `cleanup_batch`
  REQUIRED, serial `cleanup`/`sidecar_cleanup`/single-action deleted with ALL
  callers/mocks/fixtures (no default-shim, no serial fallback); untrustworthy
  responses (schema violations included) ⇒ protocol-fault retire,
  responded-timeouts ⇒ retention; kill call sites named (cleanup.rs outer
  timeout `kill_orphan`, `is_zombie_error`), no instant-kill-on-cancel
  promise; §4.2 rewritten against the ACTUAL `BatchGenerator` API
  (`prefill_batch_size=n` explicitly — ctor's `max(cbs, prefill_bs=8)`
  corrected; `stop_tokens` = full `eos_token_ids` set; per-prompt
  `max_tokens` list); §4.5 static-bound budgets (1 MiB/4 MiB provably
  dominate 16×4 096×6 / 16×32 000×6; packer deleted; buffered bounded read,
  no eager allocation; batch-only codec after cutover); E10 fixed (1-word
  “um” = one batch, not NoChanges); cache-only helpers (`unique_occurrence`,
  `clip_uncovered`, `live` param, live config fields) deleted with their
  tests — no dead code; ONE `work_mask` accessor replaces five getters;
  caps row recomputed (2 flips, `thanks`+`someone`; old 1-flip row was a
  blind copy); LEG G composed FINAL ledger in (§10): 10/10 finals
  byte-identical serial-vs-batch; g5/g7 preexisting protected-word false
  positives named as known frozen-model limitations (not “same species” as
  missed cleanup — more severe); g2 guard-blocked (cand=∅, frozen policy
  genuinely prevents) vs g4: model emitted an unsupported COMPOUND proposal
  (deleted non-candidate `here`) → guard vetoed the whole proposal, while a
  controlled um-only reply adjudication-proves **Applied** ⇒ g4's eligible
  cleanup was missed by MODEL choice, not policy — classes kept distinct;
  v2 post-ASR fgold supersedes script-ceiling gold for §7.7 scoring.
  Source code NOT approved — implementation held until pass 3.

- v0.2 2026-09-13: P1 clarity close-out — all “shipped v0.8” labels
  disambiguated: the prior incremental path belongs to the INSTALLED app
  0.9.0 (`package.json`/`tauri.conf.json`); “spec Version 0.8” is the prior
  document's version number, never an app release (the replay-log crate v0.8.4
  predates version bumping and is not evidence 0.8.4 public builds shipped the
  incremental path); wording standardized to “prior incremental path
  (app 0.9.0)” / “prior spec v0.8 (document)”. g4 class fixed with fresh
  adjudication evidence (um-only reply ⇒ Applied): model-unsupported-compound
  (guard veto correct, eligible cleanup missed by model choice) — NOT

  frozen-policy guard-blocking (that stays g2-only, cand=∅). §10 table row
  for the prior-path fixture restored to its measured values after a
  mis-anchored edit. No new measurements or model claims introduced.

- v0.2-rev1 2026-09-13: **N16 mixed arm CLOSED** (§10 bullet, §11/§12 gates
  updated): heldout replies byte-equal serial↔N16 batch 10/10 + 6/6 Unicode
  controls; caps-fidelity leg 15 sources — 14/15 byte-equal, 13/13
  adversarial lowercase identities preserved both arms, 0/15 new-batch
  literal issues, one cap-classified divergence (c13 `london`→`London`,
  batch semantically better); remaining release gates: paced cold/warm
  matrix, F hand annotation, v2 fgold scoring pass.

- v0.2-rev2 2026-09-13: P2 finding — E9's "existing job-id staleness check
  in plan_cleanup" was FALSE (read actual plan_cleanup 853–1133: only
  `llm_operation.try_lock()`). Rewrote §4.4 + E9 to the verified caller flow
  (manager.rs ticket (job_id, generation), pre-check :461, post-cleanup
  discard :509–512); explicitly PRESERVES outer result-discard / no inner
  cancel (no new API, no validation smuggled in); ambient per-chunk
  generation reads banned (new-session bump would abort an OLD plan under
  the wrong ticket identity); explicit ticket-threading named as the only
  route to real inner cancellation, out of scope here. §11/§7.5b test added:
  real cancel/new-recording path, multi-chunk, asserts old plan completes +
  result discarded + no late paste. Engine kill on the 30 s outer timeout is
  documented as NOT a cancel-ASR mechanism.

- v0.2-rev3 2026-09-13: P2 lifecycle finding resolved FROM ACTUAL SOURCE and
  scoped boring: capture.rs:244 requires Idle to start ⇒ new recordings
  CANNOT begin during cleanup (earlier "new recording bumps generation
  mid-loop" scenario was an invented flow — deleted); stop-time cancel
  shortcut unregistered at stop (manager.rs:403) ⇒ no mid-cleanup cancel
  flow exists. §4.4/E9/§7.5b rewritten: preserve existing Idle-only start +
  outer result-discard guards (manager 461/509 job+generation; pipeline
  105/148 job-only); NO job-token API, NO per-chunk cancellation feature;
  tests are observable state-flow only (rejected-start via real capture
  backend, cancel-while-Recording + clean subsequent run, controlled stale
  invalidation before post-cleanup writes); ABSENT shapes = one-time code
  review, never permanent source/wiring tests (§7 policy; existing
  wording/source-pinning tests in scope are deleted at cutover, not
  re-pinned). Native ASR/in-flight batch not instantly cancellable; faster
  bounded cleanup shrinks the non-Idle window — the user-visible win.

- v0.2-rev4 2026-09-13: cancel-history semantics fixed against
  pipeline_cancel.rs:257–344: a cancelled session SAVES its history row
  (`Transcription{cancelled:true, llm_applied:false}` :303 / empty
  placeholder :335) with zero paste — earlier “no paste/history from the
  cancelled session” wording was WRONG; §7.5b(ii) now asserts the cancelled
  row RETAINED + new session row + zero cancelled-paste + no cross-recording
  leak; E9 states discard-of-stale applies only to NORMAL results; cancel
  history policy is untouched by this spec (tests conform to it, never the
  reverse).

- v0.2-rev5 2026-09-13: §4.3 Ownership bullet added (director taste, cutover
  cleanup): final-only planner drops `'static` live-cache key ownership —
  borrow `&F`/key-range through dedup (exact borrowed key bytes, absolute
  owner positions kept), materialize each query string ONCE at the
  spawn_blocking/IPC boundary, no Arc/interner frameworks for tiny buffers,
  no extra whole-`F` clones, no per-window re-tokenization.


- v0.2-rev6 2026-09-13: PASS-2 findings (PASS2-findings.md) applied:
  F2 head cache moved INTO existing `load`/`warm_model()` (no new action,
  no warmup recursion; cold-abandon 11.58 s cited as the OBSERVED reason,
  §4.4 lag claim scoped to warm, E6 rewritten honestly);
  F3 ctor `max_tokens=BATCH_CAP` deleted — proven call shape is per-prompt
  insert budgets only;
  F4 E1/E10 corrected to the ACTUAL history contract (manager.rs:553 /
  pipeline.rs:171 `raw_text = (final != raw).then_some(raw)`; empty-composed
  row = text "" + raw_text Some(original) + llm_applied true; no paste per
  the emptiness guard; NO new history behavior);
  F5 constant named correctly `MAX_TEXT_BYTES = 32_000` (no
  `MAX_CLEANUP_BYTES`), input cap named `MAX_REQUEST_BYTES_SOFT = 4096`;
  F6 validity = 0..=16 with 0 handled BEFORE model work;
  F7 task 7b: rebuild planner-adapter (build.rs pinned ranges/sha256) +
  sotto-replay batch seam from CHANGED source before any §7.7 run;
  F8: N16/caps-fidelity already closed (§10); F hand-annotation + v2 fgold
  pass remain open honestly. Director addition: budget formula FROZEN at the
  llm_cleanup.py:240 shape computed on the KEY TEXT (context check still
  full-prompt+budget); BatchGenerator's LENGTH flag at budget NORMALIZED to
  STOP when the sequence ends in a complete EOS (matches stream_generate),
  non-EOS-at-budget stays rejected — real-boundary behavior test, no
  constant/wiring assertions. Replay paste-sink safety folded into §7.7
  (auto_paste=false still touches the clipboard — observational sink +
  isolated history; physical paste UNVERIFIED disclosed).

- v0.2-rev7 2026-09-13: §4.2 per-item detokenization invariant (no shared
  stateful detok, no prefix re-decode; fresh native factory or copy+reset
  template — BPE reset :179-183 verified; no custom decoder/LUT; per-UID
  32 000 B cap after EOS finalize + `detok.text`; EOS-at-budget accepted
  before length classification, deadline precedence; post-implementation
  early-limit/Unicode-interleave behavior test required — prototype decoded
  final-only).

- v0.2-rev8 2026-09-13: §4.2 detok bullet upgraded from candidate-choices to
  VERIFIED facts via peer tokenizer-only probe (no weights, 1.1 s): property
  :451 ⇒ fresh BPEStreamingDetokenizer per access; copy.copy + reset() SAFE
  (tokenmap shared by identity, reset rebinds only the four mutable buffers,
  held template never mutated, reuse-after-copies exact); 2-way Unicode
  interleave both exact; `.text` never mid-multibyte; trim-space parity
  holds. Cap refinement recorded: mid-stream byte cap char-boundary-safe but
  finalize() (:226) can append ⇒ cap enforced after finalize as well.

- v0.2-rev9 2026-09-13: F semantic-recall gate CLOSED by source-only gold
  (`F-source-gold-2026-09-13`; 33 positions, 29 judgeable, zero model-output
  labels, supersedes script-ceiling annotation). §10 recall row now quotes
  `score_F_ledger.py` ledger: 21/23 both arms, unauthorized 0, keep-one 6/6,
  misses H@312/H@634 (same trailing “nothing else to add, um, for now”
  shape). §7 item 7 gains the VERIFIED scoring caveat: per-index R scoring
  aliases on byte-identical `the` copies (phantom unauthorized deletions) ⇒
  group-level counts + CAP-variant normalization only.

- v0.2-rev10 2026-09-13: task 7b extended per peer note — editguard-harness
  added to the pinned-rebuild set (same range+sha256 scheme) and the replay
  stdout ROW-VOCABULARY contract recorded (driver consumes
  REPLAY_INIT/ASR/CANDS/FGOLD/FINAL only; vocabulary change ⇒ re-smoke
  against mock binary first).

- v0.2-rev11 2026-09-13: PASS-3 director corrections applied (6 concrete
  fixes, no outline churn): (1) §4.2 code fixed — `stop_tokens=[[eos] for
  eos in tokenizer.eos_token_ids]` (`list(int)` ⇒ TypeError was wrong);
  `prefix_cache` = `make_prompt_cache(model)` per-layer LIST (factory lives
  in mlx_lm/models/cache.py), `caches=[prefix_cache]*n` shared-list —
  `[[head]]` nesting removed; insert param verified `List[List[Any]]`.
  (2) Build ORDER fixed: weights → head + detok template → batch-warmup
  generation → warmed; handler ensure-warm BEFORE alarm; in-request
  head-rebuild fallback REMOVED (E6 rewritten); prefix-token miss ⇒ SAME
  full prompt + fresh cache (proven all67 shape), never raw/reject, head
  never mutated. (3) Per-UID byte cap at EVERY segment + after finalize via
  `running_bytes += len(last_segment.encode())` (no whole-output re-encode);
  capped UID removed via `BatchGenerator.remove([uid])` → Failed
  (text_limit), siblings continue; deadline first; EOS-at-budget cannot
  bypass cap. (4) Trait keeps the request-level error channel:
  `cleanup_batch(...) -> Result<Vec<BatchItem>, String>`; per-item Failed ≠
  whole-Err; typed invalid_request retained vs untrustworthy Err retired,
  explicit. (5) engine.rs `request_timeout` :53-60 mapped:
  `cleanup_batch => 15` replaces `cleanup => 15` (unmapped action would
  silently get the 5 s DEFAULT and retire mid-batch on first production
  run); load 15 / outer 30 unchanged; 7.98 s path must be proven through
  the ACTUAL Rust client. (6) “protected um/hmm” was the wrong technical
  term — §10/§12 now say INTENDED-LITERAL words the frozen guard does NOT
  protect (source-legal candidates; gold says Keep) — that is precisely why
  these false positives pass validation. Historical rev-notes keep their
  original wording (append-only).

- v0.2-rev12 2026-09-13 (note restored 2026-09-13; body lost to an earlier
  anchored edit — §11-7b header carries the contract): replay interface
  re-frozen with the driver owner as NDJSON (exactly one JSON object per
  completed run on stdout, stderr-only logs, EOF-graceful serving,
  `run_id` echo, lossless f/final + sha256, metrics/cands/tokens/audio_proof
  fields, `ok:false`+err over abort) replacing the rev10 row vocabulary;
  driver re-smokes against the mock binary before any real leg.

- v0.2-rev13 2026-09-13: pre-delivery contract fixes from the driver owner
  + director (implementation-phase, no outline churn): (a) `final_tokens
  {spans}` added — composed-final word_spans, empty final ⇒ spans=[] VALID
  and scores (Unicode item-4/6 lossless alignment, no Python regex); (b)
  `audio_proof` REWRITTEN — the rev12 sketch (`samples,pcm_ms,order,
  tail_sha256` @16k mono int16) contradicted the real pipeline: capture is
  float32 mono at the DEVICE rate (48 kHz observed), `wav.rs` retains the
  provided rate and appends `floor(rate*750/1000)` silent samples excluded
  from `captured_duration_ms`'s integer floor. Fields now carry
  capture_sample_rate + real/expected counts + floor_ms equality + first-N
  float-hash equality + padding-excluded tail hash; no implied 16 kHz or
  hidden resampling. Throwaway replay-proof interface only — no production
  telemetry added.

- v0.2-rev14 2026-09-13: audio_proof field list LOCKED verbatim with the
  driver owner (rev13's own `first_n_sha256`/`floor_ms` names superseded by
  `head_sha256` + `pcm_ms` + `source_frames` + `pad_samples` +
  `pad_all_zero` + explicit `resampling` disclosure — driver's 32 negative
  controls already green on this shape). No design change; naming + field
  ownership frozen so binary and driver cannot drift.

- v0.2-rev15 2026-09-13: replay-binary measurement honesty (Director):
  `llm_proof{resident_before_stop,pid_alive_before_stop}` from ACTUAL
  pre-stop observation (never inferred from `mode`), warm rows cannot
  silently run cold, cold rows include load+head+warm inside
  `stop_to_final_ms`, ASR-ready-before-capture both lanes with
  `asr_init_ms` separate, Rust monotonic capture timestamps, real
  pipeline-to-sink spans (no precomputed F / summed legs), physical paste
  excluded + explicit `sink`. Interface additions only; no semantics
  change to §4.

- v0.2-rev16 2026-09-13: sidecar source review (Director) — five sealing /
  identity defects fixed in `run_batch_generation` + `warm_model` +
  `build_head`: (1) `results` preallocates `None` for every input BEFORE the
  timer/preprocessing; a deadline sweep seals every unsealed slot (timeout
  when sealed/overdue, else `incomplete_generation`), so alarm-during-prep
  yields a complete all-timeout response, never a short one, and
  `warm_model` can never loop empty results (asserts count==2 && all ok).
  (2) seal-before-delete: completion seals `results[index]` first, `del
  slots[uid]` after (mid-strip alarm keeps the seal; completed uids need no
  `generator.remove` — already closed). (3) deadline precedence enforced by
  monotonic `_over_deadline` rechecks: loop-top checkpoint AND inside
  `stop_item_result` AFTER finalize/last-segment byte count, immediately
  before the success seal — native work deferring the Python alarm cannot
  smuggle a late stop in as `ok`. (4) cap failures seal the index BEFORE the
  potentially interruptible `remove`. (5) `HEAD_PREFIX_TOKENS` production
  guard REMOVED — head length 365 is now a measurement pinned test-side
  (computed common prefix is the identity mechanism; per-row head match
  remains). §4.1 response block rewritten to the shipped status-tagged
  union; the old per-item `ok`/`finish_reason` sketch was illustrative only
  and is superseded (no aliases, no dual parsing anywhere). Dead
  `validate_text` removed; byte-cap test migrated to
  `validate_batch_text`. Three behavior regressions added (alarm during
  preprocessing, alarm during EOS finalize with sibling seal intact,
  deferred-clock stop demoted without any Python alarm) — 34/34 green.

**rev17 — §4.3 mask re-arm bug (Director-reported, Rust-side).** The
`work_mask` caps-exposure cursor was built with
`boundaries.iter().copied().find(|&b| b > 0)` — an `Option`, so its
`into_iter()` fed at most ONE boundary to the peekable cursor: after the
first sentence boundary the cursor was permanently empty, `expose` never
re-armed, and every later sentence's cap opportunity read as zero work. A
caps-only `F` with no hesitation/repeat/restart anywhere (`"Alpha beta.
Gamma delta. epsilon zeta."`) masked ALL windows inactive: zero dispatch,
NoChanges, silently lost edit. Fix: `.filter(|&b| b > 0)` (all boundaries).
The bug was invisible on hesitation-heavy inputs (fillers arm windows
anyway) and in the harness lanes, whose fixtures all carry deletions.
Regression is behavioral end-to-end through the real planner + frozen
authority, asserting the exact delivered text (`... Epsilon zeta.`),
status `Applied`, and frozen-validator admissibility — no source-text or
mask-internals assertions. `work_mask`'s doc now states explicitly that
membership is a per-token OVER-APPROXIMATION of joint legality (a marked
window can still earn zero legal edits under joint re-adjudication); only
the absence direction proves skippability.

**rev18 — 2026-09-13: runtime qualification closed (Director-admitted
evidence set).** §12 pending items resolved by the paced GPU matrix (26+6
rows, warm/cold identity, headline stop→final timings; journal §17), the
faithful same-Rust paired recomposition (9 arms through actual
compose/adjudicate; Python-gated recompositions stay UNADMITTED) and the
corrected final-vs-gold scoring pass with occurrence-level byte spans
(naming: `matrix_native16` = matrix warm final = production bs16, never
"legacy"; tradeoff disclosed, no "serial only loses" framing; journal §18).
Acceleration profile closed neutral-or-worse with bs16/pss64 retained; no
production change from profiling; report's stitched-union passages are
appendix-scope heuristics, excluded from source-coverage claims (journal §19).
Cancel orchestration smoke (mocked backends, labeled; real gates :434/:482
crossed with log-observer proof; stuck-state OBSERVED + labeled test reset)
and the controlled duplicate-owner-omission mutation (plausible bug shape,
NOT the historical mechanism; historical RED construction not independently
verifiable) documented in journal §20 with artifact hashes. No pattern-kills;
frozen-model g5/g7 false positives remain disclosed as above.

**rev19 — 2026-09-13: release-run acceptance recorded (v0.9.1 local).**
"RELEASE GATES NOW CLOSED" in §12 was over-broad and is RESCOPED: it covered
the runtime-qualification evidence set (32 matrix rows + native scoring),
NOT the install-time bundled smoke. §12 now carries two new bullets: bundled
release smoke on the installed 0.9.1 app = **14/15, outstanding failing
acceptance** (`reported_sentence` exact-gold rule; byte facts and CPU
whole-text diagnostic with explicit non-`plan_cleanup` scope in the release
evidence MANIFEST; model-reply cause vs 0.8.3 left UNKNOWN — both sidecar
and validator changed between those runs), and the clippy-lane record
(default + parakeet lanes clean; literal `--all-features` unsupported by the
mutually-exclusive backend design, failing on 6 pre-existing dead symbols,
not required by any repo file). Local-only bundle: signed, notarization
skipped, no updater artifacts (CLI override; `tauri.conf.json` unchanged).
