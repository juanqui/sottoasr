# Source validator v6 development evidence

- **Version:** 6.0
- **Date:** 2026-09-08
- **Status:** In Review

Contents: [Scope](#1-scope), [Policy](#2-policy), [Results](#3-results), [Reproduction](#4-reproduction), [Limits](#5-limits).

## 1. Scope

This is an experimental source-preserving validator, with no production changes or model inference. V6 follows the failed v5 qualification: the original 250 cases and 12 restarts are now revealed development data. They cannot qualify this revision for release. The newly authored `qualification2` datasets were not opened or used.

The exact [policy](policy.md) was frozen and independently approved before implementation. Source SHA256 is `4a8bc2a49feb6b944e765ec238f7bdc381a65576907d1b8ddddf9a4b172b177b`; the policy SHA is `8860544659001771994b45ba4ac1cfecdc27dac96e683dd148067d865c3f70df`. Source and policy were frozen before these evaluations. Earlier validator archives remain unchanged.

## 2. Policy

The [v5-to-v6 patch](v5-to-v6.patch) changes five reviewed rules:

1. Admit adjacent single-word repetitions beyond the old function-word list. A small explicit emphasis/affirmation/attention exclusion applies only to single-word repetitions; it does not block eligible multiword repeats. This is an eligibility rule, not evidence that every admitted repeat is accidental.
2. Protect capitalized words following a listed honorific or uppercase single-letter initial and a period. An ordinary sentence-ending period still permits a leading hesitation in the next sentence.
3. Replace the broad whole-clause `label` lock with bounded literal payload syntax; extend that syntax to parameters and tags, including explicit `starts/ends in/with` payloads. Other inherited word/name whole-clause locks remain.
4. Allow retained protected words such as `not` to receive the existing narrow ASCII sentence-initial capitalization. Such words remain undeletable, and exact quote/code/identifier/dictionary span checks still run first.
5. When multiple valid source reconstructions exist, accept only the unique one matching the proposal's comma locations. Punctuation still comes from the source. If multiple reconstructions match, reject the proposal.

The v5 quote/code scanner, byte and alignment limits, exact protected spans, and source reconstruction rules remain intact. No language detector is included; the parent's separately measured language admission rule is outside this artifact.

## 3. Results

All 28 authored test groups passed under Python 3.11.14/Unicode 14 and Python 3.14.5/Unicode 16. The latter interpreter also ran the offline comparison. All 2,540 UTF-8 reconstruction checks passed for 1,270 proposals across 21 profiles. Sixty-four delivered outputs changed relative to v5, with **zero newly detected harmful outputs and zero lost complete-cleanup cases** on these revealed data.

The profiles include all eight original model/prompt profiles on semantic60; MiniCPM D1, D2, D3 and D4 on the old9 plus semantic60; D3's original250 and restart12; and seven completed pilot36 model/prompt controls. Repeated cases across profiles are repeated observations, not 1,270 independent examples. [inputs.json](inputs.json) explicitly lists and hashes every input; no directory discovery is used by the evaluator.

| Revealed dataset and profile | Complete cleanup: raw / v5 / v6 | Preserved cases: raw / v5 / v6 | V6 remaining detected harm |
|---|---:|---:|---|
| D3 original250 | 112 / 102 / 111 of 120 | 116 / 128 / 129 of 130 | German `Um`, release242 |
| D3 original restart12 | 1 / 1 / 1 of 6 | 3 / 6 / 6 of 6 | None detected |
| Pilot36 D3 | 12 / 7 / 12 of 22 | 7 / 12 / 13 of 14 | German `Um` |
| Pilot36 D5 | 14 / 9 / 14 of 22 | 8 / 12 / 13 of 14 | German `Um` |
| Pilot36 D6 | 14 / 10 / 14 of 22 | 9 / 13 / 14 of 14 | None detected |
| Pilot36 D7 | 16 / 12 / 16 of 22 | 10 / 13 / 13 of 14 | German `Um` |
| Pilot36 Qwen4 D5 | 17 / 11 / 17 of 22 | 7 / 12 / 13 of 14 | German `Um` |
| Pilot36 Spark D5 | 14 / 8 / 14 of 22 | 11 / 13 / 13 of 14 | Literal `uh`, restart11 |
| Pilot36 Pollard D5 | 14 / 10 / 14 of 22 | 8 / 12 / 13 of 14 | German `Um` |

Original250 ordinary cleanup improves from 90/100 to 99/100, with 100/100 ordinary preservation. Adversarial cleanup remains 12/20 complete; the unsafe surname, tag suffix and parameter deletions are now rejected, leaving the original transcript. The guard cannot repair an incorrect model proposal, so the original restart subset stays at 1/6 complete cleanup.

All eight original semantic60 profiles and four old69 development profiles have identical delivered outputs under v5 and v6. The six original MiniCPM/Pollard/Spark/Qwen profiles have no detected delivered harm. Both Liquid references still delete grammatical `had had` in semantic31; this was already permitted by v5. These negative controls expose the guard's dependence on the model's contextual judgment.

### Scoring qualifications

Complete cleanup requires every expected lexical edit, with no protected-span failure; deleting some fillers is not enough. Preservation uses ordered source-word alignment plus separately authored exact protected-span annotations where available. Per-case artifacts retain raw output, both reconstructed outputs, deletion offsets, reason, primary-reference span metrics, and the accepted-reference metrics.

The reported example has an explicitly accepted shorter alternative. A primary-reference-only span score incorrectly calls that alternative a required-word loss. This evaluator preserves the primary score for inspection but determines harm against the union of the primary and all authored alternatives, then checks exact protected spans independently. Accepted-reference span statistics select an exact lexical reference first, then a preservation-valid one; the selected index is recorded. This reporting correction changes no validator decisions.

Old9 cases do not have an independently authored exact protected-span companion; their lexical diagnostics are not presented as complete protected-string verification. Model outputs must have completed normally to receive raw quality credit. Source fallback is always scored as delivered text, even if the model request failed. There are no new model latency or hardware performance claims from this offline work.

## 4. Reproduction

From the repository root, use a Python 3.14 interpreter (Unicode 16 for the qualification runtime). The code is standard-library-only; do not install model packages or download weights.

```bash
PYTHONDONTWRITEBYTECODE=1 python3.14 benchmarks/llm/model-study-2026-09-08/direct-cleanup-diagnostic/validation-prototype-v6/source_validation.py 2>&1 | tee /tmp/source-validator-v6-tests.txt
PYTHONDONTWRITEBYTECODE=1 python3.14 benchmarks/llm/model-study-2026-09-08/direct-cleanup-diagnostic/validation-prototype-v6/evaluate.py 2>&1 | tee /tmp/source-validator-v6-evaluate.txt
```

The evaluator asserts source, predecessor and input hashes before use. Its only validator inputs are source text, completed model text, an empty user dictionary, and generation completion; neither reference outputs nor scoring annotations are passed to the validator. Every accepted result is independently reconstructed from the original UTF-8 bytes, approved ASCII capitalization edits, deletion ranges and explicit paired-dash gap insertions.

The checked-in logs capture the actual runs, avoiding a need to repeat them for review. The first evaluator pass exposed the accepted-alternative reporting error; only the evaluator was corrected and rerun. `manifest.json` covers this artifact's files; parent raw-model archives have separate manifests.

## 5. Limits

These results are development evidence, not a semantic safety guarantee or release approval. An unquoted foreign-language `um`, a grammatical repetition, or a literal sound lacking the bounded syntax can still be deleted if the model proposes it. The explicit single-word exclusion list does not cover every deliberate repetition, and multiword repetition remains contextual.

Protecting more spans can reject useful edits in the same proposal; v6 deliberately keeps whole-proposal fallback. No partial salvage, paraphrasing, user-dictionary inference, model-specific exceptions, case-ID rules or German phrase exceptions were added. Further prompt selection and any separate language admission guard require their own frozen evidence before a new independent qualification.
