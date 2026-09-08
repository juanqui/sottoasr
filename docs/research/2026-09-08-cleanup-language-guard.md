# Conservative language preservation for cleanup

- **Version:** 1.0
- **Date:** 2026-09-08
- **Status:** In Review

## Contents

1. Hypothesis and sources
2. Isolated experiment
3. Decision boundary

## 1. Hypothesis and sources

The first independent cleanup qualification lost the meaningful German preposition
`Um` at the beginning of a sentence. A lightweight language detector could bypass
English disfluency cleanup for confidently identified non-English text. This is
a separate defense, not proof that every short or mixed-language input is recognized.

[Whatlang 0.18.0](https://docs.rs/whatlang/0.18.0/whatlang/) supports 70 languages
with a trigram model. Its public `is_reliable()` result is derived from confidence
greater than 0.9; the author describes confidence using distinct trigrams and the
margin between candidate languages. This value is not a calibrated probability
of correctness. [Official algorithm notes](https://github.com/greyblake/whatlang-rs#how-does-it-work),
[Info implementation](https://docs.rs/whatlang/0.18.0/src/whatlang/core/info.rs.html).

[Whichlang](https://github.com/quickwit-oss/whichlang) is another small Rust option,
but its current public detection API returns a language without an abstention or
confidence result. Its own evaluation also shows input-length dependence. It was
not measured here; a fast forced choice is insufficient for this proposed bypass.

## 2. Isolated experiment

The [source and raw results](../../benchmarks/llm/model-study-2026-09-08/direct-cleanup-diagnostic/language-guard/)
pin Whatlang to `=0.18.0`. The standalone Rust probe calls `detect(source)` and
proposes preservation only when `is_reliable()` is true and the language is not
English. IDs, expected text and groups pass through for offline analysis and do
not enter detection. No production files, model configuration or caches changed.

The 331 inputs combine the now-revealed development69, failed qualification250
and restart12. This is post-hoc development evidence. Fourteen inputs meet the
bypass condition, all correctly identified non-English preservation cases,
including the failed German example. No English input or required-cleanup case
is bypassed. No threshold was tuned to these outcomes.

A separate 24-input authored probe includes short English dictation, accented
names, model identifiers, mixed-language quotations and five short German or
Portuguese texts. None of the 19 English inputs is bypassed. None of the five
short foreign inputs meets the reliability threshold either: short-language
ambiguity remains a real limitation. Lowering the threshold was not tested.

One serial pass on this M4 measured median detection time of 60.042 microseconds
and maximum 233.417 microseconds across the 331 inputs. Timing covers detection
only, excluding JSON parsing, serialization and process startup; this is an
initial overhead measurement, not a stable performance benchmark. MLX inference
was paused during the isolated build and execution.

## 3. Decision boundary

The proposed use is a conservative whole-source bypass for confidently detected
non-English input, using the published reliability predicate unchanged. Unknown
or low-confidence text still needs the model and source validator. This cannot
replace short/mixed-language model qualification or protect every meaningful
foreign occurrence of `um`.

If adopted, freeze the exact detector with the prompt and validator. The next
independent qualification must use the actual Rust detector, without reference
labels influencing the decision, and count every useful bypass against cleanup
coverage. Production should report a language-preservation bypass truthfully and
retain the original. Adoption remains conditional on the combined experiment.
