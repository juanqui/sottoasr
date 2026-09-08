# Frozen direct-cleanup head-to-head

- **Version:** 1.0
- **Date:** 2026-09-08
- **Status:** Implemented

The comparison is complete: eight frozen profiles produced **480 completed fresh outputs**, plus eight separate excluded warmups. Stock MiniCPM5-2B with the common transcript-as-data envelope is the most promising speed/ordinary-preservation development candidate here. It cleans the reported sentence naturally, matches the larger Pollard artifact's exact score, and runs faster with less Metal allocation. **No raw model qualified for unrestricted automatic cleanup.**

## Contents

1. Frozen results
2. What the scores conceal
3. Requested native LFM2.6 pilot
4. Artifacts and reproduction

## 1. Frozen results

The independent author froze 60 cases and the root agent independently reviewed their gold before inference:24 ordinary cleanup,24 ordinary preservation,4 adversarial cleanup,8 adversarial preservation. The experimenter opened the data only after the common prompt/runtime freeze and the approved Qwen secondary-profile addendum. Every common profile used the same envelope and two examples. Spark and Qwen original few-shot profiles were chosen as secondary controls using old development results, before fresh labels were visible.

| Frozen profile | Exact /60 | Lexically complete ordinary cleanup /24 | Ordinary preservation /24 | Median / P95 ms | Metal peak GB | Process peak RSS GB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Stock MiniCPM5-2B | 50 | 20 | 24 | 796 / 2882 | 1.93 | 1.76 |
| Pollard MiniCPM5-2B mixed | 50 | 19 | 24 | 1196 / 3774 | 2.54 | 1.92 |
| Spark4B — common | 42 | 13 | 24 | 2024 / 6685 | 2.86 | 2.33 |
| Spark4B — original few-shot | 45 | 15 | 23 | 1817 / 6074 | 2.84 | 2.24 |
| Qwen3.5-4B — common | 49 | 23 | 22 | 2391 / 7331 | 3.14 | 2.47 |
| Qwen3.5-4B — original few-shot | 43 | 13 | 23 | 1851 / 7161 | 3.03 | 2.30 |
| LFM2.5-1.2B Instruct | 22 | 9 | 11 | 520 / 1060 | 1.08 | 0.90 |
| LFM2.5-350M | 17 | 10 | 7 | 193 / 265 | 0.69 | 0.47 |

Exact accepts the original gold plus its pre-approved alternatives, stripping only surrounding output whitespace. Lexical completion ignores case and ordinary punctuation, so it is supplemented by exact protected payloads and manual review. P95 is nearest rank `ceil(0.95*N)`. Warmups are excluded from all quality and request-latency columns. Timings include template/tokenization, generation and cache clearing; they exclude loading, IPC and output JSON serialization.

The canonical scores come from `score_semantic.py`, which unions the primary expected output with its alternatives. The frozen raw harness's `accepted_sample` field has six false negatives because it omitted the primary target when alternatives were present. Raw evidence remains unchanged; that field is not authoritative for fresh-case acceptance.

GB is decimal. Metal peak includes the resident model. Process RSS peak includes model loading and warmup; unified-memory measurements overlap and must not be added. Only one inference process ran at a time; light source/JSON housekeeping continued. These measurements do not establish watts, ANE execution, idle application footprint, or performance on another Mac.

## 2. What the scores conceal

- Stock Mini2 produces50/60 exact outputs and preserves all24 ordinary preservation cases. Its ordinary exact cleanup is19/24; lexical cleanup20/24 includes one dangling-dash formatting defect. Four inputs retain a leading capitalized hesitation. It deletes intended literal content in54/56/60 and translates Portuguese in58.
- Case08 changes `184.75 dollars` to `$184.75`. The amount is preserved; this is unwanted wording/format substitution and exact-payload failure, **not a lost amount fact**. Case22 leaves a dash after removing a hesitation.
- Pollard differs from stock on only two fresh outputs: it preserves Portuguese58 but misses a hesitation10. Both score50/60; the larger mixed quantization is not an across-the-board quality win.
- Qwen common corrupts `maya.chen+lab@example.org` into `Maya Chen+lab@example.org`, which lexical normalization alone misses. It also rewrites ordinary content and removes emphasis. Qwen's original few-shot profile executes a quoted-heading instruction and drops its surrounding dictated words.
- Spark's common profile completes only14/28 useful edit attempts without detected word/payload loss; its secondary reaches18/28. Both delete literal content56/60, and the secondary echoes a demonstration before one long transcript.
- Liquid1.2 and350 are faster but have much lower exact accuracy and frequent rewriting. A high speed does not make these raw outputs suitable for automatic dictation.

The raw generated text is scored directly: no candidate IDs, sparse-role protocol, or output validator restricts these results. Therefore this comparison measures the direct-editing request shape. Later validator experiments and prompt changes using the now-revealed60 are **development** and must be reported separately; their results cannot replace this frozen head-to-head or certify an automatic release.

## 3. Requested native LFM2.6 pilot

LFM2.5-2.6B is always-thinking, and its real native template was retained. The initial greedy feasibility run retained8 requests:5 final answers and3 request timeouts at30s; remaining planned requests were explicitly unrun. With the final common envelope, the supplied sentence completed under an adequate4,096-token/90-second allowance in37.94s, after approximately1,808 retokenized reasoning tokens. It returned “all the things,” losing the intended `those`. A separate four-case envelope pilot gave one exact answer, two timeouts, and only one request below the shared10-second deployment target.

The requested native-MLX recommended-sampling control uses temperature0.1, top-k50, repetition penalty1.1, native20-token repetition context, and per-request seed42. Its adequately budgeted sample completes in16.39s (~666 estimated reasoning tokens), still returning “all the things.” The sampler therefore affects latency materially, but this observed sample remains above the10s target. The four-case recommended pilot completed two exact answers in11.41s and27.54s, with two30s timeouts. Including the16.39s sample, zero of five requests met10s. These raw results are archived under `development/results/liquid2600-native-recommended-*.json`; neither pilot has an invented60-case score. MLX-LM0.31.3's repetition implementation is not identical to Transformers' full-input penalty; this is a documented native-MLX adaptation.

## 4. Artifacts and reproduction

- [Raw and scored profiles](results/), [aggregate metrics](semantic60-summary.json), [fixture](semantic60.json), and [protected payload annotations](semantic60-protected.json).
- [Common freeze](semantic60-freeze.json), [pre-reveal Qwen addendum](semantic60-freeze-addendum.json), [native direct-output driver](run_direct.py), and [offline scorer](score_semantic.py).
- [Exact model IDs, revisions, sizes, licenses and adapter caveats](models.json). Stock Mini2 tested revision32f8dd and publisher revision014e759 have identical weight SHA256 `c207798696a4a454e7ac211b25227625466c693335941cee8904fb922f295cc1` and identical inference configuration/template/tokenizer/index blobs; no re-download was needed.
- [Primary-source model/runtime/license research](../../../../docs/research/2026-09-08-direct-cleanup-model-options.md). Pollard is a quantization variant, not an ASR cleanup fine-tune. Spark requires the pinned reviewed native adapter; it was not loaded as a fictitious Llama model. VoiceInk Refine was excluded from generic Python/Sotto execution because its license restricts it to VoiceInk usage.

Runtime:Apple M4,32GiB,macOS15.6.1; Python3.14.5,MLX0.32.2,MLX-LM0.31.3,Transformers5.3.0,HF Hub1.7.2. Common profiles use native no-thinking templates, temperature0,4GiB Metal guideline,128MiB cache allowance,10s request deadline, and a full-output token budget `min(8192,max(128,2*input_tokens+32))`. The largest observed prompt-plus-budget was702 tokens, below every loaded model's declared context limit; this was not a long-context test. All downloads were pinned public artifacts in isolated experiment directories, and inference was offline. No production model, installed settings, or user caches were changed.

See [README](README.md) for the complete old zero/few-shot diagnostics, causal framing experiment, runtime commands and interpretation limits.

The subsequent [D1–D4 development record](development/README.md) is separate from the frozen comparison. The selected D3 prompt combined with the [v3 source validator](validation-prototype-v3/README.md) delivered 31/33 complete development cleanups and 36/36 preservation cases; these are development results, not release qualification. The [v4 parser repair](validation-prototype-v4/review.json) retained the same development outputs. The [v5 repair](validation-prototype-v5/source_validation.py) closes the independently discovered code-delimiter gap without changing those development outputs. The [separate qualification freeze](qualification/freeze-v5-final.json) pins D3, v5, model artifact, runtime, deadlines, and scorers before the independent release data is opened.

The subsequent [independent D3/v5 qualification](qualification/RESULTS.md) did not pass: delivered ordinary cleanup reached 90/100 lexically complete and all 100 ordinary preservation cases, but missed required recall and accepted four adversarial word losses. The earlier development results therefore do not justify automatic promotion.
