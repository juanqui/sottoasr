//! Bounded-window cleanup planner (stop-path only).
//!
//! Every cleanup — every caller — runs as bounded windows (≤ `max_target_words`
//! target tokens + context) dispatched as head-batched sidecar requests:
//! active keys are deduplicated and chunked to ≤16 texts per request (spec
//! docs/specs/2026-09-13-batched-stop-path-correction.md §4.3–§4.5). Authority
//! is always deletion FLAGS over known source bytes, adjudicated by
//! `validation::validate_deletions` (the known-authority mirror of the frozen
//! validator: no LCS alignment, no byte or word cap on the composed text). The
//! composed flag vector IS the terminal authority: final text is reconstructed
//! only from known `f` bytes, so a long dictation can never be lost to a
//! whole-text alignment limit.
//!
//! The work mask (§4.3) skips only provably no-work windows; a batch reply
//! yields per-occurrence authority at absolute offsets, each run OR-ed into
//! the whole-`F` vector and re-adjudicated, so two windows each validly
//! deleting one copy of a repeated phrase cannot together delete both.

use std::collections::HashMap;
use std::ops::Range;
use std::time::Duration;

use crate::llm::cleanup::sidecar_cleanup_batch;
use crate::models::CleanupMode;
use crate::llm::engine::BatchItem;
use crate::llm::validation::{
    DeletionContext, WorkMask, authorized_caps, caps_flips, deletions_candidate,
    sentence_boundaries, validate_cleanup_with_edits, validate_deletions, word_spans,
};
use crate::models::LlmCleanupStatus;
use crate::state::AppState;

/// Window-sizing knobs (final-mode only: the live-gate fields died with the
/// removed phase-1 path — spec §6). Every request is a bounded target plus
/// bounded left and right context, clamped under `max_request_words`;
/// defaults chosen by measurement (journal §14–§15 of
/// docs/journals/2026-09-12-correction-baseline-experiments.md).
#[derive(Clone, Debug)]
pub struct CorrectionConfig {
    pub max_target_words: usize,
    pub max_left_words: usize,
    pub max_right_words: usize,
    /// Hard per-request word guard (key = left + target + right). The
    /// defaults' combined sizing is far smaller (20+12+8); this clamps
    /// pathological text before the byte gate below.
    pub max_request_words: usize,
}

impl Default for CorrectionConfig {
    fn default() -> Self {
        Self {
            // 20/12/8 chosen over the draft 40/30/20 by measurement
            // (real-model planner probe + paced replays; journal §14–§15 of
            // docs/journals/2026-09-12-correction-baseline-experiments.md).
            max_target_words: 20,
            max_left_words: 12,
            max_right_words: 8,
            max_request_words: 100,
        }
    }
}

/// A ≤100-word bounded window only exceeds this with pathological tokens;
/// such a request is skipped and its region stays raw. Single source of
/// truth: the engine's wire-level item budget.
const MAX_REQUEST_BYTES_SOFT: usize = crate::llm::engine::MAX_ITEM_BYTES;

/// One window to clean: exact source bytes + byte range of the TARGET inside
/// them + the absolute byte offset of the key inside `F`. Context earns no
/// authority; stop-path offsets are authoritative (never derived by searching).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowRequest {
    pub key: String,
    pub target_range: Range<usize>,
    pub source_start: usize,
}

/// Split one contiguous ASR text into ≤`max_target_words` sentence chunks
/// (targets), each wrapped with left/right context. No protection logic here:
/// the work mask skips provably no-work windows, and the per-window validator
/// pass + whole-source composition gate remain the authority; trimming around
/// detected spans would only hide them from validation. Targets partition the
/// text: no token is in two windows' targets and none is skipped.
pub fn plan_windows(text: &str, cfg: &CorrectionConfig) -> Vec<WindowRequest> {
    let spans = word_spans(text);
    if spans.is_empty() {
        return Vec::new();
    }
    let boundaries = sentence_boundaries(text);
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while cursor < spans.len() {
        let cap_end = (cursor + cfg.max_target_words).min(spans.len());
        let boundary = boundaries.iter().copied().find(|&b| b > cursor);
        // Chunk target ends at the sentence terminator inside the cap, else
        // at the cap (overlong span piece).
        let chunk_end = boundary.filter(|&b| b <= cap_end).unwrap_or(cap_end);
        if chunk_end <= cursor {
            break;
        }
        if let Some(request) = build_request(text, &spans, cursor, chunk_end, cfg) {
            out.push(request);
        }
        cursor = chunk_end;
    }
    out
}
/// One left+target+right window for target tokens `[start, end)`, shrinking
/// under the total word cap, then under the soft byte cap. A window whose
/// budget cannot fit even the target is skipped outright — same treatment as
/// the pathological-byte skip: the region stays raw.
fn build_request(
    text: &str,
    spans: &[Range<usize>],
    start: usize,
    end: usize,
    cfg: &CorrectionConfig,
) -> Option<WindowRequest> {
    let target_words = end - start;
    let mut l = cfg.max_left_words.min(start);
    let mut r = cfg.max_right_words.min(spans.len() - end);
    while l + r + target_words > cfg.max_request_words {
        if r > 0 {
            r -= 1;
        } else if l > 0 {
            l -= 1;
        } else {
            break;
        }
    }
    let mut key_start = spans[start - l].start;
    let mut key_end = spans[end - 1 + r].end;
    while key_end - key_start > MAX_REQUEST_BYTES_SOFT && (l > 0 || r > 0) {
        if r > 0 {
            r -= 1;
        } else if l > 0 {
            l -= 1;
        } else {
            r -= 1; // the loop guard leaves l == 0 ⇒ r > 0
        }
        key_start = spans[start - l].start;
        key_end = spans[end - 1 + r].end;
    }
    if key_end - key_start > MAX_REQUEST_BYTES_SOFT {
        return None; // pathological token size; region stays raw
    }
    Some(WindowRequest {
        key: text[key_start..key_end].to_string(),
        target_range: spans[start].start - key_start..spans[end - 1].end - key_start,
        // Offset of the key inside `text`, which for the stop path IS the
        // final transcript: an authoritative absolute offset.
        source_start: key_start,
    })
}

/// A window's settled authority: per-key-token deletion flags plus the
/// capitalization flips (token index, new initial letter) the frozen pass
/// accepted alongside them.
type WindowAuthority = (Vec<bool>, Vec<(usize, char)>);

/// Turn one sidecar proposal into target-only deletion + caps authority for a
/// window, or `None` (rejected). The target subset of the frozen edit set is
/// re-adjudicated against the FULL window bytes via `validate_deletions`
/// (context rules recompute jointly — deletions legal in a joint edit can be
/// illegal alone) and re-derived by the frozen pass over the subset's own
/// output, so what a window earns is exactly what the frozen gate accepts. An
/// all-false result is still settled NO-CHANGE authority. Caps flips the
/// frozen pass accepted for TARGET tokens ride alongside the flags (key-token
/// indices); they are applied globally at terminal time, never here.
/// Apply a Replace-mode edit script to the window's key bytes, returning the
/// reconstructed proposal — or `None` when the reply is not an edit script at
/// all (fail-closed: the region stays raw). Byte-exact port of the benchmark
/// parser validated by the real shipped validator
/// (benchmarks/llm/results/2026-09-14-slm-sweep/EDITFMT/parse_score.py, arm
/// DELIM): strip `<transcript>` tags; `<KEEP>` ⇒ echo the source; per
/// non-blank line, split at the FIRST `|||` (a line without it poisons the
/// whole reply); NEW `<D>` ⇒ delete; empty OLD ⇒ no-op; each edit replaces
/// every non-overlapping occurrence, left to right, sequentially. A `find`
/// matching nothing is a harmless no-op (measured copy-fidelity: attempted
/// edits are byte-exact; misses carry no authority because the result still
/// passes the frozen validator).
fn apply_edit_lines(reply: &str, key: &str) -> Option<String> {
    let s = reply.replace("<transcript>", "").replace("</transcript>", "");
    let s = s.trim();
    if s == "<KEEP>" {
        return Some(key.to_string());
    }
    let mut text = key.to_string();
    for line in s.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Some((old, new)) = line.split_once("|||") else {
            return None; // prose / fallback line: whole reply is unusable
        };
        let new = if new.trim() == "<D>" { "" } else { new };
        if old.is_empty() {
            continue;
        }
        let mut out = String::with_capacity(text.len());
        let mut cur = 0;
        while let Some(rel) = text[cur..].find(old) {
            out.push_str(&text[cur..cur + rel]);
            out.push_str(new);
            cur = cur + rel + old.len();
        }
        out.push_str(&text[cur..]);
        text = out;
    }
    Some(text)
}

fn target_authority(
    request: &WindowRequest,
    proposal: &str,
    terms: &[String],
) -> Option<WindowAuthority> {
    let edits = validate_cleanup_with_edits(&request.key, proposal, terms).ok()?;
    let spans = word_spans(&request.key);
    if edits.deleted.len() != spans.len() {
        return None;
    }
    let target = token_range_of(&spans, &request.target_range)?;
    // Caps the frozen pass accepted inside its own output, restricted to
    // TARGET tokens (context-token caps depend on context the final region
    // may not share). Extraction failure simply caches no caps.
    let flips: Vec<(usize, char)> = caps_flips(&request.key, &edits.output, &edits.deleted)
        .unwrap_or_default()
        .into_iter()
        .filter(|(token, _)| target.contains(token))
        .collect();
    let mut subset = vec![false; spans.len()];
    for i in target.clone() {
        subset[i] = edits.deleted[i];
    }
    if !subset.iter().any(|&deleted| deleted) {
        return Some((subset, flips));
    }
    validate_deletions(&request.key, &subset, terms).ok()?;
    let candidate = deletions_candidate(&request.key, &subset)?;
    let recomputed = validate_cleanup_with_edits(&request.key, &candidate, terms).ok()?;
    if recomputed.deleted != subset {
        return None;
    }
    Some((subset, flips))
}

fn token_range_of(spans: &[Range<usize>], range: &Range<usize>) -> Option<Range<usize>> {
    let first = spans.iter().position(|s| s.end > range.start)?;
    let last = spans.iter().rposition(|s| s.start < range.end)?;
    Some(first..last + 1)
}

/// Locate a byte range's token span by BINARY search over the ascending
/// token spans (O(log tokens), §4.3 complexity). Returns `None` unless the
/// range starts and ends exactly on token boundaries — planner targets
/// always do, so `None` means genuine drift and the caller fails safe.
/// (An empty range cannot name a token either: planner targets are never
/// empty, so an empty byte target is drift too.)
fn bounded_token_range(spans: &[Range<usize>], range: &Range<usize>) -> Option<Range<usize>> {
    let first = spans.partition_point(|s| s.start < range.start);
    if spans.get(first)?.start != range.start {
        return None;
    }
    // The token AFTER the last covered one starts at or after `range.end`;
    // exact byte alignment requires it to start AT `range.end`.
    let after = spans.partition_point(|s| s.start < range.end);
    let last = after.checked_sub(1)?;
    if spans[last].end != range.end || last < first {
        return None;
    }
    Some(first..last + 1)
}

fn contiguous_runs(flags: &[bool], range: Range<usize>) -> Vec<Range<usize>> {
    // Runs end at `range.end`, NEVER at `flags.len()`: an open run whose
    // final target token is deleted must not extend into the window's right
    // context (or, after shifting, into unrelated words of the final text).
    let end = range.end;
    let mut runs = Vec::new();
    let mut start: Option<usize> = None;
    for i in range {
        let on = flags.get(i).copied().unwrap_or(false);
        if on && start.is_none() {
            start = Some(i);
        } else if !on && start.is_some() {
            runs.push(start.take().unwrap()..i);
        }
    }
    if let Some(s) = start {
        runs.push(s..end);
    }
    runs
}

/// OR each candidate run (token ranges over `word_spans(f)`, ascending by
/// position) into the shared flag vector, re-adjudicating the WHOLE vector
/// through the prepared `DeletionContext` after each tentative accept
/// (deletions are non-monotonic: a later run can invalidate an earlier one,
/// so the group analysis — computed once on the full text — is consulted on
/// the full vector every time). Only the run that breaks admissibility is
/// dropped — never the rest of the plan. Rejection restores exactly the
/// run's own range (runs it touched were false before; nothing else moved),
/// so no whole-vector snapshot is needed. Returns the count of accepted
/// runs; the caller's `flags` hold the final accepted vector.
fn compose_runs(
    ctx: &DeletionContext<'_>,
    runs: impl IntoIterator<Item = Range<usize>>,
    flags: &mut [bool],
) -> usize {
    let mut accepted = 0usize;
    for run in runs {
        if run.is_empty() || run.end > flags.len() {
            continue;
        }
        flags[run.clone()].fill(true);
        if ctx.adjudicate(flags).is_ok() {
            accepted += 1;
        } else {
            flags[run.clone()].fill(false);
            log::info!(
                "incremental: run [{}, {}) breaks composition, dropped",
                run.start,
                run.end
            );
        }
    }
    accepted
}
/// The gather-then-apply core of phase D. `authority[i]` is the derived
/// authority for the i-th PLANNED WINDOW (index = window order over the
/// whole F; absent windows contribute `None` entries, so no per-entry sort
/// and no duplicated owners are needed — application is a single
/// ascending `for i in 0..authority.len()` pass, O(tokens + windows)).
/// Authorities may be ASSIGNED out of order (dispatch completes
/// chunk-by-chunk, and a later window's key may be answered before an
/// earlier one's); iteration order, not assignment order, is what makes the
/// composition deterministic. Non-commuting candidates (e.g. `um the um
/// the`: H0 = hesitation token 0, R = repeated block tokens 2..4 — H0 first
/// keeps "the um the", R first keeps "um the") therefore resolve by WINDOW
/// position regardless of arrival (Director keep-one invariant). Returns
/// the accepted-run count.
fn apply_authorities_in_window_order(
    ctx: &DeletionContext<'_>,
    authority: &[Option<Vec<Range<usize>>>],
    flags: &mut [bool],
) -> usize {
    authority.iter().fold(0usize, |accepted, runs| match runs {
        Some(runs) => accepted + compose_runs(ctx, runs.iter().cloned(), flags),
        None => accepted,
    })
}

/// One planned window occurrence with its precomputed F-token mapping.
struct WindowOcc<'a> {
    request: &'a WindowRequest,
    /// Target tokens in F's token space (`0..0` sentinel when the byte
    /// target drifts out of the tokenization — impossible for planner
    /// output, guarded anyway; the window then fails safe to ACTIVE and
    /// Phase C honestly drops its runs). Targets partition F's words.
    f_target: Range<usize>,
    active: bool,
}

/// One distinct active key, deduped by exact key bytes. Phase D iterates
/// the occurrences directly (with `slot_of` key→slot lookup), so no owner
/// lists are tracked here.
struct Authority<'a> {
    request: &'a WindowRequest,
}

/// The shared cleanup engine — EVERY caller, session or not.
///
/// `f` (the full-file ASR text) is planned into bounded windows at known
/// absolute offsets. The work mask (§4.3) proves which windows can possibly
/// change: an inactive window is skipped WITHOUT a request, and every active
/// key is asked exactly once per cleanup regardless of how many windows own
/// it (occurrence eligibility: requested once iff ANY owning window is
/// active — never first-occurrence eligibility). A batch reply yields
/// per-occurrence authority applied at each active occurrence's absolute
/// offset, in ORIGINAL window order: the global keep-one adjudication in
/// `compose_runs` only ever sees flag runs over `f`'s own bytes, so
/// batching cannot reorder global priority.
///
/// Terminal authority: the shared `DeletionContext` adjudicates the composed
/// vector. Output is constructed from `f`'s own bytes — no model-proposal
/// alignment, no byte/word cap — so one failed chunk can never cost the
/// whole transcript: later chunks still run and their regions still apply
/// (per-window isolation became per-chunk + per-item isolation).
/// Take the `llm_operation` permit for one cleanup pass.
///
/// Ordinary contention (download, prepare, another cleanup) answers
/// `None` immediately — the existing "busy" semantics. The one exception
/// is the recording-start speculative prewarm: while `llm_prewarming` is
/// set, a cleanup that arrives mid-prewarm waits up to `handoff_wait` for
/// the sentinel generation to finish and the permit to drop, because that
/// generation IS the page-in this cleanup needs anyway. `handoff_wait` is
/// a parameter so tests can exercise the expiry without a 10 s real wait.
async fn acquire_cleanup_operation(
    state: &AppState,
    handoff_wait: Duration,
) -> Option<tokio::sync::MutexGuard<'_, ()>> {
    match state.llm_operation.try_lock() {
        Ok(guard) => Some(guard),
        Err(_) if state.llm_prewarming.load(std::sync::atomic::Ordering::SeqCst) => {
            tokio::time::timeout(handoff_wait, state.llm_operation.lock())
                .await
                .ok()
        }
        Err(_) => None,
    }
}

pub(crate) async fn plan_cleanup(
    state: &AppState,
    f: &str,
    terms: &[String],
) -> (String, LlmCleanupStatus) {
    let started = std::time::Instant::now();
    // Busy gate. An ordinary collision (download, prepare, a real cleanup)
    // answers immediately with raw preserved, exactly as before. The one
    // exception is the recording-start speculative prewarm: it holds
    // llm_operation for a single tiny generation whose whole purpose is to
    // page the weights in, so waiting for it IS the fast path — skipping
    // correction then would silently discard a real cleanup opportunity.
    // Hence the bounded handoff wait, gated on the prewarm flag.
    let operation = acquire_cleanup_operation(state, crate::llm::cleanup::PREWARM_HANDOFF_WAIT).await;
    let Some(_operation) = operation else {
        return (
            f.to_string(),
            LlmCleanupStatus::Unavailable {
                reason: "Cleanup is busy preparing or loading; original text preserved".into(),
            },
        );
    };

    // One mode snapshot per cleanup: mid-cleanup setting changes must not
    // make two chunks of the same pass speak different protocols.
    let mode = state.settings.lock().await.llm_cleanup_mode;

    let cfg = CorrectionConfig::default();
    let windows = plan_windows(f, &cfg);
    let n_windows = windows.len();
    // E10: a wordless `f` (empty/whitespace/punctuation-only) yields zero
    // windows ⇒ NoChanges with no dispatch. A 1-word `f` is a normal
    // one-window batch whose reply may legitimately be "" (E1).
    if windows.is_empty() {
        return (f.to_string(), LlmCleanupStatus::NoChanges);
    }

    let f_tokens = word_spans(f);
    // One shared whole-F adjudication context: repeat/restart/protection
    // analysis depends only on (f, terms), never on the flag vector, so the
    // work mask, the per-run compose gate and the terminal gate all reuse
    // it (35x on long transcripts; equivalence-proven against per-call
    // validate_deletions).
    let del_ctx = DeletionContext::prepare(f, terms);
    // The no-work mask (§4.3) runs on DeletionContext's own tokenization.
    // Any protection-analysis error fails safe: ALL windows active (ask
    // everything) — never a silent no-work, never a panic, never a
    // raw-skip caused by broken analysis.
    let mask = match del_ctx.work_mask() {
        Ok(mask) => mask,
        Err(reason) => {
            log::warn!("cleanup: work mask unavailable ({reason}); all windows active");
            WorkMask {
                possible: vec![true; f_tokens.len()],
                cap_opportunity: vec![false; f_tokens.len()],
            }
        }
    };

    // Phase A: activity per window — a linear `.any()` over the window's own
    // target tokens (targets partition F's words, so total work here is
    // O(tokens + windows); locating each target in the token space is a
    // binary bounded search, never a per-window scan or materialised
    // vector). The mask is a NECESSARY condition (eligibility proof for
    // skipping), never a joint-legality proof: composition adjudication
    // stays the sole authority, and the mask may only skip requests.
    let mut active_windows = 0usize;
    let occurrences: Vec<WindowOcc> = windows
        .iter()
        .map(|request| {
            let absolute_target = request.source_start + request.target_range.start
                ..request.source_start + request.target_range.end;
            let (f_target, drift) = match bounded_token_range(&f_tokens, &absolute_target) {
                Some(range) => (range, false),
                None => (0..0, true),
            };
            let occ = WindowOcc {
                request,
                f_target: f_target.clone(),
                // Drift fails safe ACTIVE: ask anyway, let the per-window
                // authority gate be the judge — never a silent skip caused
                // by broken alignment analysis.
                active: drift
                    || f_target
                        .clone()
                        .any(|t| mask.possible[t] || mask.cap_opportunity[t]),
            };
            if occ.active {
                active_windows += 1;
            }
            occ
        })
        .collect();

    // Phase B: occurrence-eligibility dedup (§4.3): key k is requested once
    // iff ANY absolute window owning k is active; every active occurrence
    // keeps its own authority slot, inactive copies of an active key take
    // identity (skipped) explicitly. First-ACTIVE occurrence order fixes the
    // chunk position; Phase D reads owners through `slot_of`, never a list.
    let mut slots: Vec<Authority> = Vec::new();
    let mut slot_of: HashMap<&str, usize> = HashMap::new();
    for occ in occurrences.iter() {
        if !occ.active {
            continue;
        }
        slot_of
            .entry(occ.request.key.as_str())
            .or_insert_with(|| {
                let slot = slots.len();
                slots.push(Authority {
                    request: occ.request,
                });
                slot
            });
    }

    let mut flags = vec![false; f_tokens.len()];
    let mut applied_runs = 0usize;
    // Window-authorized caps flips translated to `f` token indices, applied
    // ONCE against the whole text by `authorized_caps` after the global
    // deletion gate passes.
    let mut all_flips: Vec<(usize, char)> = Vec::new();
    let mut first_status: Option<LlmCleanupStatus> = None;
    let mut first_rejected: Option<LlmCleanupStatus> = None;

    // Phase C: DISPATCH + per-key authority derivation only — nothing is
    // composed here. Each distinct active key is requested once, in plain
    // count-of-16 chunks (§4.5: the static line budgets dominate every
    // valid chunk — count is the only criterion, no packer). The owned
    // chunk vector is materialized ONCE per dispatch at this IPC boundary
    // and MOVED into the client. A chunk failure strikes only its own
    // regions: later chunks still RUN (each gets its own deadline) and
    // their proposals are still derived.
    //
    // ONLY the raw proposal is cached per key (the deduped reply over the
    // key's own bytes). Authority is NEVER key-local: `target_authority`
    // masks to ONE request's target range, so a shared authority would
    // silently strip the legitimate runs/caps of every later owner whose
    // target differs (and let a first owner's rejection hide a later
    // owner's legal subset). Phase D re-derives it per occurrence.
    let mut per_key: Vec<Option<String>> = vec![None; slots.len()];
    for (base, chunk) in slots.chunks(crate::llm::engine::BATCH_CAP).enumerate() {
        let texts: Vec<String> = chunk.iter().map(|slot| slot.request.key.clone()).collect();
        let items = match sidecar_cleanup_batch(state, texts, mode).await {
            Ok(items) => items,
            Err(status) => {
                log::info!("cleanup: batch chunk failed: {status:?}");
                first_status.get_or_insert(status);
                continue;
            }
        };
        for (j, (slot, item)) in chunk.iter().zip(items).enumerate() {
            // Acceptance is typed per item, never positional and never by
            // truthiness: Some("") is a VALID all-deletable proposal (E1).
            let proposal = match item {
                BatchItem::Proposal(Some(proposal)) => proposal,
                BatchItem::Proposal(None) => {
                    log::info!("cleanup: success item without proposal text; regions stay raw");
                    first_status.get_or_insert_with(|| LlmCleanupStatus::Failed {
                        reason: "sidecar returned a success item without a proposal".into(),
                    });
                    continue;
                }
                BatchItem::TimedOut => {
                    // Auditable per-item outcome: the FINAL status can be
                    // Applied while individual items timed out (any surviving
                    // edit outranks), so the wire log is what distinguishes
                    // "all questions completed" from "partial raw
                    // preservation".
                    log::info!(
                        "cleanup: batch item {} timed out; its region stays raw",
                        slot.request.source_start
                    );
                    first_status.get_or_insert(LlmCleanupStatus::TimedOut {
                        elapsed_ms: started.elapsed().as_millis() as u64,
                    });
                    continue;
                }
                BatchItem::Failed(code) => {
                    log::info!("cleanup: batch item failed ({code}); regions stay raw");
                    first_status.get_or_insert_with(|| LlmCleanupStatus::Failed {
                        reason: format!("cleanup item failed: {code}"),
                    });
                    continue;
                }
            };
            per_key[base * crate::llm::engine::BATCH_CAP + j] = Some(proposal);
        }
    }

    // Phase D: DERIVE each occurrence's OWN authority from its key's
    // proposal, then APPLY in ORIGINAL WINDOW ORDER (Director invariant —
    // strictly ascending occurrence index, NEVER slot/chunk order).
    // Gather-then-apply makes the greedy keep-one composition identical to
    // the serial path for cross-owner and cross-batch-boundary overlaps:
    // an earlier window's run is never denied by a later window that
    // dispatched first. Derivation is per-OCCURRENCE: identical key bytes
    // can carry different target slices, and each owner's subset must
    // independently pass the frozen revalidation.
    // Indexed slots: one authority entry PER PLANNED WINDOW (window order
    // over the whole F). Owners assign their slice by window index during
    // the gather pass below; the single ascending apply pass reads slots in
    // window order. Inactive/drifted/never-dispatched windows keep `None`.
    let mut authority: Vec<Option<Vec<Range<usize>>>> = vec![None; windows.len()];
    let mut flip_relocations: Vec<(usize, char)> = Vec::new();
    for (i, occ) in occurrences.iter().enumerate() {
        if !occ.active {
            continue; // an INACTIVE occurrence never earns application,
            // even when its key was dispatched for another owner — the
            // mask's no-work proof means the whole F gains nothing here.
        }
        let Some(slot) = slot_of.get(occ.request.key.as_str()) else {
            continue; // never-dispatched key (all owners inactive)
        };
        let Some(proposal) = per_key[*slot].as_ref() else {
            continue; // never dispatched, failed, or timed out
        };
        // Replace mode: the reply is an edit SCRIPT, not a proposal. Rust
        // applies it verbatim to the key bytes (the same parse the benchmark
        // harness validated); a parse failure means the reply was not an
        // edit script at all and this region stays raw — never a silent
        // retype fallback. The reconstructed text is then adjudicated by
        // the unchanged frozen validator below.
        let edited: Option<String> = match mode {
            CleanupMode::Retype => None,
            CleanupMode::Replace => match apply_edit_lines(proposal, &occ.request.key) {
                Some(text) => Some(text),
                None => {
                    log::info!("cleanup: replace-mode reply unparseable; region stays raw");
                    first_status.get_or_insert_with(|| LlmCleanupStatus::Failed {
                        reason: "replace-mode reply was not a valid edit script".into(),
                    });
                    continue;
                }
            },
        };
        let proposal: &str = match &edited {
            Some(text) => text,
            None => proposal.as_str(),
        };
        let Some((deleted, flips)) = target_authority(occ.request, proposal, terms) else {
            log::info!("cleanup: window proposal rejected by validator");
            first_rejected.get_or_insert_with(|| LlmCleanupStatus::Rejected {
                reason: "model proposal rejected by validator".into(),
            });
            continue;
        };
        if occ.f_target.is_empty() {
            log::info!("cleanup: window target token drift, runs dropped");
            continue;
        }
        let Some(key_target) =
            token_range_of(&word_spans(&occ.request.key), &occ.request.target_range)
        else {
            log::info!("cleanup: window target token drift, runs dropped");
            continue;
        };
        if key_target.len() != occ.f_target.len() {
            log::info!("cleanup: window target token drift, runs dropped");
            continue;
        }
        let offset = occ.f_target.start - key_target.start;
        let runs: Vec<Range<usize>> = contiguous_runs(&deleted, key_target.clone())
            .into_iter()
            .map(|run| run.start + offset..run.end + offset)
            .collect();
        // Occurrences are built 1:1 over `windows` in order, so `i` IS the
        // window index — no extra bookkeeping.
        authority[i] = Some(runs);
        for (token, letter) in flips {
            if key_target.contains(&token) {
                flip_relocations.push((token + offset, letter));
            }
        }
    }
    // APPLY ONLY NOW — one ascending pass over the window-indexed slots.
    // The gather pass above never touched `flags`.
    applied_runs += apply_authorities_in_window_order(&del_ctx, &authority, &mut flags);
    all_flips.extend(flip_relocations);

    log::info!(
        "cleanup complete windows={n_windows} active_windows={active_windows} active_keys={} applied_runs={applied_runs} f_tokens={} elapsed_ms={}",
        slots.len(),
        f_tokens.len(),
        started.elapsed().as_millis()
    );

    // Terminal authority. compose_runs keeps the vector admissible at every
    // accept; this final adjudication re-validates it (defense in depth).
    // Output is constructed from f's own bytes — never model text. Window
    // caps ride on top of the PASSED deletion output only: the global flip
    // gate in `authorized_caps` (protections of the ORIGINAL source,
    // sentence_start on the deletion output) is the sole mutator, and f is
    // never pre-guarded.
    match del_ctx.adjudicate(&flags) {
        Ok(candidate) => {
            let output = if all_flips.is_empty() {
                candidate
            } else {
                authorized_caps(f, &flags, &all_flips, terms)
            };
            if output != f {
                (
                    output,
                    LlmCleanupStatus::Applied {
                        elapsed_ms: started.elapsed().as_millis() as u64,
                    },
                )
            } else {
                // Nothing survived — and the status comes from the ACTUAL
                // final diff, never from an accepted-run count (a caps-only
                // proposal with zero deletion bits reaches Applied above; a
                // fully-rejected one lands here honestly). Distinguish:
                // a transport/window failure outranks, then a model
                // rejection, then the genuine settled NO-CHANGE /
                // protected-abstention result.
                (
                    f.to_string(),
                    first_status
                        .or(first_rejected)
                        .unwrap_or(LlmCleanupStatus::NoChanges),
                )
            }
        }
        Err(reason) => {
            log::warn!("cleanup: composition gate rejected final flags: {reason}");
            (
                f.to_string(),
                first_status
                    .or(first_rejected)
                    .unwrap_or(LlmCleanupStatus::Rejected { reason }),
            )
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::engine::{LlmBackend, MAX_REQUEST_LINE_BYTES};
    use crate::llm::validation::validate;
    use crate::models::Settings;
    use crate::test_support::{MockAsrEngine, MockAudioCapture, MockLlmBackend, MockPasteBackend};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// Mock that deletes every standalone occurrence of one hesitation from
    /// EACH batch text, standing in for a well-behaved sidecar across many
    /// windows. Batch-level by construction: one call, one item per text.
    struct Stripper(&'static str);

    impl LlmBackend for Stripper {
        fn cleanup_batch(&mut self, texts: &[String], _mode: CleanupMode) -> Result<Vec<BatchItem>, String> {
            let filler = format!(" {} ", self.0);
            let dotted = format!(" {}.", self.0);
            Ok(texts
                .iter()
                .map(|text| {
                    let out = text.replace(&filler, " ").replace(&dotted, ".");
                    BatchItem::Proposal(Some(out))
                })
                .collect())
        }
        fn request_raw(&mut self, _: &serde_json::Value) -> Result<serde_json::Value, String> {
            Err("no protocol requests expected".into())
        }
    }

    /// Backend whose every batch item is `Proposal(None)` — the parser's
    /// mapping for an ok-without-text wire item, which must behave as a
    /// per-item failure, NEVER as an erase-everything empty edit.
    struct NoText;

    impl LlmBackend for NoText {
        fn cleanup_batch(&mut self, texts: &[String], _mode: CleanupMode) -> Result<Vec<BatchItem>, String> {
            Ok(texts.iter().map(|_| BatchItem::Proposal(None)).collect())
        }
        fn request_raw(&mut self, _: &serde_json::Value) -> Result<serde_json::Value, String> {
            Err("no protocol requests expected".into())
        }
    }

    fn state_with_llm(backend: impl LlmBackend + 'static) -> AppState {
        AppState::new_with_backends(
            Box::new(MockAudioCapture::sine_wave()),
            Box::new(MockAsrEngine::with_text("unused")),
            Some(Box::new(backend)),
            Box::new(MockPasteBackend::new()),
            Settings::default(),
        )
    }

    /// Composition safety regression: two runs that are EACH independently
    /// valid deletions over `f`, whose union deletes both copies of an
    /// adjacent repeat, must compose to exactly one; a later independent run
    /// is still accepted after the conflicting run is dropped.
    /// The Director keep-one ordering regression, at the gather/apply seam.
    /// `um the um the` carries two non-commuting candidates:
    /// H0 = hesitation deletion at token 0 (window 0), R = repeated block at
    /// tokens 2..4 (window 1). Applied ascending: [H0 accepted, R dropped]
    /// → "the um the"; applied in REVERSE arrival order: R first keeps
    /// "um the". Authorities are ASSIGNED out of order into window-indexed
    /// slots (window 1 answered first), the single ascending pass still
    /// applies window 0 first, so out-of-order arrival cannot change which
    /// candidate survives. (Pre-restructure, composition ran inside the
    /// per-chunk dispatch loop — feeding this call the same reversed
    /// assignment produced "um the".)
    #[test]
    fn out_of_order_authority_still_applies_in_window_order() {
        let f = "um the um the";
        let terms: Vec<String> = Vec::new();
        let ctx = DeletionContext::prepare(f, &terms);
        assert_eq!(word_spans(f).len(), 4);
        // Two planned windows; dispatch answers LATER window first.
        let mut authority: Vec<Option<Vec<Range<usize>>>> = vec![None, None];
        authority[1] = Some(std::iter::once(2..4usize).collect()); // arrives first
        authority[0] = Some(std::iter::once(0..1usize).collect()); // arrives second
        let mut flags = vec![false; 4];
        let accepted = apply_authorities_in_window_order(&ctx, &authority, &mut flags);
        assert_eq!(accepted, 1, "exactly one non-commuting candidate survives");
        let output = deletions_candidate(f, &flags).expect("admissible flags");
        assert_eq!(
            output, "the um the",
            "window 0 must win its position despite answering last"
        );
        ctx.adjudicate(&flags).expect("composition stays admissible");
    }

    #[test]
    fn composition_rejects_only_the_run_that_breaks_the_repeat() {
        // tokens: one(0) the(1) the(2) two(3) three(4) the(5) the(6) four(7)
        let f = "one the the two three the the four";
        let terms: Vec<String> = Vec::new();
        let ctx = DeletionContext::prepare(f, &terms);
        let mut flags = vec![false; 8];
        // run 1..2 deletes the(1): sibling block 2..3 fully kept → valid.
        // run 2..3 would delete the(2) too: both copies go, no kept sibling
        //   → the ONLY rejected run.
        // run 6..7 deletes the(6): sibling 5..6 kept → still accepted after.
        let accepted = compose_runs(&ctx, vec![1..2, 2..3, 6..7], &mut flags);
        assert_eq!(accepted, 2, "only the run that empties the repeat is dropped");
        assert_eq!(
            flags,
            vec![false, true, false, false, false, false, true, false]
        );
        let out = ctx.adjudicate(&flags).expect("final vector admissible");
        assert_eq!(out, "one the two three the four");
    }

    /// End-to-end (director contract): deletions BEFORE retained words must
    /// not shift the retained text or its punctuation, and a window-
    /// authorized sentence-start capital rides the terminal reconstruction.
    /// The terminal pipeline reproduces the delivered text byte-exactly from
    /// `raw`'s own bytes.
    #[tokio::test]
    async fn deletion_before_retained_words_preserves_text_and_caps() {
        struct Capting;
        impl LlmBackend for Capting {
            fn cleanup_batch(&mut self, texts: &[String], _mode: CleanupMode) -> Result<Vec<BatchItem>, String> {
                Ok(texts
                    .iter()
                    .map(|t| BatchItem::Proposal(Some(t.replacen("um never", "Never", 1))))
                    .collect())
            }
            fn request_raw(&mut self, _: &serde_json::Value) -> Result<serde_json::Value, String> {
                Err("no protocol requests expected".into())
            }
        }
        let state = state_with_llm(Capting);
        let raw = "um never do that. keep this.";
        let (out, status) = plan_cleanup(&state, raw, &[]).await;
        assert!(matches!(status, LlmCleanupStatus::Applied { .. }), "{status:?}");
        assert_eq!(out, "Never do that. keep this.", "{out:?}");
        // Terminal authority reproduces it from f's bytes alone.
        let terms: Vec<String> = Vec::new();
        assert!(validate(raw, &out, &terms).is_ok(), "frozen validator agrees");
    }

    /// Director mask-edge contract: a caps-ONLY proposal (ZERO deletion
    /// bits) must still dispatch and still land Applied — status comes from
    /// the ACTUAL final diff (`hello there` → `Hello there` changes the text
    /// with no deletions at all). A no-op echo over the same text stays
    /// NoChanges, proving Applied here reflects the diff, not a run count.
    #[tokio::test]
    async fn caps_only_proposal_applies_from_the_final_diff() {
        struct CapsOnly;
        impl LlmBackend for CapsOnly {
            fn cleanup_batch(&mut self, texts: &[String], _mode: CleanupMode) -> Result<Vec<BatchItem>, String> {
                Ok(texts
                    .iter()
                    .map(|t| {
                        let mut chars = t.chars();
                        let first = chars.next().unwrap_or(' ').to_uppercase().to_string();
                        BatchItem::Proposal(Some(first + chars.as_str()))
                    })
                    .collect())
            }
            fn request_raw(&mut self, _: &serde_json::Value) -> Result<serde_json::Value, String> {
                Err("no protocol requests expected".into())
            }
        }
        let state = state_with_llm(CapsOnly);
        let (out, status) = plan_cleanup(&state, "hello there", &[]).await;
        assert!(matches!(status, LlmCleanupStatus::Applied { .. }), "{status:?}");
        assert_eq!(out, "Hello there", "caps-only diff, zero deletions: {out:?}");

        // Contrast: echo reply ⇒ zero diff ⇒ NoChanges.
        let echo = state_with_llm(MockLlmBackend::passthrough());
        let (out2, status2) = plan_cleanup(&echo, "hello there", &[]).await;
        assert_eq!(out2, "hello there");
        assert!(matches!(status2, LlmCleanupStatus::NoChanges), "{status2:?}");
    }

    /// E1/E10 director contract: a 1-word `F` ("um") is a normal one-window
    /// batch whose fully-erasing reply `""` is VALID; cleanup reports Applied
    /// with empty output (history/clipboard behavior is pipeline-owned).
    /// Whitespace-only F dispatches NOTHING and reports NoChanges.
    #[tokio::test]
    async fn single_word_hesitation_fully_erases_to_empty_applied() {
        let state = state_with_llm(MockLlmBackend::proposal(""));
        let (out, status) = plan_cleanup(&state, "um", &[]).await;
        assert_eq!(out, "", "{out:?}");
        assert!(matches!(status, LlmCleanupStatus::Applied { .. }), "{status:?}");

        let blank = state_with_llm(MockLlmBackend::failing("must not be called"));
        let (out, status) = plan_cleanup(&blank, "   ", &[]).await;
        assert_eq!(out, "   ");
        assert!(matches!(status, LlmCleanupStatus::NoChanges), "{status:?}");
    }

    /// Targets partition the text; each target ≤ cap; every token is planned
    /// exactly once (final mode: no live tail deferral any more).
    #[test]
    fn window_targets_partition_the_whole_text() {
        let cfg = CorrectionConfig {
            max_target_words: 4,
            max_left_words: 2,
            max_right_words: 2,
            max_request_words: 8,
        };
        // 4 words/sentence x3, then a 3-word tail.
        let text = "alpha bravo charlie delta. echo foxtrot golf hotel. india juliet kilo lima. mike november oscar";
        let spans = word_spans(text);
        let windows = plan_windows(text, &cfg);
        let mut covered = vec![false; spans.len()];
        for w in &windows {
            let base = text.find(&w.key).expect("key is a substring");
            let t = token_range_of(
                &spans,
                &(base + w.target_range.start..base + w.target_range.end),
            )
            .unwrap();
            assert!(!t.is_empty());
            assert!(t.end - t.start <= cfg.max_target_words);
            for i in t {
                assert!(!covered[i], "target token {i} covered twice");
                covered[i] = true;
            }
        }
        assert!(
            covered.iter().all(|&c| c),
            "final plan must cover every token"
        );
    }

    /// Byte-gate skip: a target whose window cannot fit the 4 KiB soft cap
    /// even without context is SKIPPED (region stays raw) — and the skip
    /// must not corrupt partitioning of the rest.
    #[test]
    fn overlong_target_skips_keep_the_rest_planned() {
        let cfg = CorrectionConfig::default();
        let huge = "x".repeat(5000);
        let text = format!("alpha um bravo. {huge} charlie um delta.");
        let windows = plan_windows(&text, &cfg);
        assert!(
            windows.iter().all(|w| w.key.len() <= MAX_REQUEST_BYTES_SOFT),
            "planner never emits an over-budget key"
        );
        assert!(
            windows.iter().any(|w| w.key.contains("alpha")),
            "plannable windows survive the skip"
        );
        assert!(
            !windows.iter().any(|w| w.key.contains(&huge[..100])),
            "the un-plannable region must never be dispatched"
        );
    }

    /// §7.2b wire budget shape: the planner chunking splits >16 distinct
    /// keys across sequential full-cap batches (observed through the mock's
    /// per-call batch sizes), and a full BATCH_CAP batch of worst-case JSON
    /// escaping still fits the client's request-line cap.
    #[tokio::test]
    async fn chunks_split_at_sixteen_with_escaped_budget_headroom() {
        // 800 words = 40 twenty-word windows, every one active (a filler in
        // each target) ⇒ dispatch sizes must be [16, 16, 8].
        let f: String = (0..400)
            .map(|i| format!("word{i} um "))
            .collect::<Vec<_>>()
            .join("");
        struct SharedRecorder {
            seen: Arc<Mutex<Vec<usize>>>,
        }
        impl LlmBackend for SharedRecorder {
            fn cleanup_batch(&mut self, texts: &[String], _mode: CleanupMode) -> Result<Vec<BatchItem>, String> {
                assert!(
                    texts.len() <= crate::llm::engine::BATCH_CAP,
                    "client dispatched a {}-text batch",
                    texts.len()
                );
                self.seen.lock().unwrap().push(texts.len());
                Ok(texts
                    .iter()
                    .map(|t| {
                        // A key may END on the filler token (no trailing
                        // space inside the key bytes); strip that too.
                        let s = t.replace(" um ", " ");
                        BatchItem::Proposal(Some(match s.strip_suffix(" um") {
                            Some(stripped) => stripped.to_string(),
                            None => s,
                        }))
                    })
                    .collect())
            }
            fn request_raw(&mut self, _: &serde_json::Value) -> Result<serde_json::Value, String> {
                Err("none".into())
            }
        }
        let seen = Arc::new(Mutex::new(Vec::new()));
        let state = state_with_llm(SharedRecorder {
            seen: Arc::clone(&seen),
        });
        let (out, status) = plan_cleanup(&state, &f, &[]).await;
        let sizes = seen.lock().unwrap().clone();
        assert_eq!(
            sizes,
            vec![16, 16, 8],
            "count-of-BATCH_CAP chunking over 40 active keys"
        );
        assert!(matches!(status, LlmCleanupStatus::Applied { .. }), "{status:?}");
        assert!(!out.contains(" um "), "all chunks' edits shipped: {out:?}");

        // Static framing budget: BATCH_CAP keys at the planner's own
        // window cap (4 KiB raw, worst-case escaping: every NUL byte →
        // 6-char \u0000 escape) fit the request line cap.
        let dense = "\u{0}".repeat(crate::llm::engine::MAX_ITEM_BYTES);
        let request = serde_json::json!({
            "action": "cleanup_batch",
            "texts": vec![dense; crate::llm::engine::BATCH_CAP],
        });
        let line = serde_json::to_vec(&request).unwrap().len() as u64;
        assert!(line <= MAX_REQUEST_LINE_BYTES, "escaped batch line {line} B");
    }

    /// E2 (director-named): duplicate key, MIXED eligibility — the adapter
    /// fixture `mixed_dup_case_truekey.json`. Both copies of the repeated
    /// sentence carry the same window key bytes; the quoted copy is fully
    /// protected (mask-inactive), the live copy is not. The key is requested
    /// ONCE, and the shared reply applies ONLY to the live occurrence: the
    /// quoted `um` stays, the live `um` goes. Asserted through FINAL TEXT.
    #[tokio::test]
    async fn duplicate_key_mixed_eligibility_keeps_protected_copy() {
        // Reference fixture (verbatim): "client um before" appears twice —
        // occurrence 1 inside committee quotes, occurrence 2 after the chair
        // repeats the wording live. Expected output = input with EXACTLY the
        // live copy's "um " removed.
        let f = "The minutes opened at noon and the chair read the previous record aloud. He said, \u{201c}Everyone in the room agreed that the revised schedule for the rollout looked reasonable. please confirm today that the corrected build ships for the client um before Friday. and all owners must confirm their parts in writing today without further delay. That text stands as the official decision of this committee per policy.\u{201d} Later the chair repeated the exact wording live for the new recorder who missed it. Everyone in the room agreed that the revised schedule for the rollout looked reasonable. please confirm today that the corrected build ships for the client um before Friday. and all owners must confirm their parts in writing today without further delay. The recorder filed it.";
        let quote_end = f.rfind('\u{201d}').expect("quote");
        let first = f.find("um ").expect("occurrence 1");
        let second = first + f[first + 1..].find("um ").expect("occurrence 2") + 1;
        assert!(first < quote_end && second > quote_end, "fixture shape drifted");
        let expected = format!("{}{}", &f[..second], &f[second + 3..]);
        let state = state_with_llm(Stripper("um"));
        let (out, _status) = plan_cleanup(&state, f, &[]).await;
        assert_eq!(out, expected, "quoted copy raw, live copy cleaned");
    }

    /// Per-OCCURRENCE authority regression (Director fixture): two planned
    /// windows can share one deduped key with DIFFERENT target slices. A
    /// key-local authority (masked to the first owner's target) silently
    /// strips the second owner's legitimate runs.
    ///
    /// `F = K + " " + K` with K 28 tokens (hesitations at local 5 and 25).
    /// Default planner (20/12/8, no punctuation ⇒ cap-packed targets):
    /// window 0 = key `K` (target local 0..20 → owns local um 5),
    /// window 1 = key F[8..48] (a DIFFERENT 40-token key; owns global um
    /// 25/33 in its target), window 2 = key `K` again (target local 12..28
    /// → owns global um 53). One proposal removes every standalone `um`;
    /// ALL FOUR hesitations must vanish from the final text while only TWO
    /// distinct keys are ever dispatched. Key-local authority drops the
    /// window-2 run (um 53 survives) — this fails pre-fix.
    #[tokio::test]
    async fn duplicate_key_owners_each_derive_their_own_authority() {
        let k = "alpha bravo charlie delta echo um foxtrot golf hotel india \
                juliet kilo lima mike november oscar papa quebec romeo sierra \
                tango uniform victor whiskey xray um yankee zulu";
        let words: Vec<&str> = k.split_whitespace().collect();
        assert_eq!(words.len(), 28);
        let key = words.join(" ");
        let f = format!("{key} {key}");

        // Ground the split against the ACTUAL planner before asserting.
        let windows = plan_windows(&f, &CorrectionConfig::default());
        assert_eq!(windows.len(), 3, "fixture drift: {windows:?}");
        // w0 and w2 share the identical key; w1 is a different 40-word key.
        assert_eq!(windows[0].key, windows[2].key);
        assert_ne!(windows[0].key, windows[1].key);
        // The shared key's two owners have disjoint targets in key space:
        // window 0 keeps the LEFT half, window 2 the RIGHT half.
        // Grounded split: w0 target = key-local 0..20 (OWNS local um@5);
        // w2 target = key-local 12..28 (OWNS local um@25 ⇒ global 53). The
        // two owners' target token ranges over the SAME key bytes are
        // disjoint in the authority that matters: the 25-run only exists
        // inside w2's slice, and w0's authority (masked to 0..20) can never
        // carry it.
        let t0 = &windows[0].key[windows[0].target_range.clone()];
        assert!(
            t0.ends_with("sierra") && t0.contains(" um "),
            "w0 target owns key-local um@5: {t0:?}"
        );
        let t2 = &windows[2].key[windows[2].target_range.clone()];
        assert!(
            t2.starts_with("lima") && t2.contains(" um "),
            "w2 target owns key-local um@25: {t2:?}"
        );
        // Ground truth: each window's target must actually own the filler
        // slices we claim — otherwise the fixture (not the code) is wrong.
        assert!(
            windows.iter().any(|w| {
                let tgt = &w.key[w.target_range.clone()];
                tgt.contains("um")
            }),
            "no window target contains the fillers: fixture invalid"
        );

        #[derive(Default)]
        struct Counting {
            keys: Arc<Mutex<Vec<String>>>,
        }
        impl LlmBackend for Counting {
            fn cleanup_batch(&mut self, texts: &[String], _mode: CleanupMode) -> Result<Vec<BatchItem>, String> {
                self.keys.lock().unwrap().extend(texts.iter().cloned());
                Ok(texts
                    .iter()
                    .map(|t| {
                        BatchItem::Proposal(Some(
                            t.replace(" um ", " ").replace(" um.", "."),
                        ))
                    })
                    .collect())
            }
            fn request_raw(&mut self, _: &serde_json::Value) -> Result<serde_json::Value, String> {
                Err("none".into())
            }
        }
        let keys = Arc::new(Mutex::new(Vec::new()));
        struct Shared(Arc<Mutex<Vec<String>>>);
        impl LlmBackend for Shared {
            fn cleanup_batch(&mut self, texts: &[String], mode: CleanupMode) -> Result<Vec<BatchItem>, String> {
                Counting {
                    keys: self.0.clone(),
                }
                .cleanup_batch(texts, mode)
            }
            fn request_raw(&mut self, _: &serde_json::Value) -> Result<serde_json::Value, String> {
                Err("none".into())
            }
        }
        let state = state_with_llm(Shared(keys.clone()));
        let (out, status) = plan_cleanup(&state, &f, &[]).await;
        assert!(matches!(status, LlmCleanupStatus::Applied { .. }), "{status:?}");

        // Dedup holds: 3 windows, 2 distinct keys, each requested ONCE.
        let dispatched = keys.lock().unwrap().clone();
        let distinct: std::collections::BTreeSet<&String> = dispatched.iter().collect();
        assert_eq!(distinct.len(), 2, "dedup must hold: {dispatched:?}");
        assert_eq!(dispatched.len(), 2, "each distinct key asked exactly once");

        // The observable contract: EVERY filler gone from the final text.
        // Key-local authority leaves the window-2 hesitation (global token
        // 53) untouched — one `um` would survive.
        assert!(
            !out.split_whitespace().any(|w| w == "um"),
            "all four hesitations must be edited out, got: {out:?}"
        );
        // And nothing else moved: same words otherwise, in order.
        let expect: Vec<&str> = key
            .split_whitespace()
            .filter(|w| *w != "um")
            .flat_map(|w| [w].into_iter())
            .collect();
        let mut expect_full = expect.clone();
        expect_full.extend(expect.iter().copied());
        assert_eq!(
            out.split_whitespace().collect::<Vec<_>>(),
            expect_full,
            "final text is the two cleaned copies, nothing else changed"
        );
    }

    /// Chunk isolation: a failing chunk costs ONLY its own regions — the
    /// batch AFTER the strike still runs and its edits still ship (per-chunk
    /// isolation became the spec's headline reliability property).
    #[tokio::test]
    async fn failed_chunk_never_sinks_later_chunks() {
        struct FirstChunkFails {
            seen: AtomicUsize,
        }
        impl LlmBackend for FirstChunkFails {
            fn cleanup_batch(&mut self, texts: &[String], _mode: CleanupMode) -> Result<Vec<BatchItem>, String> {
                if self.seen.fetch_add(1, Ordering::SeqCst) == 0 {
                    // A typed, NON-retire batch fault: handle stays, later
                    // chunks still dispatch, this chunk's regions stay raw.
                    Err("Cleanup endpoint declined: synthetic first-chunk fault".into())
                } else {
                    Ok(texts
                        .iter()
                        .map(|t| BatchItem::Proposal(Some(t.replace(" um ", " "))))
                        .collect())
                }
            }
            fn request_raw(&mut self, _: &serde_json::Value) -> Result<serde_json::Value, String> {
                Err("no protocol requests expected".into())
            }
        }
        let state = AppState::new_with_backends(
            Box::new(MockAudioCapture::sine_wave()),
            Box::new(MockAsrEngine::with_text("unused")),
            Some(Box::new(FirstChunkFails {
                seen: AtomicUsize::new(0),
            })),
            Box::new(MockPasteBackend::new()),
            Settings::default(),
        );
        // One 5-word sentence per window (default targets break at every
        // sentence boundary) ⇒ 20 distinct active keys ⇒ two count-of-16
        // chunks (16 + 4): chunk 1 strikes, chunk 2 still RUNS and ships.
        // Two chunks is the minimal shape that proves isolation; the old
        // 2400-window fixture cost minutes re-adjudicating the whole-F
        // composition per accepted run without testing anything extra.
        let f: String = (0..20)
            .map(|i| format!("sentence {i} um filler. "))
            .collect::<Vec<_>>()
            .join("");
        let (out, status) = plan_cleanup(&state, &f, &[]).await;
        assert!(matches!(status, LlmCleanupStatus::Applied { .. }), "{status:?}");
        assert!(
            out.contains("sentence 0 um filler"),
            "first chunk (window 0) stayed raw"
        );
        assert!(!out.contains("sentence 19 um"), "later chunk shipped");
    }

    /// §7.3 deadline sealing: a per-item `TimedOut` costs only its own
    /// region; completed items in the SAME batch still ship.
    #[tokio::test]
    async fn per_item_timeout_ships_siblings_but_marks_region_raw() {
        struct Mixed {
            timeout_key: String,
        }
        impl LlmBackend for Mixed {
            fn cleanup_batch(&mut self, texts: &[String], _mode: CleanupMode) -> Result<Vec<BatchItem>, String> {
                Ok(texts
                    .iter()
                    .map(|t| {
                        if *t == self.timeout_key {
                            BatchItem::TimedOut
                        } else {
                            BatchItem::Proposal(Some(t.replace(" um ", " ")))
                        }
                    })
                    .collect())
            }
            fn request_raw(&mut self, _: &serde_json::Value) -> Result<serde_json::Value, String> {
                Err("no protocol requests expected".into())
            }
        }
        // Sentence A (3 words) is window 1's whole target (default targets
        // break at the first sentence boundary); the long sentence B forms
        // window 2's target. Time window 2 out: A ships, B stays raw — the
        // batch itself succeeded, the deadline costs exactly its own region.
        let f = "alpha um bravo. charlie um delta echo foxtrot golf hotel india juliet kilo lima mike november oscar papa quebec romeo sierra tango.";
        let windows = plan_windows(f, &CorrectionConfig::default());
        // Window 2 = the window whose TARGET holds "charlie". Both windows'
        // keys reach back to token 0 (left context overruns the start), so
        // neither source_start nor key-substring selection separates them —
        // the TARGET slice is the discriminator.
        let second = windows
            .iter()
            .find(|w| w.key[w.target_range.clone()].contains("charlie"))
            .expect("charlie window");
        let key_target = &second.key[second.target_range.clone()];
        assert!(key_target.contains("charlie"), "key target: {key_target:?}");
        let second = second.key.clone();
        let state = state_with_llm(Mixed { timeout_key: second });
        let (out, status) = plan_cleanup(&state, f, &[]).await;
        assert!(out.contains("alpha bravo."), "sibling applied: {out:?}");
        assert!(
            out.contains("charlie um delta"),
            "timed-out region raw: {out:?}"
        );
        assert!(matches!(status, LlmCleanupStatus::Applied { .. }), "{status:?}");
    }

    /// A per-item `Failed` code keeps its region raw and surfaces a typed
    /// status when NOTHING else applied (E1 final-diff precedence: failure
    /// outranks a would-be NoChanges).
    #[tokio::test]
    async fn per_item_failed_code_preserves_region() {
        struct AllFail;
        impl LlmBackend for AllFail {
            fn cleanup_batch(&mut self, texts: &[String], _mode: CleanupMode) -> Result<Vec<BatchItem>, String> {
                Ok(texts
                    .iter()
                    .map(|_| BatchItem::Failed("incomplete_generation".into()))
                    .collect())
            }
            fn request_raw(&mut self, _: &serde_json::Value) -> Result<serde_json::Value, String> {
                Err("no protocol requests expected".into())
            }
        }
        let state = state_with_llm(AllFail);
        let (out, status) = plan_cleanup(&state, "hello um world.", &[]).await;
        assert_eq!(out, "hello um world.");
        assert!(matches!(status, LlmCleanupStatus::Failed { .. }), "{status:?}");
    }

    /// Proposal-with-no-text (`BatchItem::Proposal(None)`) is a per-item
    /// failure, never an accidental full deletion (E1: `Some("")` ≠ `None`).
    #[tokio::test]
    async fn proposal_none_is_a_failure_not_an_empty_edit() {
        let state = state_with_llm(NoText);
        let (out, status) = plan_cleanup(&state, "hello um world.", &[]).await;
        assert_eq!(out, "hello um world.", "None never means erase-everything");
        assert!(matches!(status, LlmCleanupStatus::Failed { .. }), "{status:?}");
    }

    /// Whole-request transport failure: text preserved, status Failed, no
    /// partial application.
    #[tokio::test]
    async fn transport_failure_preserves_text_and_reports_status() {
        let state = state_with_llm(MockLlmBackend::failing("sidecar down"));
        let (out, status) = plan_cleanup(&state, "hello um world.", &[]).await;
        assert_eq!(out, "hello um world.");
        assert!(matches!(status, LlmCleanupStatus::Failed { .. }), "{status:?}");
    }

    /// Long dictation (>32 KB, the old whole-text refusal boundary): the
    /// planner processes window-by-window and applies every proposal's
    /// deletions, keeping everything else byte-exact.
    #[tokio::test]
    async fn long_dictation_is_planned_in_windows_not_refused() {
        let mut parts = Vec::new();
        for i in 0..900 {
            parts.push(format!("this is sentence number {i} um with a filler inside"));
        }
        let raw = parts.join(". ");
        assert!(raw.len() > 32_000);
        let state = state_with_llm(Stripper("um"));
        let (cleaned, status) = plan_cleanup(&state, &raw, &[]).await;
        assert!(
            matches!(status, LlmCleanupStatus::Applied { .. }),
            "long dictation must apply deletions, got {status:?}"
        );
        assert!(!cleaned.contains(" um "), "hesitations must be gone");
        assert!(cleaned.contains("sentence number 899"));
        assert!(cleaned.contains("sentence number 0 "));
        // Nothing else changed: word count drops by exactly the 900 fillers.
        assert_eq!(word_spans(&cleaned).len(), word_spans(&raw).len() - 900);
    }

    /// Busy guard is observable on the ACTUAL path: with the operation lock
    /// held (a load/other cleanup in flight), cleanup reports Unavailable
    /// and never touches the backend.
    #[tokio::test]
    async fn busy_operation_lock_short_circuits() {
        let state = state_with_llm(MockLlmBackend::failing("must not be called"));
        let guard = state.llm_operation.try_lock().expect("lock free before the test takes it");
        let (out, status) = plan_cleanup(&state, "hello um world.", &[]).await;
        drop(guard);
        assert_eq!(out, "hello um world.");
        assert!(matches!(status, LlmCleanupStatus::Unavailable { .. }), "{status:?}");
    }

    /// Director mask-edge regression: a caps-ONLY opportunity at a LATER
    /// sentence boundary (no hesitations, no repeats anywhere) must still
    /// arm the window and dispatch. `Alpha`/`Gamma` are already caps; only
    /// `epsilon` (start of sentence 3) is a legal flip. A boundary cursor
    /// that consumes only the FIRST boundary leaves the whole mask
    /// all-false ⇒ zero dispatch ⇒ the cap opportunity is silently lost.
    /// Asserted observably: dispatch count + delivered text + status.
    #[tokio::test]
    async fn caps_opportunity_at_a_later_sentence_boundary_dispatches() {
        struct Capting;
        impl LlmBackend for Capting {
            fn cleanup_batch(&mut self, texts: &[String], _mode: CleanupMode) -> Result<Vec<BatchItem>, String> {
                Ok(texts
                    .iter()
                    .map(|t| BatchItem::Proposal(Some(t.replace("epsilon", "Epsilon"))))
                    .collect())
            }
            fn request_raw(&mut self, _: &serde_json::Value) -> Result<serde_json::Value, String> {
                Err("no protocol requests expected".into())
            }
        }
        let f = "Alpha beta. Gamma delta. epsilon zeta.";
        let state = state_with_llm(Capting);
        let (out, status) = plan_cleanup(&state, f, &[]).await;
        assert_eq!(
            out, "Alpha beta. Gamma delta. Epsilon zeta.",
            "the later-boundary cap must compose from the reply"
        );
        assert!(matches!(status, LlmCleanupStatus::Applied { .. }), "{status:?}");
        let terms: Vec<String> = Vec::new();
        assert!(validate(f, &out, &terms).is_ok(), "frozen validator agrees");
    }

    /// §7.4 mask discipline, observed end-to-end: for adversarial small F's
    /// — fillers next to quotes, repeat blocks, and protected numerals — the
    /// DELIVERED text must always be admissible under the frozen validator
    /// (composition adjudication stays the sole authority), and any F whose
    /// every hesitation the frozen rules would accept deleting must have
    /// been DISPATCHED at least once — a wrongly-skipping mask would silently
    /// ask nothing and leave accepted deletions unsent.
    #[tokio::test]
    async fn mask_decisions_never_skip_fully_deletable_fillers() {
        let cases = [
            "alpha um bravo.",
            "said \u{201c}quote um inside\u{201d} and um after.",
            "the the um run.",
            "value 859 um dollars.",
            "um alpha.",
            "alpha um",
            "Please um, keep the whole final instruction intact.",
        ];
        struct Recorder {
            seen: Arc<AtomicUsize>,
        }
        impl LlmBackend for Recorder {
            fn cleanup_batch(&mut self, texts: &[String], _mode: CleanupMode) -> Result<Vec<BatchItem>, String> {
                self.seen.fetch_add(texts.len(), Ordering::SeqCst);
                Ok(texts
                    .iter()
                    .map(|t| {
                        BatchItem::Proposal(Some(
                            t.replace(" um ", " ").replace(" um, ", ", "),
                        ))
                    })
                    .collect())
            }
            fn request_raw(&mut self, _: &serde_json::Value) -> Result<serde_json::Value, String> {
                Err("none".into())
            }
        }
        for f in cases {
            let seen = Arc::new(AtomicUsize::new(0));
            let state = state_with_llm(Recorder {
                seen: Arc::clone(&seen),
            });
            let (out, _status) = plan_cleanup(&state, f, &[]).await;
            // Delivered text is always frozen-validator admissible (or raw).
            let terms: Vec<String> = Vec::new();
            assert!(
                out == f || validate(f, &out, &terms).is_ok(),
                "delivered text inadmissible for {f:?}: {out:?}"
            );
            // Baseline: would the frozen rules accept deleting EVERY
            // hesitation of this F? If yes, at least one window must have
            // been judged active and dispatched.
            let spans = word_spans(f);
            let mut flags = vec![false; spans.len()];
            for (j, s) in spans.iter().enumerate() {
                if matches!(f[s.clone()].to_lowercase().as_str(), "um" | "uh") {
                    flags[j] = true;
                }
            }
            let deletable = flags.iter().any(|&x| x)
                && validate_deletions(f, &flags, &terms).is_ok();
            if deletable {
                assert!(
                    seen.load(Ordering::SeqCst) >= 1,
                    "F with fully-deletable fillers never dispatched: {f:?}"
                );
            }
        }
    }
    /// Parser contract (ported from the benchmark's 24-test parse suite,
    /// EDITFMT/test_parse.py): exact semantics of `apply_edit_lines`.
    #[test]
    fn apply_edit_lines_matches_the_benchmark_parser() {
        let key = "I um need uh the the green notebook tomorrow.";
        // delete + fold, one line each
        assert_eq!(
            apply_edit_lines("um |||<D>\nuh the the|||the", key).as_deref(),
            Some("I need the green notebook tomorrow.")
        );
        // <KEEP> echoes the source verbatim
        assert_eq!(apply_edit_lines("<KEEP>", key).as_deref(), Some(key));
        // transcript tags are stripped around a valid script; `<D>` is a
        // byte deletion, so the surrounding spaces survive (whitespace
        // folding is the MODEL's job — fewshots show `uh the the|||the`).
        assert_eq!(
            apply_edit_lines("<transcript>\nuh|||<D>\n</transcript>", key).as_deref(),
            Some("I um need  the the green notebook tomorrow.")
        );
        // one delimiterless line poisons the WHOLE reply (fail-closed)
        assert_eq!(apply_edit_lines("um |||<D>\nI cleaned it up", key), None);
        // empty OLD is a no-op line, not a poisoner
        assert_eq!(apply_edit_lines("|||x", key).as_deref(), Some(key));
        // a find that matches nothing is a harmless no-op
        assert_eq!(
            apply_edit_lines("zebra|||horse", key).as_deref(),
            Some(key)
        );
        // edits apply sequentially: the second sees the FIRST's output,
        // and left-to-right non-overlapping scan re-reads from after each
        // replacement (a replaced span is never rescanned by the same edit).
        assert_eq!(
            apply_edit_lines("a b|||c d\nc d|||e f", "a b a b").as_deref(),
            Some("e f e f")
        );
        // every non-overlapping occurrence is replaced
        assert_eq!(
            apply_edit_lines("the the|||the", "the the the the").as_deref(),
            Some("the the")
        );
        // NEW is verbatim EXCEPT a trimmed `<D>` is the delete token
        assert_eq!(apply_edit_lines("a||| <D> ", "a").as_deref(), Some(""));
        assert_eq!(apply_edit_lines("a|||x<D>", "a").as_deref(), Some("x<D>"));
        // blank lines are skipped, not poisoners
        assert_eq!(
            apply_edit_lines("\na|||b\n\n", "a").as_deref(),
            Some("b")
        );
        // `<D>` is only recognized as the whole trimmed NEW
        assert_eq!(apply_edit_lines("a|||x<D>", "a").as_deref(), Some("x<D>"));
    }

    /// End-to-end Replace mode: a mock sidecar replying with edit SCRIPTS
    /// must yield exactly the same cleaned output Retype yields for the same
    /// intended edits; an unparseable reply keeps the region raw.
    #[tokio::test]
    async fn replace_mode_scripts_compose_and_fail_closed() {
        struct Scripter;
        impl LlmBackend for Scripter {
            fn cleanup_batch(
                &mut self,
                texts: &[String],
                _mode: CleanupMode,
            ) -> Result<Vec<BatchItem>, String> {
                // Half the replies are scripts, half are prose garbage.
                Ok(texts
                    .iter()
                    .enumerate()
                    .map(|(i, text)| {
                        let reply = if i % 2 == 0 {
                            let _ = text;
                            "um|||<D>".to_string()
                        } else {
                            "I cannot help with that.".to_string()
                        };
                        BatchItem::Proposal(Some(reply))
                    })
                    .collect())
            }
            fn request_raw(&mut self, _: &serde_json::Value) -> Result<serde_json::Value, String> {
                Err("none".into())
            }
        }
        let settings = Settings {
            llm_cleanup_mode: CleanupMode::Replace,
            ..Settings::default()
        };
        let state = AppState::new_with_backends(
            Box::new(MockAudioCapture::sine_wave()),
            Box::new(MockAsrEngine::with_text("unused")),
            Some(Box::new(Scripter)),
            Box::new(MockPasteBackend::new()),
            settings,
        );
        // A full 16-key batch: keys alternate clean-script replies (even
        // indices) and prose garbage (odd). Every key is one tiny window, so
        // per-KEY isolation is exactly the batch-index parity below.
        let f: String = (0..16)
            .map(|i| format!("w{i} um end. "))
            .collect::<Vec<_>>()
            .join("");
        let (out, status) = plan_cleanup(&state, &f, &[]).await;
        assert!(matches!(status, LlmCleanupStatus::Applied { .. }), "{status:?}");
        for i in 0..16 {
            if i % 2 == 0 {
                assert!(!out.contains(&format!("w{i} um")), "script {i} must apply: {out:?}");
            } else {
                assert!(out.contains(&format!("w{i} um")), "garbage {i} must stay raw: {out:?}");
            }
        }
        // Delivered text is always frozen-validator admissible.
        assert!(out == f || validate(&f, &out, &[]).is_ok(), "{out:?}");
    }

    /// Retype mode must be byte-identical to pre-0.10.0 behavior: the mode
    /// never reaches the proposal pipeline as a transform.
    #[test]
    fn settings_default_mode_is_retype() {
        assert_eq!(Settings::default().llm_cleanup_mode, CleanupMode::Retype);
        let mut value = serde_json::to_value(Settings::default()).unwrap();
        value.as_object_mut().unwrap().remove("llm_cleanup_mode");
        let restored: Settings = serde_json::from_value(value).unwrap();
        assert_eq!(restored.llm_cleanup_mode, CleanupMode::Retype);
        let mut value = serde_json::to_value(Settings::default()).unwrap();
        value["llm_cleanup_mode"] = serde_json::json!("replace");
        let switched: Settings = serde_json::from_value(value).unwrap();
        assert_eq!(switched.llm_cleanup_mode, CleanupMode::Replace);
        assert_ne!(Settings::default(), switched);
    }

    #[tokio::test]
    async fn in_flight_prewarm_hands_off_to_cleanup_instead_of_skipping_it() {
        // A recording-start prewarm holds llm_operation while its sentinel
        // generation runs. A stop arriving mid-prewarm must WAIT for the
        // handoff and then run the real cleanup — never silently preserve
        // raw text. The sentinel (prewarm text) passes through unchanged;
        // the cleanup window gets a valid caps-only edit.
        let backend = MockLlmBackend::run(|text| {
            if text == "Please um keep this readiness check local." {
                return Ok(text.to_string());
            }
            let mut chars = text.chars();
            let first = chars.next().unwrap_or(' ').to_uppercase().to_string();
            Ok(first + chars.as_str())
        });
        let state = std::sync::Arc::new(state_with_llm(backend));
        let holder = state.clone();
        let prewarming = tokio::spawn(async move {
            let _operation = holder.llm_operation.lock().await;
            holder.llm_prewarming.store(true, std::sync::atomic::Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        });
        // Deterministic rendezvous: proceed only once the flag is observable
        // (the setter runs before it ever takes the lock, so the lock being
        // held plus the flag set is the exact collision state).
        for _ in 0..2000 {
            if state.llm_prewarming.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(state.llm_prewarming.load(std::sync::atomic::Ordering::SeqCst));
        let (out, status) = plan_cleanup(&state, "hello there", &[]).await;
        assert!(
            matches!(status, LlmCleanupStatus::Applied { .. }),
            "handoff must yield a real cleanup, got {status:?}"
        );
        assert_eq!(out, "Hello there");
        prewarming.await.unwrap();
    }

    #[tokio::test]
    async fn expired_prewarm_handoff_falls_back_to_busy_unavailable() {
        // Same collision, but the prewarm outlives the handoff budget: the
        // cleanup answers `None` (→ busy-Unavailable, raw preserved — the
        // existing branch, already covered). `handoff_wait` is a parameter
        // precisely so this expiry runs in milliseconds, not 10 s.
        let state = std::sync::Arc::new(state_with_llm(MockLlmBackend::passthrough()));
        let holder = state.clone();
        let prewarming = tokio::spawn(async move {
            let _operation = holder.llm_operation.lock().await;
            holder.llm_prewarming.store(true, std::sync::atomic::Ordering::SeqCst);
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        });
        for _ in 0..2000 {
            if state.llm_prewarming.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            acquire_cleanup_operation(&state, std::time::Duration::from_millis(50)).await.is_none(),
            "handoff budget must expire while the prewarm still holds the permit"
        );
        // Control: within budget, the same wait acquires the permit.
        assert!(
            acquire_cleanup_operation(&state, std::time::Duration::from_secs(2)).await.is_some(),
            "handoff must succeed once the prewarm drops the permit"
        );
        prewarming.await.unwrap();
    }
}
