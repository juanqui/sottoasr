# Bare hesitation follow-up

- **Version:** 1.0
- **Date:** 2026-09-08
- **Status:** Implemented

## Contents

- [Fixed regression comparison](#fixed-regression-comparison)
- [Independent validation](#independent-validation)
- [Exact production representation](#exact-production-representation)
- [Reproduction](#reproduction)

The reported sentence had three bare `um` tokens and one `uh`. The comma-only guard returned before inference. This bounded follow-up compares the anticipated optional-comma guard with the same stock LFM2.5-350M model/runtime, three prompt variants and the previous prompt. It improves review-suggestion coverage; it does not establish automatic-cleanup safety or fully clean the reported sentence.

## Fixed regression comparison

`comparison-cases.json` contains the supplied test sentence followed by the unchanged prior 96 synthetic cases. Prompt selection used this known regression set. All four conditions have 72 eligible cases among those 96; the supplied sentence is also eligible. Gold labels were not changed to accommodate expanded eligibility.

| Prompt | Exact /96 | Eligible exact /72 | Harmful /96 | Safe useful edits /40 | Preserves unchanged /56 | Median eligible ms | Reported sentence IDs |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| Previous production | 56 | 32 | 33 | 31 | 29 | 82.1 | `[0]` |
| **Occurrence instruction** | **60** | **36** | **31** | **37** | **27** | **99.7** | `[0]` |
| Balanced examples | 57 | 33 | 36 | 31 | 28 | 150.5 | `[0,1,2,3]` |
| Repeated-occurrence example | 60 | 36 | 31 | 37 | 27 | 101.0 | `[0]` |

The occurrence instruction was selected for better useful editing on the broader regression set. The balanced variant removes all four reported fillers but increases harmful deletions without increasing useful edit coverage. No further variants were tried after the third variant.

## Independent validation

A separate agent authored and froze 16 cases before receiving the selected prompt. Its original SHA256 is `4d543330e45adb7d12ef8015ff39e10a35fd7eacca7f85eba8d6993ee8d68b6f`. The selected prompt was frozen as `frozen-occurrences.json`, SHA256 `544917c179c0a94d13244d73eb72de2fad20b14b809f3d9587c61ffdbe9f2f9d`, before this data was opened. No tuning followed its results.

| Metric | Previous prompt | Selected occurrence prompt |
| --- | ---: | ---: |
| Exact /16 | 8 | 8 |
| Eligible exact /11 | 5 | 5 |
| Harmful cases | 5 | 5 |
| Safe useful edits /8 | 6 | 6 |
| Preservation cases unchanged /8 | 3 | 3 |
| Malformed/deadline fallbacks | 0 | 0 |
| Median eligible ms | 82.6 | 98.4 |

Five cases bypass inference, including two required edits outside the existing guards. The independent result shows no quality improvement or regression, and it still fails the automatic preservation gate. A separate normalized fixture adds `raw` equal to the author's `text` field; original inputs and gold strings remain identical. Initial harness attempts failed on this schema difference before any case inference.

## Exact production representation

The first manual probe built candidate dictionaries in `id,text,kind` property order and selected `[3]`. Actual Rust serialization uses `id,kind,text`; the production-ordered baseline and selected prompt both select `[0]`. The model therefore changes its choice with a semantically equivalent JSON representation. `wire-order.json` preserves this distinction. All table results use Rust-produced candidates. The integrated `production-reported-sentence.json` confirms `[0]` with the current production Rust adapter, updated sidecar and installed runtime in 133ms.

With the selected prompt, the reported sentence becomes:

> This is a test to see if this can um remove all the um uh yeah, those things from the sentences.

Only the first `um` is removed. The remaining fillers and `yeah,` stay unchanged. Production prompt messages and chat-template options were compared exactly with the frozen experiment before adoption; model identity, runtime, parser and generation budget are unchanged.

## Reproduction

Use the parent study's isolated Python requirements and pinned `liquid350` artifact. All runs take local weights and offline inference; no download is performed by this harness.

```bash
cargo build --locked --manifest-path benchmarks/llm/model-study-2026-09-08/bare-hesitation-followup/adapter/Cargo.toml

HF_HUB_OFFLINE=1 HF_HUB_DISABLE_IMPLICIT_TOKEN=1 python benchmarks/llm/model-study-2026-09-08/bare-hesitation-followup/compare.py \
  --model-path /absolute/path/to/local/stock350 --label stock350 \
  --prompt-variant occurrences \
  --dataset benchmarks/llm/model-study-2026-09-08/bare-hesitation-followup/heldout16-normalized.json \
  --output /tmp/occurrence-independent16.json
```

Use `production` for the prior prompt. Omit `--dataset` for the known 97-case comparison. Per-case results omit duplicate input/gold/candidate fields; join IDs to their named fixture and reconstruct candidates with the archived adapter. The adapter snapshots the old guard with only its comma requirement removed; historical comments are retained in that frozen snapshot. Artifact hashes, provenance and source-path normalization are in `manifest.json`.
