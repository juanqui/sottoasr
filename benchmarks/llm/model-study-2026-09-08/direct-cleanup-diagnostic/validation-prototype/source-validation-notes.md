# Source-preserving direct-output validator prototype

- **Version:** 2.0
- **Date:** 2026-09-08
- **Status:** In Review

## Contents

1. Scope and invariants
2. Small edit policy
3. Development checks
4. Limits and next review

## 1. Scope and invariants

This isolated Python prototype implements the source-alignment experiment in the approved automatic-cleanup recovery specification, section4.2. It is not production code, does not load a model, and was not evaluated on semantic60. Raw model comparisons and their scoring remain unchanged. Its only inputs were the existing nine development examples and separately authored structural/contextual counterexamples.

The proposal supplies a subsequence of source words, never replacement text. Source tokens retain character positions and UTF-8 byte boundaries. Protected words cannot be deleted; insertions, substitutions, changed word order, unsupported deletion categories, incomplete generation, malformed Unicode, and exceeded bounds preserve the complete source. The caller must truthfully supply `completed=False` for truncated or failed inference.

Quotes/code, numeric or syntactic identifiers, URLs/email, dictionary matches, negations, numeric units, capitalized words, and explicit word/name mentions receive deterministic or conservative syntactic protection. Exact quote/code, identifier, and dictionary text is also checked for changed punctuation. This is not a complete name recognizer or semantic analyzer.

Alignment uses an explicit stack with limits of32,000 UTF-8 bytes,1,024 source words,20,000 explored states, and128 enumerated alignments. Equivalent duplicate alignments are accepted only when their reconstructions are identical. Materially different reconstructions preserve the input. No partial salvage occurs when a proposal needs an unsupported deletion.

## 2. Small edit policy

Allowed deletion categories are:

- Hesitation tokens `um`, `uh`, `erm`, `er`, `uhm`, `umm`, and `hmm`, subject to protection. Capitalized fillers are eligible at input start and after explicit sentence punctuation or a newline. Terminal fillers are covered. Dictionary and quote protection always take precedence.
- Adjacent repeated spans of one to four words, with at least one intact copy retained. One-word repeats use a small function-word/pronoun set. Multiword repeats are not restricted to that vocabulary; they must contain at least two distinct words. `Please attach please attach the photo.` and `Send it send it tomorrow.` are covered. A capitalized initial phrase word is exempted from the capitalized-name heuristic only when its matching repeated phrase starts with the lowercase form. Repeated capitalized names and isolated emphatic content repeats such as `very very` remain protected. This bound still cannot classify every intended repetition.
- A bounded article restart: `a/an/the` followed by one to four hesitations, followed by a retained replacement determiner. An optional `yeah,` is allowed only after at least two hesitations and before that determiner. The article, intervening hesitation run, and marker must all be deleted together. This is one grammatical category, not a match for the reported sentence.

Output is reconstructed from source spans. A deletion consumes its attached comma and following horizontal gap; terminal deletion also removes a dangling preceding comma/gap. Newlines and other punctuation remain sourced. Proposal comma and horizontal-space changes are ignored in favor of reconstructed source punctuation. Other punctuation changes reject the proposal. Explicitly requested capitalization of the first surviving word at each reconstructed sentence/paragraph boundary is allowed, using a single ASCII lowercase-to-uppercase change. Its letter case is the sole non-deletion transformation. Original UTF-8 offsets are returned for both deletions and capitalization.

Example: a model proposal `I came this morning.` for `I came, um, this morning.` becomes source-derived `I came, this morning.`. Retaining that awkward comma is an intentional prototype policy, not a claim of perfect formatting.

## 3. Development checks

`source-validation-tests-v2.txt` captures thirteen passing stdlib unittest groups after the root's independent coverage review. They cover equivalent duplicate alignment, rejected materially different alignments, general repeated phrases, independent article-restart variants, sentence/paragraph/terminal fillers, UTF-8 reconstruction, quotes/code/dictionary/identifier protection, numbers/negations/names/final clauses, literal mentions, agreement, punctuation/paragraphs, incomplete output, and bounded work. A final expanded UTF-8 check verifies source offsets for capitalization after a multibyte name as well as deletion offsets; its focused passing result is in `source-validation-utf8-v2.txt`.

The initial twelve groups took0.134seconds, and the revised thirteen groups took0.033seconds (process0.37seconds real,0.10seconds user,0.02seconds system). The final focused UTF-8 check took0.001seconds. The model-comparison owner permitted these small CPU-only checks during its serial inference window; therefore that timing window must not be described as an otherwise idle machine. No MLX import, browser, build, package change or model inference ran in the validator task.

`source-validation-development-v2.json` preserves every old9 raw proposal, acceptance decision, reconstructed output, category, source offsets, and exact-output result, together with source and fixture hashes. The initial v1 evidence remains separately preserved. The inspected envelope-only few-shot results were unchanged by the coverage improvements:

| Raw model profile | Validator accepted proposals | Final exact accepted-reference output | Complete edit-required cases |
| --- | ---: | ---: | ---: |
| MiniCPM5-2B, official quant | 9/9 | 9/9 | 5/5 |
| Spark4B | 8/9 | 6/9 | 2/5 |
| Qwen3.5-4B | 6/9 | 8/9 | 4/5 |

MiniCPM's only raw exact-format difference was a removed comma; reconstruction restored source punctuation. Its reported-example proposal removes all clear hesitations and the bounded abandoned article/restart marker. The validator rejects Spark's dropped name `Um`, Qwen's dropped literal `um` and name `Um`, and Qwen's merged literal sequence `a a` into `aa`. A rejected proposal counts against edit coverage when the original still needs cleanup; preservation fallbacks are not represented as model successes.

## 4. Limits and next review

Two executable counterexamples deliberately demonstrate that the syntactic policy is not a semantic guarantee:

- `Há um sensor.` → `Há sensor.` is accepted, although Portuguese `um` is meaningful.
- `We we are responsible.` → `We are responsible.` is accepted even when the repetition is deliberate emphasis.

Unlisted names, literal uses without explicit mention syntax, and contextual agreement/restarts have the same fundamental ambiguity. The bounded restart form could still remove meaningful words in an unusual but valid utterance. Adding a long list of ad hoc exceptions would not solve that reliably.

Therefore the prototype demonstrates useful source alignment and concrete structural containment, not qualification for automatic delivery. It requires independent code review, source-derived unit coverage in the eventual Rust implementation, and the separately frozen semantic release gate. All semantic60 data remains unopened by this task. No installed application change is authorized by this prototype.
