# Direct cleanup prompt development

- **Version:** 1.0
- **Date:** 2026-09-08
- **Status:** In Review

This directory is **development**, begun only after the complete frozen eight-profile head-to-head was archived. The same 60 cases are now revealed development data, combined with the original eight diagnostic cases and supplied sentence (69 total). These results cannot be represented as fresh qualification. Release250 and restart12 remain separate and unopened until an explicit model/prompt/validator freeze and approval.

## Contents

1. Controlled changes
2. Raw results and interpretation
3. Native LFM sampler control
4. Reproduction

## 1. Controlled changes

Each variant changes one thing from its named predecessor. Model remains the pinned stock MiniCPM5-2B MLX artifact; the sampler remains greedy/no-thinking, with native chat templates and full-output budgets. No product settings or model cache changes occurred.

- D1 adds one initial-hesitation demonstration to the frozen common envelope prompt: “Um, the spare key is inside the top drawer.” → “The spare key is inside the top drawer.”
- D2 adds exactly one instruction to D1: “Copy every retained word exactly from the transcript, in its original language; do not change spelling or number formatting.”
- D3 retains the D2 instruction and all three examples verbatim, but places clearly delimited examples inside the system message. The actual transcript is the only user turn. This tests whether previous conversation examples caused observed demonstration echoes.
- D4 adds one semantic definition to D3: “A word or sound being named, spelled, quoted, written, or described is intended content, even if it looks like a filler or repeated word.”

## 2. Raw results and interpretation

| Development variant | Exact /60 | Lexical /60 | Ordinary cleanup lexical /24 | Ordinary preservation lexical /24 | Old dev lexical /8 | Added-word cases /60 | Word-loss cases /60 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| D1: leading example | 51 | 51 | 22 | 22 | 6 | 4 | 9 |
| D2: retained-word instruction | 52 | 53 | 22 | 23 | 7 | 3 | 7 |
| D3: examples inside system | 54 | 56 | 24 | 24 | 4 | 0 | 4 |
| D4: literal definition | 54 | 55 | 24 | 23 | 6 | 0 | 5 |

All 276 requests finished normally. Every variant preserves the natural supplied-sentence cleanup. These raw word-loss counts are diagnostics: the `$184.75` substitution preserves the amount but violates wording, while dropping quoted words or translating a passage changes intended content. Exact protected-payload checks and manual review remain separate.

D1 improves leading hesitations and a dangling dash, but increases translation and required-word loss. D2 restores some preservation but echoes the last example instead of a long transcript. D3 removes those demonstration echoes and translations: all 24 ordinary cleanup cases are lexically complete and all 24 ordinary preservation cases are exact. Its remaining problems are literal words, quoted sound names and notation; old mixed literal/cleanup cases also fail. D4 fixes two old literal/name cases but newly deletes emphatic “very” in an ordinary preservation case and provides no additional cleanup coverage.

A separately frozen source validator must assess the **whole** proposal. A proposal that drops protected content falls back to the original transcript; the benchmark must not salvage its convenient edits. Partial useful-edit counts do not satisfy a complete-cleanup gate. Combined validator results and independent token/run-recall metrics are separate artifacts; none of these raw model results alone qualifies automatic delivery.

## 3. Native LFM sampler control

The native always-thinking LFM2.5-2.6B control uses the unchanged common prompt. Greedy sample latency was37.94s. Its documented native-MLX recommended-parameter control (temperature0.1, top-k50, repetition penalty1.1,20-token native repetition window, seed42 reset per request) completes the same sample in16.39s but still drops intended “those.” The four pilot cases complete in11.41s and27.54s (both exact), with two30s timeouts. None of the five requests meets the shared10s deployment target. Reasoning text and final output are separately retained; estimated reasoning-token counts are explicitly retokenized estimates.

These results show sampler sensitivity, not a complete optimization study or a claim that every possible LFM configuration is unsuitable. MLX-LM0.31.3's native repetition window and prefilling behavior differ from Transformers' full-input repetition penalty. Raw native-control files are in `results/liquid2600-native-recommended-*.json`.

## 4. Reproduction

Use the same isolated runtime and pinned local model as the parent comparison. `run_development.py` adds native sampling and streamed performance metadata while retaining conversation-form examples. `run_inline.py` additionally supports D3/D4 system examples. Input fixture and prompts are preserved verbatim.

```bash
HF_HUB_OFFLINE=1 HF_HUB_DISABLE_IMPLICIT_TOKEN=1 python run_inline.py \
  --model /absolute/path/to/pinned/MiniCPM5-2B --label minicpm2000 \
  --prompt prompt-inline.json --cases development69.json --modes few_shot \
  --output /tmp/reproduced-development.json
```

The new performance fields include library-reported prompt/generation throughput and time to first generated token. They support a later measured prefix-cache experiment; no prefix cache was used here. The original frozen driver and results remain unchanged in the parent directory.

After the failed independent qualification, [D5 and D6](d5-restart/README.md) isolate an added inline restart example and an added abstract restart instruction on a preselected36-case development pilot. D5 partly improves restart coverage; D6 does not further improve it. Neither result qualifies a release.
