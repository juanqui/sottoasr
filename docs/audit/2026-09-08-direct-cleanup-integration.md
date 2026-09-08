# Direct cleanup integration audit

- **Version:** 1.1
- **Date:** 2026-09-08
- **Status:** In Review

## Contents

1. Outcome and scope
2. Current delivery and protocol
3. Resource bounds and model identity
4. Status and provenance
5. Required verification
6. Ordered implementation tasks
7. Audit verification
8. MiniCPM sidecar integration supplement

## 1. Outcome and scope

Automatic cleanup can use the existing local sidecar, lifecycle ownership,
paste/copy flow and History schema. The essential change is to receive a complete
text proposal, validate source-derived edits in Rust, and assign the accepted
result to ordinary output. Current callers retain the proposal separately and
continue delivering the ASR/dictionary text.

This is a read-only implementation audit supporting the
[automatic cleanup recovery plan](../specs/2026-09-08-automatic-cleanup-recovery.md).
It does not select a replacement model, qualify a validator, or authorize bypassing
the plan's quality gates. No product, model configuration, cache location or
installed application was changed for this audit.

Source pointers below identify the working tree inspected on September 8, 2026;
they are line pointers for that snapshot, not a claim that the code is committed.

## 2. Current delivery and protocol

| Source pointer | Verified current behavior | Minimum integration work |
|---|---|---|
| [src-tauri/sidecar/llm_cleanup.py:126](../../src-tauri/sidecar/llm_cleanup.py#L126) | The prompt contains the transcript and numbered deletion candidates. | Use the qualified native direct-text prompt and return a bounded, completed text proposal. |
| [src-tauri/sidecar/llm_cleanup.py:163](../../src-tauri/sidecar/llm_cleanup.py#L163) | The cleanup response contains `delete_ids` and elapsed time. | Change the cleanup response contract together with its Rust consumer; retain structured errors and transcript-free diagnostics. |
| [src-tauri/src/llm/engine.rs:23](../../src-tauri/src/llm/engine.rs#L23) | `LlmBackend::cleanup` accepts `DeletionCandidate` values and returns `Vec<usize>`. The trait is declared in this file, not a separate backend module. | Return a text proposal through the typed interface. Update the production implementation at [line 352](../../src-tauri/src/llm/engine.rs#L352) in the same change. |
| [src-tauri/src/llm/edits.rs:85](../../src-tauri/src/llm/edits.rs#L85) | The detector permits a narrow list of hesitation and adjacent function-word deletions. Source byte ranges stay in Rust. | Replace candidate enumeration as the model interface with bounded alignment and the qualified edit/protection policy. Keep source byte positions and reconstruction under Rust ownership. |
| [src-tauri/src/llm/cleanup.rs:42](../../src-tauri/src/llm/cleanup.rs#L42) | The helper skips inputs under five words or without old detector candidates, then owns lifecycle locking, inference and failure recovery. Valid deletions return `Suggested`. | Remove obsolete detector-based coverage exclusions, validate the returned proposal, and return accepted output or the complete unchanged cleanup input. Preserve lifecycle locking and owned-process recovery. |
| [src-tauri/src/hotkeys/manager.rs:466](../../src-tauri/src/hotkeys/manager.rs#L466) | Production stores the returned text in `cleanup_suggestion`; `final_text` remains unchanged and `llm_applied` is false at [line 522](../../src-tauri/src/hotkeys/manager.rs#L522). | Assign accepted reconstructed text to `final_text`, set application status truthfully, and let the existing delivery at [line 560](../../src-tauri/src/hotkeys/manager.rs#L560) use it. |
| [src-tauri/src/pipeline.rs:135](../../src-tauri/src/pipeline.rs#L135) | The testable pipeline mirrors the suggestion-only behavior; delivery uses `final_text` at [line 209](../../src-tauri/src/pipeline.rs#L209). | Update this path with production behavior so tests exercise accepted automatic output rather than a divergent mirror. |

Pass any protected vocabulary/dictionary terms to validation from the same
immutable processing snapshot used for that transcript. Exact saved terms can be
protected deterministically; this does not imply automatic recognition of every
unlisted name. No audio capture, ASR model, recording lifecycle or clipboard
backend redesign is required for this integration.

## 3. Resource bounds and model identity

### 3.1 Deadlines and Unicode response size

The engine's [request timeout](../../src-tauri/src/llm/engine.rs#L57) is **10 seconds**
for cleanup, including the bounded request write and response wait implemented at
[engine.rs:186](../../src-tauri/src/llm/engine.rs#L186). The shared helper separately
uses a **30-second** outer deadline at
[cleanup.rs:25](../../src-tauri/src/llm/cleanup.rs#L25). Increasing generation tokens
or the outer deadline alone does not increase the wire request's allowed time.
Set these bounds coherently using the selected model's measured behavior.

The response reader is capped at **64 KiB** by
[engine.rs:55](../../src-tauri/src/llm/engine.rs#L55). The sidecar accepts **16,000
characters** at [llm_cleanup.py:18](../../src-tauri/sidecar/llm_cleanup.py#L18) and
serializes responses with default `json.dumps` escaping at
[line 204](../../src-tauri/sidecar/llm_cleanup.py#L204). A direct response containing
16,000 non-ASCII BMP characters can require 96,000 bytes for the escaped text
alone; non-BMP characters can require 192,000 bytes. The current small ID response
does not have this full-transcript expansion risk.

Using UTF-8 JSON with `ensure_ascii=False` reduces escape expansion, but does not
by itself prove the response fits: 16,000 four-byte characters consume 64,000
bytes before the envelope, and control-character escaping can add overhead.
Coordinate accepted input size, formatted prompt/context limits, final-output
token allowance and serialized response bytes. Retain a finite response cap and
fail to the complete input on oversize, malformed or unfinished output.

The current generation budget at
[llm_cleanup.py:153](../../src-tauri/sidecar/llm_cleanup.py#L153) is based on candidate
count, from 32 to 1,024 tokens. Direct output needs an input-token-based final
answer allowance; a native reasoning model also needs its separately measured
reasoning allowance and final-answer boundary handling. Do not report truncated
reasoning as a semantic cleanup failure.

### 3.2 Model, runtime and packaged artifacts

| Source pointer | Identity or packaging concern |
|---|---|
| [src-tauri/src/llm/engine.rs:849](../../src-tauri/src/llm/engine.rs#L849) | Rust's `SOTTO_MODEL` specifies the model ID, display name and download size. |
| [src-tauri/sidecar/llm_cleanup.py:14](../../src-tauri/sidecar/llm_cleanup.py#L14) | Python separately specifies `MODEL_ID` and `MODEL_NAME`; both layers must identify the same qualified artifact. |
| [src-tauri/src/commands/llm.rs:19](../../src-tauri/src/commands/llm.rs#L19) | Settings status takes its model identity and size from the Rust configuration. Changing only Python would leave misleading status and readiness behavior. |
| [src-tauri/src/llm/engine.rs:526](../../src-tauri/src/llm/engine.rs#L526) | Rust readiness follows the configured model's existing Hugging Face cache and `refs/main`; Python's offline lookup is at [llm_cleanup.py:44](../../src-tauri/sidecar/llm_cleanup.py#L44). Verify the actual revision selected by both sides, not merely equal repository names. |
| [src-tauri/src/llm/engine.rs:420](../../src-tauri/src/llm/engine.rs#L420) | Minimum runtime version and pinned packages are defined here; Python also checks its minimum at [llm_cleanup.py:16](../../src-tauri/sidecar/llm_cleanup.py#L16). A selected custom architecture must be supported by the prepared runtime. |
| [src-tauri/tauri.conf.json:35](../../src-tauri/tauri.conf.json#L35) | The configured sidecar resource is only `sidecar/llm_cleanup.py`. If the selected implementation imports an additional local adapter, bundle that dependency or install its verified pinned package explicitly. A development checkout import does not establish packaged-app readiness. |
| [src-tauri/src/llm/engine.rs:298](../../src-tauri/src/llm/engine.rs#L298) | Sidecar path discovery handles packaged and development locations. Verify the actual bundled resource after building. |
| [src-tauri/src/commands/llm.rs:53](../../src-tauri/src/commands/llm.rs#L53) | Explicit preparation already downloads and verifies loading under lifecycle ownership. Reuse this flow for the qualified artifact. |
| [src-tauri/src/llm/download.rs:9](../../src-tauri/src/llm/download.rs#L9) | Download uses the model configuration and sidecar. Preserve existing cached models during migration; the explicit delete operation at [line 62](../../src-tauri/src/llm/download.rs#L62) is not a migration step. |

Keep the measured model revision, precision, runtime and any adapter identity in
the qualification record. Verify the installed runtime against those identities
before marking it ready. Model preparation must remain explicit and local
inference must not install packages or download weights implicitly.

## 4. Status and provenance

The existing [Rust status enum](../../src-tauri/src/models.rs#L23) and
[TypeScript union](../../src/lib/utils/tauri.ts#L9) already include `Applied`,
`NoChanges`, `Failed`, `Unavailable` and `TimedOut`. `Applied` can represent new
validated automatic cleanup; update its legacy-only comment. Identical model
output supports “No changes suggested.” Validation failure can use the existing
failure reason without requiring a new enum solely for this integration.

Retain deserialization and display support for `Suggested`, `SkippedTooShort` and
`SkippedNoCandidates` in old history. Do not silently relabel historical detector
bypasses as successful model judgments. If a new outcome is needed after the
validator contract is settled, update Rust serialization, TypeScript and all
exhaustive UI matches together.

The existing [Transcription fields](../../src-tauri/src/models.rs#L48) can represent
the minimum automatic flow:

- `text`: the actual delivered, reconstructed output.
- `raw_text`: original ASR, retained when processing changes it.
- `llm_applied`: whether accepted cleanup changed the cleanup input.
- `cleanup_suggestion`: retained for old suggestion records; new automatic records need not populate it.

An original-to-delivered diff includes vocabulary/dictionary changes as well as
AI deletions. If an AI-only diff is required, an optional cleanup-input field must
retain the pre-cleanup text. That is an additional product decision, not a
prerequisite for automatic delivery. Such a field must be added compatibly to
[Transcription equality](../../src-tauri/src/models.rs#L223), TypeScript,
serialization tests and [CSV export](../../src-tauri/src/commands/transcription.rs#L63),
using the existing formula-safe [CSV cell encoder](../../src-tauri/src/commands/transcription.rs#L10).

History already computes the original/output diff at
[history-item.svelte:34](../../src/lib/components/history-item.svelte#L34), displays
“AI Cleaned” at [line 127](../../src/lib/components/history-item.svelte#L127), and
copies ordinary `item.text` at [line 92](../../src/lib/components/history-item.svelte#L92).
The overlay already maps `applied` to “Cleaned” at
[overlay-pill.svelte:191](../../src/lib/components/overlay-pill.svelte#L191).
Keep the separate legacy suggestion component readable.

Update suggestion-only copy in
[settings-dictation.svelte:14](../../src/lib/components/settings-dictation.svelte#L14),
the ready notice in
[cleanup-setup.svelte.ts:4](../../src/lib/stores/cleanup-setup.svelte.ts#L4),
and model/license attribution in
[about-view.svelte:21](../../src/lib/components/about-view.svelte#L21).
Preserve the saved enable preference and the existing default-off value at
[models.rs:191](../../src-tauri/src/models.rs#L191).

## 5. Required verification

The shared mock currently returns deletion IDs:
[test_support.rs:156](../../src-tauri/src/test_support.rs#L156) and its trait
implementation at [line 190](../../src-tauri/src/test_support.rs#L190).
Two lifecycle-test backends in
[cleanup.rs:253](../../src-tauri/src/llm/cleanup.rs#L253) and
[cleanup.rs:281](../../src-tauri/src/llm/cleanup.rs#L281) implement the same signature.
Migrate these with the protocol; preserve their ownership, concurrency and panic
coverage instead of replacing them with superficial output assertions.

Verification should cover:

- Valid model proposals change the actual pasted/copied text, saved `text`, word count and `llm_applied`; original ASR remains recoverable.
- Identical, invalid, unsupported, malformed, oversized and incomplete proposals deliver the complete cleanup input and an accurate status.
- Dictionary/vocabulary processing followed by cleanup preserves provenance and exact protected terms.
- Capitalized, short, terminal and repeated-phrase cases reach the new policy instead of old detector exclusions.
- UTF-8 byte boundaries, JSON escaping near the response cap, multiple equivalent repeat alignments and tail preservation have focused structural tests.
- Sidecar death, setup overlap, cancellation, capture interruption and stale jobs retain the existing recovery and no-partial-paste behavior.
- Older history with missing optional fields or legacy suggestions still loads; ordinary Copy uses delivered text and suggestion Copy remains explicit for old records.
- Settings readiness and the built bundle identify and load the same qualified model/runtime/adapter that was measured.

Use the plan's independent semantic qualification for model usefulness and
preservation. Mocked integration tests cannot establish those model properties.

## 6. Ordered implementation tasks

1. Qualify and freeze the model, direct prompt, decoding mode, runtime and validator policy under the reviewed plan.
2. Set coherent context, generation, request-time and serialized-response bounds; define the typed text-proposal contract.
3. Update the sidecar, Rust trait/consumer and mock backends together; retain lifecycle ownership and bounded failure recovery.
4. Implement source alignment/reconstruction and replace the old candidate/short-input gate with the qualified policy.
5. Update both production and testable pipelines to deliver accepted text, preserve originals and set actual application status.
6. Update model/status identity and any required packaged runtime resources; preserve caches and saved settings.
7. Update automatic-cleanup wording and compatible history/status handling, adding new provenance only if required by the intended UI.
8. Run focused protocol/validator/integration checks, then the required application build, linter and test checks once on the completed change; capture output with `tee`.
9. Inspect the final bundled sidecar, runtime and model identity and perform the planned local microphone/paste verification before installation.

## 7. Audit verification

This note was checked against current source declarations, callers, serializer,
mock implementations and bundle configuration. The Unicode sizes above are
derived JSON-encoding bounds, not measured model failures. No inference, product
build, test suite, cache mutation or product/spec edit was performed for this
documentation task. The final note received a Markdown whitespace/link-target
check; its implementation tasks remain pending model qualification and review.

## 8. MiniCPM sidecar integration supplement

This September 8 supplement prepares integration of the selected development
candidate, D3 with official MiniCPM5-2B. It does not claim that release qualification
has passed. Product edits remain blocked on the coordinating agent's release of
the reviewed implementation phase; no model files or live cache locations changed.

### 8.1 Exact artifact and preparation

| Item | Required production value |
|---|---|
| Repository | `openbmb/MiniCPM5-2B-MLX` |
| Revision | `32f8dd5df1188512a20413f1297083238306634c` |
| Precision/runtime architecture | Affine 4-bit, group size 64; built-in `mlx_lm.models.llama`, no custom adapter |
| Model context | `131072` tokens from the pinned config; this is not the product input allowance |
| Model EOS IDs | `1` and `130073`; preserve the loader's complete EOS set |
| Weight file | `model.safetensors`, 1,416,035,216 bytes |
| Weight SHA-256 | `c207798696a4a454e7ac211b25227625466c693335941cee8904fb922f295cc1` from the existing download's LFS metadata; direct full-file verification belongs to preparation |
| `config.json` SHA-256 | `deb9ca33e863cbc84a9ab7209cd924fc05505dc33270c807d47f1b78fbd53a50` |
| `tokenizer.json` SHA-256 | `3e065a558a034185fe299917b398685c1facd0169a9eea1e629eb30c171fed81` |
| `tokenizer_config.json` SHA-256 | `b89503c3e5070c6b6d33daf2e20cb4a5c88537c1670d9b7e0cfb4506a61448a9` |
| `chat_template.jinja` SHA-256 | `cc945752db555d60949b16989df4ccfeb52a313d6b4b5c5229dd786e2e9fcf1c` |
| `generation_config.json` SHA-256 | `9ac4f32e5f32358697a9f438a3ea89ef80e6ba786c72c49e932f9f21c122fdb1` |
| Weight-index SHA-256 | `ccf202e0a06fe3c7eb8f354cfb29412a5e64956ad895413d4d9267ae4b3a6045` |
| Runtime pins | `mlx==0.32.2`, `mlx-lm==0.31.3`, `transformers==5.3.0`, `huggingface-hub==1.7.2` |
| Measured harness memory settings | MLX memory limit 4 GiB, allocator cache limit 128 MiB; do not inherit the old sidecar's 2 GiB limit |

The small-file hashes above were computed from the existing isolated snapshot;
its download manifest records 1,426,119,249 bytes across downloaded artifacts.
Preparation must verify the full weight hash once and record the qualified
revision, required files, sizes and verification result. Startup performs cheap
complete-manifest checks, not repeated 1.4 GB hashing on every request.

Use `snapshot_download(MODEL_ID, revision=MODEL_REVISION, token=False)` only during
explicit preparation, and the same revision with `local_files_only=True` for
runtime resolution. Rust readiness must inspect that pinned snapshot rather than
`refs/main`; Python and Rust model constants must match in a focused test. Require
the separate chat template: this tokenizer config does not embed one.

Keep the existing Hugging Face cache location and old model data. A missing,
incomplete or invalid snapshot produces a setup failure without deleting it.
Download/update repairs or installs the qualified pin; a changed remote `main`
must not replace it or create an unfulfillable "update available" loop. No ASR
cache or model path changes are part of this work.

### 8.2 Prompt, completion and bounded protocol

The selected [D3 prompt](../../benchmarks/llm/model-study-2026-09-08/direct-cleanup-diagnostic/development/prompt-inline.json)
has SHA-256 `ffe72a3812d42fb9f455e428f62ff300d232ace46acdf668e6d012425e7d48e5`;
its [native-layout runner](../../benchmarks/llm/model-study-2026-09-08/direct-cleanup-diagnostic/development/run_inline.py)
has SHA-256 `9db3ef16bb93d50bf77e63ad2d316009f2d0d5df546fb81ea4467eb3b27c15ee`.
Copy the exact system prompt, examples and data-envelope strings. D3 places all
three examples inside the **system message** under `<examples>`, followed by one
user transcript; converting them to separate user/assistant turns changes the
tested prompt. Preserve the runner's separators and newlines. Render the model's native template with
`add_generation_prompt=True`, `tokenize=False`, `enable_thinking=False`, then use
the same string-prompt tokenization as the tested harness. The native template
already adds BOS and a closed empty thinking block; do not add another BOS or
invent a generated reasoning delimiter parser for this non-thinking mode.

The development runner uses greedy sampling, seed 42, no repetition penalty, and
`min(8192, max(128, 2 * input_tokens + 32))` output tokens, where input tokens use
the tested tokenizer's `encode(raw)` behavior. Copy the final frozen configuration
after qualification. Check full formatted prompt tokens plus reserved output
against context before generation; never truncate the input or silently clip a
budget that cannot reproduce its source. Preserve the measured seed, memory
limits and completion policy when moving into the persistent process.

Recommended minimal wire contract:

```json
{"action":"cleanup","text":"the complete cleanup input"}
{"ok":true,"text":"the complete untrusted proposal","finish_reason":"stop","elapsed_ms":123}
{"ok":false,"error_code":"incomplete_generation","error":"Cleanup did not finish; original text preserved"}
```

Rust's typed `cleanup(&str)` should return the proposal only after checking a
successful response, string text and `finish_reason == "stop"`. It then passes the
proposal to the independently qualified Rust validator. Python accumulates the
complete stream, including its final detokenizer segment; `length`, missing final
response, exceptions and deadline expiry return errors with no partial proposal.
An empty completed proposal remains distinguishable from an absent or incomplete
response; only Rust validation can authorize deleting an all-filler input.

Keep a bounded 10-second warm-request deadline if qualification retains that value;
this includes prompt preparation and generation, as in the development runner.
Allow explicit protocol overhead outside it and keep the outer 30-second owned
process recovery as the final ceiling. The current equal 10-second sidecar wire
allowance can race the warm-request deadline, so coordinate a slightly larger wire
allowance with Rust rather than silently changing the measured generation budget.
The outer deadline includes cold startup; timeout always preserves the complete
input and terminates the owned process before another request can reuse it.

Retain the 16,000-character input bound unless qualification changes it, count it
consistently in Python and Rust, and enforce explicit UTF-8/serialized byte bounds.
A conservative concrete wire ceiling is 256 KiB per JSON line, including newline,
with at most 32,000 proposal characters; `ensure_ascii=False` still escapes some
controls, so check the actual serialized byte length before writing. Those bounds
accommodate the source's worst JSON expansion without unbounded output. Oversize
requests, token/context overflow and oversized proposals fail to unchanged text.

Emit compact UTF-8 JSON with one newline and flush each response. The input loop
should have the same finite line bound and reject oversized or non-object input
without consuming unbounded memory. Use stable, transcript-free public error
messages; arbitrary exception strings may contain input or generated text despite
the existing no-log comment. Keep stdout exclusively for protocol responses.

### 8.3 Persistent cache and verification boundary

Initially retain the existing single process and model residency, with an
independent request cache. Static-prefix caching is a later optimization only if
the [cache-equivalence experiment](../research/2026-09-08-mlx-cleanup-performance.md)
passes. The persistent baseline may contain only shipped system/examples/template
tokens; clone it for each request, verify an exact token prefix of the complete
native prompt, and discard the clone after completion, error or cancellation.
Never save a transcript-derived cache or trim a previous request back for reuse.

Sidecar ownership is limited to `sidecar/llm_cleanup.py` and its protocol tests;
the root integration owns Rust protocol/model metadata/deadlines and delivery,
and the validator owner owns source reconstruction. MiniCPM needs no extra local
adapter, so the existing bundled Python resource is sufficient if prompt/model
constants remain in that file. If a separate prompt or manifest file is chosen,
add it explicitly to Tauri resources and verify the actual packaged copy.

Focused sidecar tests should cover exact frozen prompt/template arguments,
complete versus truncated and empty output, local-only pinned snapshot selection,
missing chat template, preparation hash mismatch without cache deletion, near-cap
Unicode/control JSON, bounded input lines, and transcript-free exceptions. The
integrated bundle smoke must exercise a real completed proposal through Rust;
mock tests alone cannot establish native generation equivalence. No inference or
tests were run for this read-only integration supplement.
