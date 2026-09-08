# Source-preserving validator v3: development review

- **Version:** 3.0
- **Date:** 2026-09-08
- **Status:** In Review

## Contents

1. [Scope and provenance](#1-scope-and-provenance)
2. [Policy changes](#2-policy-changes)
3. [Offline results](#3-offline-results)
4. [Limits and remaining failures](#4-limits-and-remaining-failures)
5. [Artifacts and reproduction](#5-artifacts-and-reproduction)

## 1. Scope and provenance

This is an experimental source-deletion validator, with no product integration or installation. Version 2 and its [independent raw-archive review](../validation-prototype/README.md) remain unchanged. Version 3 was deliberately developed from the observed semantic60 failures, then frozen before its own scoring pass. Neither release250 nor restart12 was opened.

The source hash is `33520cd53a2716fb502a602b89cce77531ec0ab6c2858ec8e52504cdc3254e9e`; [freeze.json](freeze.json) records the timestamp and predecessor. Nineteen authored unit-test groups passed before this v3 scoring pass. This is development evidence, not a fresh holdout result or a semantic safety guarantee.

Validation receives only the original transcript, the proposed model output, completion status, and an empty user dictionary for this experiment. Reference outputs and manually annotated protected spans are used exclusively by the scorer. The validator derives its protections from source syntax; it never receives case IDs, expected text, or benchmark annotations.

## 2. Policy changes

All existing constraints remain: no added, substituted, or reordered words; bounded alignment; source UTF-8 deletion offsets; syntactic identifier/code/quote/dictionary protections; adjacent repetitions and bounded article restarts; full-source fallback when any proposed edit is unsupported. Equivalent alignments must reconstruct the same output. A rejected proposal is never partly salvaged.

| Change | Explicit boundary | Authored counterexample or limitation |
|---|---|---|
| Literal payload | After `string` or `literal`, protect the immediately named word and up to four identical adjacent instances. Allow one intervening `literal`/`value` keyword. | `The string is um tangled.` still permits deleting `um`; the rest of the clause is not locked. `Enter the string er into that field.` preserves literal `er`. |
| Notation payload | After `notation`, protect one to four words only when immediately followed by `means`, `denotes`, or `represents`, within the same clause. | `In our notation, a a denotes two distinct inputs.` preserves both `a`s. `This notation is um hard to read.` permits the filler deletion. |
| Reported sound | `said`/`answered`/`replied`/`uttered`/`responded` followed immediately by one to four hesitation tokens, with an explicit sound/utterance/syllable cue and exact/literal/recorded/transcribed cue within twelve subsequent tokens and before a sentence boundary. | `She replied erm, and the clerk transcribed that exact syllable.` is protected. `She answered um the next question carefully.` permits the filler deletion. A cue in the next sentence does not count. |
| Optional final period | Comparison may disregard one final period; source punctuation determines the delivered result. | Source `We arrive Monday erm`, proposal `We arrive Monday.` delivers `We arrive Monday`. Questions, exclamations, ellipses, and paragraph changes remain rejected. |
| Paired filler em-dashes | Remove only a paired em-dash region containing one to four deleted hesitation tokens, between retained words. Replace that local region and adjacent horizontal spacing with one space. At that exact join, a proposal may contain zero, one, or two residual em-dashes. | `Ask Léa—uh—to bring coffee.` delivers `Ask Léa to bring coffee.` A single unmatched dash, a substantive aside, a semicolon replacement, or a protected filler is rejected. |
| Quote delimiter style | Paired straight/smart quote styles may differ within the same single/double quote family. Deliver original delimiters and require exact inner payload. Code delimiters stay exact. | Changing a comma or letter case inside the quote is rejected. Quote parsing now keeps apostrophes within contractions inside the protected payload; `‘It isn’t um optional.’` cannot lose `um`. |

The em-dash rule adds one explicitly recorded whitespace operation, `gap_insertions`, at an original UTF-8 deletion start. It does not copy generated punctuation or arbitrary whitespace into the transcript. The evaluator reconstructs the result independently from deletion intervals, the recorded one-space insertions, and the inherited narrow sentence-initial ASCII capitalization operations.

The inherited `MENTION_MARKERS` rule remains intentionally conservative and broad: a word/name/token/label/etc. mention locks the remainder of its clause through the next `.`, `!`, `?`, or newline. Version 3 does not expand that whole-clause rule. It can suppress useful edits later in a mention clause and is not general language understanding.

## 3. Offline results

The six original MiniCPM/Pollard/Spark/Qwen profiles were evaluated unchanged, along with MiniCPM development prompts D1, D3, and D4. Complete cleanup requires an allowed reference word sequence **and** exact protected-span fidelity. Preservation means byte-for-byte original source. The detected-loss count is the union of added-word, required-word-loss, and protected-fidelity diagnostics; it is not comprehensive semantic adjudication.

Values separated by arrows are **raw proposal → v2 delivered → v3 delivered**. Each profile uses the same 60 development cases, not independent new cases.

| Profile | Ordinary complete cleanup /24 | Adversarial complete cleanup /4 | v3 exact preservation /32 | Detected loss cases /60 |
|---|---:|---:|---:|---:|
| MiniCPM5-2B common | 20 → 18 → 20 | 3 → 3 → 3 | 32 | 5 → 3 → 0 |
| Pollard mixed common | 19 → 17 → 19 | 3 → 3 → 3 | 32 | 4 → 3 → 0 |
| Spark common | 13 → 12 → 13 | 1 → 1 → 1 | 32 | 2 → 2 → 0 |
| Spark best development | 15 → 14 → 15 | 3 → 3 → 3 | 32 | 3 → 2 → 0 |
| Qwen3.5-4B common | 22 → 20 → 22 | 3 → 2 → 3 | 32 | 5 → 2 → 0 |
| Qwen3.5-4B best development | 13 → 11 → 13 | 3 → 2 → 3 | 32 | 3 → 1 → 0 |
| MiniCPM D1 | 22 → 20 → 22 | 4 → 4 → 4 | 32 | 9 → 3 → 0 |
| MiniCPM D3 | 24 → 22 → 24 | 4 → 3 → 4 | 32 | 4 → 3 → 0 |
| MiniCPM D4 | 24 → 22 → 24 | 4 → 4 → 4 | 32 | 5 → 3 → 0 |

Version 3 restores useful cleanup previously rejected because of a final-period difference, paired filler em-dashes, or straight/smart quotes. Its new literal guards reject the observed unquoted string, notation, and reported-sound deletions. No additional detected loss appears in these data.

The older nine development examples remain a separate check, including five cleanup and four preservation cases:

| MiniCPM profile + v3 | Older complete cleanup /5 | Older exact preservation /4 | Combined complete cleanup /33 | Combined preservation /36 | Combined detected losses |
|---|---:|---:|---:|---:|---:|
| D1 | 5 | 4 | 31 | 36 | 0 |
| D3 | 3 | 4 | 31 | 36 | 0 |
| D4 | 3 | 4 | 31 | 36 | 0 |

D3 and D4 complete all semantic60 cleanup cases but miss the same two older cleanup cases. D4 adds no delivered coverage over the simpler D3 and introduces a raw deletion of emphatic `very` that the validator rejects. D1 misses different semantic60 cleanup cases while completing all older cases. These development tradeoffs do not establish a model or prompt winner on unseen dictation.

There are **1,134 independent UTF-8 reconstruction checks**: 567 proposals evaluated through both frozen validators. The 567 proposals are six profiles ×60, plus three development profiles ×69. This is offline scoring of existing outputs, with no new inference or latency claims.

## 4. Limits and remaining failures

- The prototype cannot prove whether an unquoted hesitation-looking token or repeated phrase is semantically dispensable. Executable counterexamples remain: Portuguese `Há um sensor.` can lose meaningful `um`; `We we are responsible.` can lose deliberate emphasis. Successful development results do not erase these limits.
- D3/D4 delete both an ordinary filler and literal `a` from `Please uh, type a a into the box without removing either letter.` They also delete both repeated `I` and literal `erm` from `I I wrote the word erm, in the glossary yesterday.` The validator falls back to the complete source in both cases, leaving the useful cleanup undone.
- D3 achieves 60/60 lexical/protected acceptance on semantic60 but 59/60 strict exact text. One source-owned comma remains in `Reserve a seat for, Elena at the morning workshop.` Source reconstruction intentionally does not adopt arbitrary model comma choices.
- Capitalized names, mention syntax, quote parsing, and language assumptions are heuristic. For example, sentence-boundary detection around abbreviations is not a general proper-name recognizer. Production use still requires the separately approved runtime/failure contract, meaningful unseen evaluation, and further review.
- The original raw driver’s embedded `accepted_sample` flag is **noncanonical**: it omits the primary reference when alternatives are present. The frozen raw records were not modified. The [independent audit](../validation-prototype/archive-audit.json) found six such false negatives and no mismatch in the external canonical 480-request summary, which unions primary and alternative references.

## 5. Artifacts and reproduction

- [source_validation.py](source_validation.py): frozen v3 implementation and authored tests.
- [freeze.json](freeze.json): source provenance, initial input hashes, and experimental boundary.
- [unit-tests.txt](unit-tests.txt): 19 passing test groups, including protected contractions and independent UTF-8 gap reconstruction.
- [evaluate.py](evaluate.py): scorer; imports the unchanged frozen v2 evaluator for common metrics, validates source annotation byte offsets, and never supplies gold annotations to either validator.
- [summary.json](summary.json): complete cleanup, exact preservation, fallback, and diagnostic counts, separated by dataset group and old9.
- Profile JSON files: raw proposals, v2/v3 delivered text, reasons, deletion/capitalization/gap offsets, all scores, input paths, and hashes.
- `inputs/`: exact D1/D3/D4 raw output copies. Original six result files remain in the parent `results/` directory.
- [manifest.json](manifest.json): portable artifact hashes; generated Python bytecode is excluded.

From the repository root, with a standard Python 3 interpreter and no extra dependencies:

```bash
PYTHONDONTWRITEBYTECODE=1 python3 benchmarks/llm/model-study-2026-09-08/direct-cleanup-diagnostic/validation-prototype-v3/source_validation.py 2>&1 | tee /tmp/source-validation-v3-tests.txt
PYTHONDONTWRITEBYTECODE=1 python3 benchmarks/llm/model-study-2026-09-08/direct-cleanup-diagnostic/validation-prototype-v3/evaluate.py 2>&1 | tee /tmp/source-validation-v3-scoring.txt
```

The experiment used only lightweight standard-library source checks and JSON scoring. Those short CPU operations could overlap a peer's timed development inference; no claim is made that the whole machine was otherwise idle. No browser, build, MLX import, model download, or model run was performed for this validator iteration.
