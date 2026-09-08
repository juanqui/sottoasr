# Frozen source-validator v2: exploratory pipeline evaluation

- **Version:** 2.0
- **Date:** 2026-09-08
- **Status:** Implemented

## Contents

1. Scope and provenance
2. Coverage and preservation
3. Concrete findings
4. Archive audit and reproduction

## 1. Scope and provenance

The frozen prototype is exploratory development evidence, not release qualification. Its author first inspected semantic60 after writing `freeze.json` at13:04:03UTC. The validator SHA256 is `4a7becf860f7062afd2831be8db3886ebb8830865158d4290aaedc5d2faa42a1`, unchanged through this evaluation. Earlier work used the old9 examples and authored counterexamples. The root's preceding coverage review supplied a repeated-phrase development example that overlaps semantic16, so this is not presented as a fully independent blind validator test.

The unchanged validator processed360 saved proposals: four model families, with additional preselected Spark and Qwen prompt profiles. No inference ran. The dictionary input was empty; gold outputs and the protected-span companion were used only by the scorer. They were never passed into validation as special protections. Release250 and restart12 were not viewed.

## 2. Coverage and preservation

Complete cleanup requires the expected word sequence plus exact protected-span fidelity. It allows harmless punctuation differences, but is not a claim of polished formatting. Preservation columns require exact original text. Rejected useful edits count against cleanup coverage.

| Profile | Ordinary complete cleanup, raw→validated /24 | Adversarial complete cleanup, raw→validated /4 | Ordinary original preserved, raw→validated /24 | Adversarial original preserved, raw→validated /8 | Validated protected-loss cases /60 |
| --- | ---: | ---: | ---: | ---: | ---: |
| MiniCPM5-2B common | 20→18 | 3→3 | 24→24 | 4→5 | 3 |
| Pollard mixed common | 19→17 | 3→3 | 24→24 | 5→5 | 3 |
| Spark common | 13→12 | 1→1 | 24→24 | 6→6 | 2 |
| Spark best development prompt | 15→14 | 3→3 | 23→24 | 6→6 | 2 |
| Qwen4B common | 22→20 | 3→2 | 21→24 | 4→6 | 2 |
| Qwen4B best development prompt | 13→11 | 3→2 | 23→24 | 5→7 | 1 |

Every validated output was independently reconstructed again from its recorded UTF-8 deletion/capitalization offsets:360/360 matched. All rejected proposals returned the complete source. Structural containment therefore works, but the remaining content losses prevent qualification.

## 3. Concrete findings

- Literal string content in semantic54: MiniCPM, Pollard and common-prompt Qwen still delete `uh` from a request to save that exact string.
- Literal electrical notation in semantic56: MiniCPM, Pollard and both Spark profiles reduce `I I` to `I`.
- Reported speech in semantic60: every profile deletes the witness's recorded `uh`, which is intended content.

These are substantive preservation errors. They are not punctuation disputes or obsolete guard-driven gold labels. The validator's permitted token/repetition syntax does not resolve their meaning.

Useful cleanup is also rejected unnecessarily in three general situations: a proposal adds a terminal period absent from source (semantic5); a filler sits between paired em dashes (semantic22); or a proposal changes curly quote delimiters to straight ones without changing the literal payload (Qwen semantic51). These identify explicit normalization-policy development work. The v2 validator and its results remain unchanged.

## 4. Archive audit and reproduction

`archive-audit.json` independently reconciles all480 scored requests from eight profiles of six distinct models, with eight known-example warmups excluded. All source IDs, primary/alternative references, protected UTF-8 annotations, frozen prompt/harness hashes, model pins, runtime profiles, medians, nearest-rank p95 values, CPU sums and RSS/Metal extrema agree with canonical `semantic60-summary.json`.

Six raw-result `accepted_sample` flags disagree with canonical scoring because the frozen driver substitutes alternatives for the primary reference instead of taking their union. The external scorer correctly unions them. Treat embedded `accepted_sample` as noncanonical; preserve the raw artifacts. The affected rows are semantic5 for MiniCPM, Pollard, both Qwen profiles and Liquid1.2, plus common-prompt Qwen semantic1.

The load timer excludes imports and does not purge OS caches. Request timers include formatting/generation/cache cleanup but exclude IPC. Process RSS peak and per-request allocated Metal peak measure different scopes and must not be summed. The frozen driver lacks a pre-inference context guard; all recorded prompt-plus-budget totals are safely below each model's context in this dataset. It records actual Spark architecture execution, not a guessed substitute.

Run the reproducible offline evaluation from the repository root:

```bash
python3 benchmarks/llm/model-study-2026-09-08/direct-cleanup-diagnostic/validation-prototype/evaluate.py
```

The evaluator reads only this frozen prototype, the named semantic60 dataset/annotations, and the eight original raw60 result files. It writes its own profile results, summary and audit. It does not invoke models, touch production files, or read later qualification datasets.
