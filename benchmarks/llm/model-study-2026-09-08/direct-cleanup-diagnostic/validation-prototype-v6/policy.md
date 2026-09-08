# Validator v6 development policy

- **Version:** 1.0
- **Date:** 2026-09-08
- **Status:** Approved

## Contents

1. [Scope and evidence](#1-scope-and-evidence)
2. [Exact policy and counterexamples](#2-exact-policy-and-counterexamples)
3. [Model errors and verification](#3-model-errors-and-verification)

## 1. Scope and evidence

The v5 qualification failed. Its release250 and restarts12 outputs were explicitly released for development review before this policy was written. V5 produced 90/100 complete ordinary cleanup cases versus the model's 99/100, with four detected delivered word-loss cases across release250. The twelve restart cases completed only one of six intended cleanups, mostly because the model dropped a meaningful replacement determiner or invented another article.

This is a new experimental iteration, not a product port or a fresh qualification claim. Frozen v2–v5 code and raw model results remain unchanged. The root reviewed the proposed boundaries before implementation; this document records the concrete policy and counterexamples before the v6 source is changed.

## 2. Exact policy and counterexamples

| Change | Exact rule | Useful example | Counterexample / unchanged boundary |
|---|---|---|---|
| Retained negation capitalization | Permit the existing ASCII lowercase-to-uppercase change on a retained sentence-initial word even when deletion protection applies. Exact quote/code/dictionary checks still prohibit changing those payloads. No protected word becomes deletable. | `Uh, not yet.` → `Not yet.`; `Um, don't send it.` → `Don't send it.` | Reject deleting either `not` from `Do not not send it.`; reject changing case inside a protected quote or dictionary term. |
| General single-word repetitions | Remove the function-word-only eligibility restriction for one-word adjacent repeats. Require at least one retained copy, the same source gap rules, and all existing source protections. Apply the explicit exclusion set below only to one-word repeat groups. | `We can can finish.` → `We can finish.`; `Please bring bring the wrench.` → `Please bring the wrench.` | Keep `very very`, `really really`, `so so`, `too too`, `yes yes`, `yeah yeah`, `hey hey`, `wait wait`, proper names, numbers, negations, literal strings, and dictionary terms. Existing `Please attach please attach…` remains eligible. |
| Source-comma ambiguity | If several already-valid source reconstructions differ, prefer a candidate only when exactly one agrees with the proposal's comma punctuation under the existing space/quote/final-period comparison policy. If none or several match, reject. Never insert a model comma or other punctuation. | `At the moment at the moment, the server is unavailable.` can retain the source occurrence followed by its comma. | A semicolon or question-mark rewrite still rejects. `I, I need it.` → `I need it.` may now select its uniquely matching source reconstruction; genuinely unresolved alternatives still reject. |
| Bounded label payload | Remove only `label`/`labels` from the broad mention-to-clause-end lock. Protect the immediately named payload, including one existing optional `literal`/`value` keyword, or a payload introduced by `reads`/`says`/`is`/`was`. Protect up to four identical adjacent instances. | `The return label should go on on the carton.` → `The return label should go on the carton.` | `The label reads um exactly.` preserves `um`; `The label is um` stays protected. The broad word/name/token/etc. clause locks remain unchanged. |
| Named parameters and tag endpoints | Extend bounded literal nouns to `tag`/`tags` and `parameter`/`parameters`. Also recognize an immediate `starts`/`ends` + `in`/`with` before the payload. Protect that payload only, not the clause. | `The tag ends in uh, um.` may remove final filler `um` while retaining literal `uh`. | Reject deleting `um` from `Set the parameter um to zero.`; ordinary `The parameter should um remain unchanged.` still allows its filler removal. |
| Honorific/initial names | Protect a capitalized following word after an explicit honorific or single uppercase initial and a period. This protection overrides sentence-initial hesitation eligibility. | `Dr. Um um confirmed it.` may remove the second lowercase `um` if the proposal preserves `Dr. Um`. | Reject `Dr. confirmed it.`; preserve `J. Um` as a name. `All set. Um, bring it.` remains eligible: ordinary periods do not globally protect fillers. |

The single-word repetition exclusion set is: `very`, `really`, `so`, `too`, `much`, `more`, `less`, `quite`, `rather`, `extremely`, `absolutely`, `yes`, `yeah`, `yep`, `yup`, `nope`, `okay`, `ok`, `right`, `hey`, `wait`, `stop`, `please`. It does not globally lock these words or block their removal as part of an otherwise eligible multiword repeated span. It is an explicit conservative set for intensity, affirmation, attention, and pleading; it cannot classify every deliberate repetition.

Honorifics are `Dr.`, `Mr.`, `Mrs.`, `Ms.`, `Mx.`, `Prof.`, `Rev.`, and `Hon.`, matched without case sensitivity, plus a single uppercase letter followed by a period. The next word must start uppercase. Identifiers, quotes/code, dictionary terms, numeric content, and negations retain their existing independent protections.

For comma tie-breaking, compare only already-reconstructed strings. Normalize horizontal spacing around commas for comparison, retain newlines, and keep the existing quote-delimiter/final-period comparison semantics. The delivered string is the selected source reconstruction, never the normalized comparison string.

## 3. Model errors and verification

The meaningful German `Um` deletion is a model semantic failure. No phrase or German word exception is added to the core validator. The root's separate source-language experiment reports that `whatlang 0.18.0` with its built-in reliable-non-English result preserves 14/331 revealed development inputs, all intended keep cases, while missing five short foreign controls. That optional whole-source bypass remains root-owned, independently measured, and explicitly cannot guarantee language or semantic safety. The current ASR result exposes no language field.

The model must still preserve replacement determiners in abandoned-article restarts, literal quoted words, factual corrections, and contractions. The validator must not repair the model's proposal, choose a replacement article, salvage only the safe part of a rejected proposal, or infer intended meaning from benchmark labels. The remaining restart grammar `a those` is incomplete model cleanup, not permission to delete another source word without a valid proposal.

Before freezing v6, add authored tests for each useful example and counterexample, replay all previously revealed raw proposals, and report complete cleanup separately from preservation and detected loss. Root/peer-owned prompt and language experiments remain separate columns. Record the full source hash and compare against the frozen predecessor; no runtime, dependency, product, or installed-app edits belong to this step.
