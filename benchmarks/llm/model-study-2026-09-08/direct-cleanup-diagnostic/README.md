# Direct transcript editing diagnostic

- **Version:** 2.0
- **Date:** 2026-09-08
- **Status:** In Review

Direct transcript output fixes the earlier request-format bottleneck. **Stock MiniCPM5-2B with a transcript-as-data envelope cleans the reported sentence naturally** and is the strongest speed/ordinary-preservation candidate in this measured comparison. It scored 50/60 accepted exact outputs at 796ms median, with all 24 ordinary preservation cases intact. Remaining literal-content errors mean this is a development candidate, not a qualified automatic-cleanup release. No production model, prompt or settings changed during these experiments.

The concise final frozen comparison is in [HEAD-TO-HEAD.md](HEAD-TO-HEAD.md).

## Contents

1. Request and reported sentence
2. Full development comparison
3. Interpretation and next experiment
4. Reproduction and limits
5. Frozen independent 60-case comparison
6. Native reasoning pilot and next development

## 1. Request and reported sentence

All models received the same direct editing instruction: remove empty hesitations, accidental stutters and clearly abandoned short fragments; preserve intended wording, facts, names, numbers, negations, language and order. They returned the entire transcript, with no candidate list or ID/role classification. The few-shot condition adds one ordinary cleanup and one literal-word preservation example. `prompt.json` preserves both examples and the exact system instruction.

The supplied sentence is:

> This is a test to see um if this can um remove all the um uh yeah, those things from the sentences.

Qwen3.5-4B zero-shot returned in 1.486 seconds:

> This is a test to see if this can remove all those things from the sentences.

It removed every `um` and `uh`, the discourse `yeah`, and the abandoned `the`, preserving the intended remaining words and their order. The primary agent accepted this natural target before inference. Qwen4B few-shot returned the stricter filler-only target in 1.524 seconds:

> This is a test to see if this can remove all the yeah, those things from the sentences.

Qwen0.8B removed all three `um` tokens but retained `uh yeah` in both modes. MiniCPM5-2B removed the fillers but returned “all the things,” dropping the intended `those`; that difference is recorded, not called exact cleanup. MiniCPM5-1B paraphrased or echoed demonstrations. Granite retained fillers. Stock350 introduced words. Every raw output and lexical change is preserved in `results/`.

## 2. Full development comparison

These eight existing development cases are known diagnostic data: four edits and four preservation cases. Exact match strips only outer output whitespace. Lexical exact match casefolds Unicode word tokens and ignores punctuation. “Added” means output words are not a subsequence of the input; “lost” means required gold words are no longer a subsequence of the output. Word multiplicity and order matter. The historical useful-edit diagnostic requires a lexical change on an edit case with neither added nor lost words. These are lexical diagnostics, not adjudicated factual-harm measurements. The original table includes the raw unfinished Qwen0.8 output in its diagnostic flags; an unfinished output is not a successful edit and is not semantic evidence. The later frozen60 scorer gates every quality numerator on normal completion.

| Model | Prompt | Exact /8 | Lexical exact /8 | Cases adding words | Cases losing required words | Useful lexical edits /4 | Median dev ms | Truncated /8 |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| LFM2.5-350M | Zero-shot | 1 | 2 | 3 | 5 | 2 | 103 | 0 |
| LFM2.5-350M | Few-shot | 2 | 4 | 3 | 4 | 2 | 104 | 0 |
| Qwen3.5-0.8B | Zero-shot | 5 | 6 | 1 | 1 | 4 | 313 | 1 |
| Qwen3.5-0.8B | Few-shot | 5 | 6 | 1 | 1 | 3 | 309 | 0 |
| MiniCPM5-1B | Zero-shot | 2 | 5 | 2 | 1 | 3 | 261 | 0 |
| MiniCPM5-1B | Few-shot | 2 | 3 | 4 | 4 | 3 | 260 | 0 |
| MiniCPM5-2B | Zero-shot | 5 | 6 | 0 | 2 | 3 | 518 | 0 |
| MiniCPM5-2B | Few-shot | 6 | 7 | 0 | 0 | 3 | 656 | 0 |
| Qwen3.5-4B | Zero-shot | 3 | 6 | 0 | 2 | 3 | 1008 | 0 |
| Qwen3.5-4B | Few-shot | 6 | 6 | 0 | 0 | 3 | 1170 | 0 |
| Granite4.2-3B | Zero-shot | 2 | 6 | 0 | 2 | 3 | 713 | 0 |
| Granite4.2-3B | Few-shot | 2 | 3 | 0 | 2 | 2 | 832 | 0 |

No leaked special-token strings were found in these raw outputs. Quotes/backticks and punctuation edits explain some lexical-versus-exact differences. These formatting differences are kept separate from substantive words lost or added. Qwen0.8B's single truncated generation is a real repeated “speaker says” completion after misinterpreting an imperative transcript; the raw output and `length` finish reason remain visible.

MiniCPM5-2B contains approximately 2.517B parameters; Granite's 3B label corresponds to a derived 3.660B dense configuration. Model IDs, revisions and artifact sizes are pinned in `models.json`. This is a comparison of the loaded MLX artifacts, not proof that their family labels express resident memory usage.

The additional user-requested artifacts used the same initial zero/few-shot prompts. These rows use full request timing, unlike the historical generation-only table above. Native LFM2.6 has a separate explicitly incomplete pilot in section6.

| Additional artifact | Prompt | Exact /8 | Lexical /8 | Added-word cases | Required-word-loss cases | Useful lexical edits /4 | Median full request ms |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Pollard Mini2 mixed | zero shot | 5 | 6 | 0 | 2 | 3 | 596 |
| Pollard Mini2 mixed | few shot | 6 | 7 | 0 | 0 | 3 | 764 |
| Spark4B | zero shot | 6 | 6 | 0 | 2 | 4 | 953 |
| Spark4B | few shot | 7 | 7 | 0 | 0 | 3 | 1111 |
| LFM1.2 Instruct | zero shot | 1 | 3 | 4 | 4 | 3 | 247 |
| LFM1.2 Instruct | few shot | 2 | 4 | 3 | 3 | 3 | 285 |

## 3. Interpretation and next experiment

The earlier ID/role failures were not a fair ceiling on direct editing capability. This result supports pursuing whole-transcript output with separate preservation checks. It does not show that any model is ready for unrestricted automatic delivery: the strongest zero-shot mode deletes meaningful Portuguese emphasis and a literal repeated letter in development cases, while its few-shot mode misses two clear fillers.

The next experiment should isolate transcript framing. Several smaller models execute “Repeat the words…” inside the transcript or echo a demonstration instead of editing the latest input. Add a clear transcript-as-data envelope and instruction to preserve its commands as content, comparing it with this frozen baseline. Then separately test an example covering a contiguous filler run and an abandoned function-word fragment. Combining both changes at once would obscure which fixes the residual behavior.

Freeze the resulting prompt and compare the small model with the Qwen4B quality control on a new semantically authored blind set. Include meaningful `yeah`, literal/multilingual fillers, numbers, negations, names, code, deliberate repetitions and factual endings. Gold should describe intended speech rather than a prior guard's limitations. Assess raw output first; a minimal lexical alignment can reject new words or severe deletion, but an arbitrary candidate vocabulary must not define model capability.

## 4. Reproduction and limits

Use the parent study's isolated requirements on Apple Silicon and supply one pinned local artifact from `models.json`. The harness reuses only the existing load helper, overrides its local model path, and bypasses its candidate/ID prompt completely. The local path and offline tokenizer prevent model downloads at inference.

```bash
HF_HUB_OFFLINE=1 HF_HUB_DISABLE_IMPLICIT_TOKEN=1 python benchmarks/llm/model-study-2026-09-08/direct-cleanup-diagnostic/run_model.py \
  --model /absolute/path/to/local/model --label qwen4000
```

By default the diagnostic writes the requested model's result JSON beside the harness. Run a copied diagnostic directory outside the repository to preserve archived evidence. Both modes run deterministically with their native chat templates and `enable_thinking=False`. Output budget is `min(512, max(128, 2 * input_tokens + 32))`, based on full input length rather than an ID budget. Actual stop reason, budget, raw generation, latency and CPU time are recorded.

The measured environment was Apple M4/32GiB/macOS15.6.1, Python3.14.5, MLX0.32.2, mlx-lm0.31.3, transformers5.3.0 and huggingface-hub1.7.2. One model process ran at a time. A 4GiB MLX guideline was used; it is not a hard RSS limit. Cold load used normal OS file caches. Per-case diagnostic alarms and an external whole-model deadline bounded the short experiment. These short cases do not establish long-dictation deadlines, energy usage or ANE execution.

## 5. Frozen independent 60-case comparison

The original nine development cases showed a causal framing effect: adding the same transcript-as-data envelope to demonstrations and user input raised stock Mini2 from 6/8 exact and 7/8 lexical to 7/8 exact and 8/8 lexical. It returned the natural reported sentence in 904ms. Adding a separate abandoned-article/filler-cluster demonstration regressed Mini2, Qwen4 and Spark preservation; that variant was rejected before opening fresh labels. Qwen0.8 and Mini1B also received the envelope development check and remained less accurate.

The independent author froze 60 semantic cases (24 ordinary cleanup, 24 ordinary preservation, 4 adversarial cleanup, 8 adversarial preservation), and the root agent independently reviewed all labels before model inference. Gold describes intended speech, rather than an old candidate guard. The experimenter did not access the new labels until `semantic60-freeze.json` and its explicitly approved Qwen secondary-profile addendum were frozen. The common prompt is `prompt-envelope.json`; Spark and Qwen also have original few-shot secondary profiles chosen solely on development results.

| Model/profile | Exact /60 | Lexical /60 | Useful edits† /28 | Added-word cases | Required-word-loss cases | Protected-payload failures |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| MiniCPM5-2B · common | 50 | 51 | 24 | 1 | 5 | 4 |
| Pollard MiniCPM5-2B mixed · common | 50 | 51 | 23 | 0 | 4 | 4 |
| Spark4B · common | 42 | 44 | 14 | 0 | 2 | 2 |
| Spark4B · original few-shot | 45 | 47 | 18 | 1 | 2 | 2 |
| Qwen3.5-4B · common | 49 | 54 | 25 | 1 | 4 | 3 |
| Qwen3.5-4B · original few-shot | 43 | 46 | 17 | 1 | 2 | 1 |
| LFM2.5-1.2B Instruct · common | 22 | 23 | 13 | 28 | 34 | 14 |
| LFM2.5-350M · common | 17 | 21 | 12 | 13 | 33 | 15 |

† A useful edit is a lexical change on a cleanup case with no detected added words, required-word loss, or protected-payload failure. It can still be incomplete or have a formatting defect; it is not the complete-usable-cleanup rate or a safety proof. All 480 fresh requests finished normally. Each profile has a separate known-sentence warmup, excluded from all fresh quality and latency totals.

Root manual adjudication of Mini2 separates the failure types: replacing `184.75 dollars` with `$184.75` preserves the amount but violates requested wording; a remaining dash after deleting a filler is a formatting defect; three cases delete literal content; one translates Portuguese. Initial capitalized hesitations remain in four cases. Ordinary preservation is 24/24 exact, while ordinary complete accepted cleanup is 19/24; lexical completion is 20/24 and includes the dangling-dash defect.

Pollard and stock Mini2 differ on only two fresh outputs: Pollard preserves Portuguese in one case but misses an ordinary hesitation in another. Both have 50/60 exact outputs, while Pollard costs about 0.62GB more artifact bytes and is slower here. Qwen common has higher lexical completion but corrupts an email address, rewrites ordinary wording, and removes meaningful emphasis. Spark is more conservative but leaves many clear fillers. The tiny Liquid references are substantially less accurate in this direct-output task. Model identities, actual parameter counts, artifact revisions and license/native-runtime caveats are in `models.json` and the [primary-source research](../../../../docs/research/2026-09-08-direct-cleanup-model-options.md).

| Model/profile | Median ms | P95 ms | Maximum ms | Model load ms | Process peak RSS GB | Metal peak GB | CPU seconds /60 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| MiniCPM5-2B · common | 796 | 2882 | 3354 | 1066 | 1.76 | 1.93 | 15.95 |
| Pollard MiniCPM5-2B mixed · common | 1196 | 3774 | 5825 | 1623 | 1.92 | 2.54 | 24.12 |
| Spark4B · common | 2024 | 6685 | 8150 | 1561 | 2.33 | 2.86 | 29.47 |
| Spark4B · original few-shot | 1817 | 6074 | 8670 | 1570 | 2.24 | 2.84 | 32.38 |
| Qwen3.5-4B · common | 2391 | 7331 | 8169 | 2233 | 2.47 | 3.14 | 47.87 |
| Qwen3.5-4B · original few-shot | 1851 | 7161 | 7664 | 2460 | 2.30 | 3.03 | 53.47 |
| LFM2.5-1.2B Instruct · common | 520 | 1060 | 2988 | 541 | 0.90 | 1.08 | 9.51 |
| LFM2.5-350M · common | 193 | 265 | 588 | 341 | 0.47 | 0.69 | 4.66 |

These are full inference requests from template/tokenization through generation and cache clearing, excluding IPC, model loading and JSON serialization. They are not directly comparable with the older generation-only latency column above. Model load excludes Python/library imports and uses ordinary OS file caches; it is not a cold-disk benchmark. P95 uses nearest rank `ceil(0.95*N)`. Process peak RSS includes load/warmup, and Metal peak includes resident model allocation; they overlap on unified memory and must not be added. The machine ran only one inference process at a time, with light source/JSON housekeeping; it was not an instrumented power laboratory. No watts or ANE execution were measured. Maximum prompt-plus-budget was 702 tokens across these frozen profiles, below every declared configuration limit; long-context support was not tested.

Reproduce a common profile with the pinned local artifact and isolated runtime:

```bash
HF_HUB_OFFLINE=1 HF_HUB_DISABLE_IMPLICIT_TOKEN=1 python run_direct.py \
  --model /absolute/path/to/pinned/local/artifact --label minicpm2000 \
  --prompt prompt-envelope.json --cases semantic60.json \
  --warmup-cases reported-only.json --modes few_shot --output /tmp/reproduced-model.json
python score_semantic.py --results /tmp/reproduced-model.json
```

For Spark only, prepend the archived `spark-runtime` to `PYTHONPATH` and add `--spark`; it registers the reviewed, pinned native architecture only in that process. The adapter does not modify MLX-LM or require remote model code. Use `prompt.json` for the two secondary profiles. The immutable freeze files preserve runtime versions, model pins, context configurations and source hashes; their original experiment paths are provenance, not required reproduction paths.

## 6. Native reasoning pilot and next development

LFM2.5-2.6B is an always-thinking model; its native template was preserved. The initial greedy run retained eight requests before a documented feasibility stop: five final answers and three 30-second timeouts. Remaining cases were not run and have no fabricated score. A final-envelope sample with an adequate 4,096-token/90-second budget completed in 37.94 seconds after approximately 1,808 retokenized reasoning tokens, returning “all the things” and losing the intended `those`. A four-case envelope pilot produced two finals (7.67s and 28.38s), two 30s timeouts, and only one exact answer. Thus this profile failed the same 10-second deployment target; a capped reasoning request is reported as incomplete, not semantic inability.

A separate native-MLX recommended-sampling pilot (temperature0.1, top-k50, repetition penalty1.1, explicitly recorded context window and seed) completed the sample in16.39s and two of four pilot cases in11.41s/27.54s; the other two reached30s timeouts. Zero of these five requests met10s. This distinguishes greedy behavior from broader model capability while documenting the remaining latency limitation. It does not replace the frozen comparison. After all frozen profiles were archived, the revealed60 was explicitly reclassified as development for further single-change prompt work. New release250 and restart12 cases remain separate and unopened until a model/prompt/validator freeze and independent approval. No automatic model is qualified by this report alone.

The subsequent [D1–D4 development record](development/README.md) is separate from the frozen comparison. The selected D3 prompt combined with the [v3 source validator](validation-prototype-v3/README.md) delivered 31/33 complete development cleanups and 36/36 preservation cases; these are development results, not release qualification. The [v4 parser repair](validation-prototype-v4/review.json) retained the same development outputs. The [v5 repair](validation-prototype-v5/source_validation.py) closes the independently discovered code-delimiter gap without changing those development outputs. The [separate qualification freeze](qualification/freeze-v5-final.json) pins D3, v5, model artifact, runtime, deadlines, and scorers before the independent release data is opened.

The subsequent [independent D3/v5 qualification](qualification/RESULTS.md) did not pass: delivered ordinary cleanup reached 90/100 lexically complete and all 100 ordinary preservation cases, but missed required recall and accepted four adversarial word losses. The earlier development results therefore do not justify automatic promotion.

The [static-prefix cache experiment](prefix-cache/README.md) is archived as prepared source only. Its pure helper checks passed, but no cache inference or timing run has occurred; performance work remains separate from cleanup qualification.
