# Small Local Cleanup Models and Runtime Measurements

- **Version:** 1.0
- **Date:** 2026-09-08
- **Status:** Implemented

## Table of Contents

1. [Decision and scope](#1-decision-and-scope)
2. [Current model survey](#2-current-model-survey)
3. [Locked evaluation](#3-locked-evaluation)
4. [Fixed protocol results](#4-fixed-protocol-results)
5. [Runtime and resource findings](#5-runtime-and-resource-findings)
6. [Implementation consequences](#6-implementation-consequences)
7. [Artifacts and next gate](#7-artifacts-and-next-gate)

## 1. Decision and scope

The initial 25-case comparison was too easy to establish safe cleanup quality. Its stock LFM2.5-350M result of 24/25 does **not** justify calling that model the best modern choice, or treating the deletion vocabulary as semantically safe. A new independently reviewed 96-case evaluation found 32 cases where the same production model/prompt deleted text the reference preserved. None of the 14 generative checkpoints or the specialist encoder qualified for automatic cleanup in the tested policies. Keep cleanup off by default and retain the fast stock model only for experimental History suggestions that require explicit review and Copy. Ordinary dictation must use ASR plus saved dictionary corrections; no AI suggestion is automatically pasted.

The task is narrow: remove clear hesitation fillers and accidental repetitions, preserving wording, facts, order, paragraph boundaries, names, and quoted/code content. Deletion-only output prevents invention and arbitrary rewriting, but words such as `um` may themselves be meaningful. Valid candidate IDs are therefore a structural guarantee, not a semantic guarantee.

This research supports the [approved follow-up specification](../specs/2026-09-08-vocabulary-settings-performance.md). It supplements the [initial cleanup journal](../journals/2026-09-08-conservative-cleanup.md), whose earlier measurements remain reproducible but have a narrower conclusion.

## 2. Current model survey

Primary model cards, official release notes, public Hub metadata/configs, and available MLX artifacts were checked on 2026-09-08. Model-repository creation and update timestamps do not necessarily equal release dates. Quantized Hub parameter totals may count packed storage rather than the original model's parameters; the table uses original model sizes or labels and identifies size exceptions.

| Family | Relevant checkpoint | Evidence and role |
| --- | --- | --- |
| OpenBMB | MiniCPM5-1B, MiniCPM5-2B | Official releases May19 and Sep7,2026. Standard Llama architecture, official MLX4bit artifacts, Apache2.0; actual totals1.081B and2.517B. Newer independent family, slightly above the preferred size. [Release log](https://github.com/OpenBMB/MiniCPM), [1B card](https://huggingface.co/openbmb/MiniCPM5-1B), [2B card](https://huggingface.co/openbmb/MiniCPM5-2B). |
| Liquid | LFM2.5-230M,350M,1.2B-Instruct | Official MLX4bit conversions;230M artifact appeared June2026, after the original350M selection.230M/350M size controls plus1.2B quality control. [230M card](https://huggingface.co/LiquidAI/LFM2.5-230M), [350M artifact](https://huggingface.co/LiquidAI/LFM2.5-350M-MLX-4bit), [1.2B artifact](https://huggingface.co/LiquidAI/LFM2.5-1.2B-Instruct-MLX-4bit). |
| Alibaba | Qwen3.5-0.8B,2B | Modern hybrid architecture; community4bit conversions; no-thinking template. The2B original contains2.274B total parameters including vision, while mlx-lm loads its text model. [0.8B card](https://huggingface.co/Qwen/Qwen3.5-0.8B), [2B card](https://huggingface.co/Qwen/Qwen3.5-2B). No official sub1B Qwen3.8 checkpoint was verified in the current catalog. |
| TII | Falcon-H1-Tiny-90M-Instruct | January2026,91.1M parameters, hybrid Transformer/Mamba, English, Falcon license. Official card explicitly lists MLX support. [Primary card](https://huggingface.co/tiiuae/Falcon-H1-Tiny-90M-Instruct). |
| IBM | Granite4.0-350M and4.0-H-350M | October28,2025 instruct models, Apache2.0, independent dense/hybrid controls; actual original sizes352M/340M. [Dense card](https://huggingface.co/ibm-granite/granite-4.0-350m), [hybrid card](https://huggingface.co/ibm-granite/granite-4.0-h-350m). |
| Google | Gemma3-270M-IT and FunctionGemma270M-IT | Independent tiny instruction/function-calling controls, public community MLX artifacts. FunctionGemma is task-specialized, so failure on ordinary JSON classification is not a general capability claim. [Gemma card](https://huggingface.co/google/gemma-3-270m-it), [FunctionGemma card](https://huggingface.co/google/functiongemma-270m-it). |

Additional current releases were considered and excluded from this small cleanup comparison:

- The latest official Qwen3.8 releases are27B and2.4T-A95B in August2026. The same official repository still identifies the small0.8B/2B/4B models as Qwen3.5 releases from March2,2026. A newer series name alone does not establish a newer official small checkpoint. [Primary release history](https://github.com/QwenLM/Qwen3.8/blob/main/README.md).
- Tencent's Hunyuan0.5B-Instruct is a July30,2025 release, despite secondary pages with2026 update dates. It is another older control, not a newer-than-LFM2.5 replacement. [Primary card](https://huggingface.co/tencent/Hunyuan-0.5B-Instruct).
- Gemma4 E2B/E4B are effective-size names, with approximately5B/8B total weights, outside the preferred tiny resident footprint. [Official release](https://blog.google/innovation-and-ai/technology/developers-tools/gemma-4/).
- Ling3.0-tiny is an August2026 hybrid MoE with7.9B total weights and1.3B active per token, not a1.3B memory footprint. Its current Mac recipe needs special runtime support. [Primary card](https://huggingface.co/inclusionAI/Ling-3.0-tiny).
- IBM released Granite4.2-3B on August25,2026, after4.1, with an official4bit MLX artifact and optional low-effort/native reasoning. This corrects the initial catalogue's4.1 listing. Its official MLX conversion was measured as the final larger control below. The original dense configuration contains a derived 3.660B parameters, despite the 3B label. NVIDIA Nemotron3 Nano4B and SmolLM3-3B remain unmeasured alternatives; no exhaustive best-model claim is made. [Granite4.2 card](https://huggingface.co/ibm-granite/granite-4.2-3b), [official MLX artifact](https://huggingface.co/ibm-granite/granite-4.2-3b-q4-mlx), [NVIDIA announcement](https://huggingface.co/blog/nvidia/nemotron-3-nano-4b), [SmolLM3 card](https://huggingface.co/HuggingFaceTB/SmolLM3-3B).
- HuggingFaceTB nanowhale100M is explicitly an educational, undertrained, sometimes incoherent release requiring custom code. [Primary card](https://huggingface.co/HuggingFaceTB/nanowhale-100m).
- NVIDIA Privasis-Cleaner0.6B is a privacy-redaction specialist trained to remove sensitive facts and names, with a noncommercial license; that behavior is contrary to preserving the user's dictation. [Primary card](https://huggingface.co/nvidia/Privasis-Cleaner-0.6B).

### Specialist encoder alternatives

Disfluency token classifiers deserve consideration because one encoder pass can label a whole transcript without generating text. Google's published small-BERT work supports that architecture as an on-device research direction, but does not establish a currently packaged, validated replacement for this app. [Primary research](https://research.google/blog/identifying-disfluencies-in-natural-speech/).

An approximately151MB INT8 ONNX export of a150M ModernBERT classifier is described by its converter as intended for Mów. It has separate filled-pause/repetition/revision labels. Only the first two could authorize an existing Rust candidate; revisions must stay protected. The converter declares CC BY4.0, while the original checkpoint's visible card omits a license and describes its evaluation only as a disfluency dataset, with self-reported overall F1 .754. Converter accuracy claims have not been independently reproduced; the later CPU diagnostic below measures this artifact directly. These provenance gaps prevent immediate promotion. [Converter card](https://huggingface.co/sam-castro/mow-disfluency-classifier), [original checkpoint](https://huggingface.co/arielcerdap/modernbert-base-multiclass-disfluency-v2).

Live metadata places the original checkpoint in March2026 and its ONNX conversion in April2026. Neither original nor Teloxico repository contains a root license file, and their current card metadata does not declare one. The converter's linked Mów repository also lacks its claimed export script or a current ONNX/disfluency implementation in its recursive source tree; the listed third-party licenses do not resolve the model license. This limits both reproducibility and adoption evidence. Metadata is captured in `encoder-catalog.json`. [Linked repository](https://github.com/krokoko/mow), [third-party notices](https://github.com/krokoko/mow/blob/main/docs/THIRD_PARTY_LICENSES.md).

A later local diagnostic tested the public ModernBERT ONNX export at revision `e1f59b45e03988dd55b8ff307c602f0d4567bf8c`, using a separate ONNX Runtime1.29.0/tokenizers0.23.2/NumPy2.5.3 environment and one CPU thread. Only FP labels could accept a Rust hesitation candidate and RP labels a repetition; revision and partial-word labels never authorize edits. On the eight separate development cases, it achieved1/8 exact,5 harmful cases, and safe useful edits on2/4 edit cases. The encoder also confuses literal uses with hesitation, so it is rejected before independent validation. It loaded in0.393seconds; median complete eligible case was8.24milliseconds with388MB peak RSS. Those numbers establish a fast diagnostic path, not a useful product replacement or ANE execution. The model was downloaded into the experiment directory; production dependencies, model weights, and settings were not changed.

Teloxico's ModernBERT disfluency card reports high precision but lacks dataset and license metadata, so its production-readiness claim is insufficient evidence. The29M stillerman deletion tagger explicitly warns that its training mix includes noncommercial DailyDialog data. Both are potential research controls, not current replacement recommendations. [Teloxico card](https://huggingface.co/Teloxico/modernBERT-4-disfluency), [29M tagger card](https://huggingface.co/stillerman/fdt-disfluency-small-29m).

## 3. Locked evaluation

The96-case synthetic set was written before inference, then independently reviewed in full by the primary agent. All labels were accepted unchanged. SHA256: `aa8c89b9d7b1e152767ede77286b395b3a2c75a01c4e9637077cf7b7a46f15f1`. It contains40 cases needing edits and56 requiring byte-identical preservation, including ambiguous literal fillers, deliberate repetitions, multilingual meaning, mixed content, embedded instructions, quotes/code, boundaries, and long text.

Production Rust candidate enumeration is exercised through the compiled `cleanup_edits` example, and the actual production Python module supplies the prompt, generation budget, strict ID parsing, and reconstruction boundary. Of96 cases,71 are eligible for inference (40 edit cases,31 preservation cases);25 bypass the model. Both denominators are reported so bypasses cannot inflate classifier quality. The fixed prompt includes its original one positive demonstration, unchanged across all candidates, native chat templates, greedy decoding, and `enable_thinking=False`.

A harmful deletion means the expected text can no longer be obtained by deleting only additional bytes from the model's output. This mechanically detects removal of reference content; specific semantic claims still require case inspection. Exact match, preservation cases unchanged, useful safe edits, and malformed/unfinished generations are reported separately. Returning malformed output is a safe fallback, not a correct classifier decision.

The developer uses8 separate development cases for prompt experiments; individual v2 failures are not used for prompt tuning. The primary agent independently locked a further72 cases (32 edits/40 preservation), SHA256 `73978a8658e7a4147c4866b71fd7514409a728c75d8857b3bd48a54a906cce9c`, with contents withheld until the revised prompt/guard is frozen.

## 4. Fixed protocol results

Initial10-model sweep on Apple M4/32GiB/macOS15.6.1, isolated Python3.14.5, MLX0.32.2, mlx-lm0.31.3, transformers5.3.0, huggingface-hub1.7.2. Models ran serially, one process per model. No private transcript was read and no live configuration or production model weights were changed. All public experiment weights are under `/tmp/experiments`.

| Model | Exact /96 | Eligible exact /71 | Needed exact /40 | Safe useful edits /40 | Preservation unchanged /31 eligible | Harmful cases | Fallbacks | Warm median ms | Peak Metal GiB | Post-run RSS MiB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| liquid350 | 57 | 32 | 27 | 31 | 5 | 32 | 8 | 87.1 | 1.28 | 340 |
| qwen800 | 46 | 21 | 14 | 17 | 7 | 30 | 15 | 257.8 | 1.43 | 941 |
| liquid230 | 58 | 33 | 2 | 3 | 31 | 3 | 65 | 92.6 | 0.66 | 343 |
| falcon90 | 59 | 34 | 10 | 13 | 24 | 12 | 46 | 77.7 | 0.74 | 237 |
| granite350 | 53 | 28 | 7 | 8 | 21 | 16 | 47 | 115.5 | 0.87 | 499 |
| graniteh350 | 55 | 30 | 0 | 0 | 30 | 1 | 70 | 156.3 | 1.45 | 491 |
| gemma270 | 56 | 31 | 4 | 7 | 27 | 9 | 55 | 192.1 | 0.85 | 1194 |
| functiongemma270 | 57 | 32 | 3 | 4 | 29 | 7 | 60 | 189.0 | 0.86 | 1182 |
| liquid1200 | 48 | 23 | 17 | 19 | 6 | 32 | 20 | 229.8 | 1.19 | 831 |
| qwen2000 | 54 | 29 | 26 | 27 | 3 | 36 | 8 | 470.1 | 1.99 | 1341 |
| minicpm1000 | 57 | 32 | 28 | 29 | 4 | 32 | 10 | 216.2 | 1.22 | 798 |
| minicpm2000 | 57 | 32 | 25 | 26 | 7 | 32 | 1 | 507.1 | 1.99 | 820 |

No candidate passes the preservation/usefulness gate with the existing prompt. Some tiny models achieve apparently reasonable overall exact scores primarily because malformed output falls back unchanged. This is evidence that the task framing and demonstrations need work, not a reason to promote the model with the highest aggregate score. The official MiniCPM5 artifacts loaded successfully with the same pinned runtime, but neither improved preservation with this prompt. Their idle samples were only0.1seconds and are not used for idle CPU conclusions. The revised-prompt validation below also rejected its development winners.


### Separate development iteration

A research-only balanced few-shot prompt added explicit meaningful-word, foreign-language, and mixed-deletion negative examples. It was evaluated on the8 development cases, never the unseen72-case validation. None of12 models met a safe useful development gate: MiniCPM5-2B reached6/8 exact but made2 mixed-case harmful deletions; Qwen3.5-0.8B/2B each reached5/8 with1 harmful deletion, and stock350 reached3/8 with5 harmful deletions. Granite hybrid's zero harm came from7 malformed/no-op fallbacks and zero useful edits.

The next experiment asks each model to compare the original text with one exact Rust-proposed deletion and choose one token: `0` preserves the original, `1` accepts the deletion. The prompt has seven balanced demonstrations, separate from the eight development cases. The unchanged Rust guard supplies thirteen candidate judgments across seven eligible development cases. Exploratory log-likelihood margins were recorded, but they are not calibrated confidence scores; low-precision normalization even produced label probability sums slightly above one, so those sums are not used as safety evidence.

| Model | Development exact /8 | Harmful cases | Edit cases receiving safe useful edits /4 | Median per-candidate ms |
| --- | ---: | ---: | ---: | ---: |
| LFM2.5-350M | 4 | 1 | 1 | 138.6 |
| Qwen3.5-0.8B | 6 | 2 | 3 | 347.0 |
| LFM2.5-1.2B-Instruct | 4 | 0 | 1 | 392.0 |
| Qwen3.5-2B | 4 | 0 | 0 | 668.0 |
| MiniCPM5-1B | 3 | 2 | 2 | 333.0 |
| MiniCPM5-2B | 7 | 0 | 4 | 874.8 |

MiniCPM5-2B is the first promising development candidate, at a materially larger2.52B actual parameter count and1.43GB artifact. This result supports fresh validation, not promotion. Its proposed classifier accepts only when the unconstrained greedy first token is the tokenizer's single `1` token; `0` preserves, any other token fails the entire request closed. No margin threshold is selected. The whole classification request has a10-second deadline and preserves the original on timeout; this prevents candidate count from creating unbounded post-dictation delay.

The complete candidate configuration was frozen before access to the independent72 cases: `binary-classifier-v1.json`, SHA256 `9f29c47e6efca2d02b558575dac13b84d11d971b29b13ebf89dac3446d20ea1e`, using official revision `32f8dd5df1188512a20413f1297083238306634c` and unchanged Rust guard SHA256 `c575b765ed95a42ebaaba5e059a79b63492dabf30b4b9763c0d81fc99547cc30`. `candidate_classifier.py` reconstructs each proposed edit through the actual Rust adapter. No production prompt or model has changed. Synthetic prefix-cache optimization is postponed until the classifier passes independent validation.

### Independent validation rejected the development winner

The frozen MiniCPM5-2B classifier scored55/72 exact, or39/56 among eligible cases. It preserved35/40 preservation cases (19/24 eligible), but made7 harmful deletions overall, including2 mixed-edit cases. It produced safe useful edits on21/32 required-edit cases (65.6%) and exact edits on20/32. It fails both promotion requirements; the apparently good8-case development result did not generalize.

This run used a resident subprocess with an external10-second request deadline and15-second load deadline. A timeout would kill and reap the process, preserve the original, and reload for the next eligible case. There were zero deadline fallbacks and zero malformed-output fallbacks. Median eligible request latency was0.898s and maximum3.697s. Artifacts: `validate_binary.py`, `binary_worker.py`, `minicpm2000-binary-fresh72.json/log`. No individual validation raw text or failure examples were used to adjust the prompt. The next bounded comparison adds a4B control on development data; this failed classifier is not promoted or optimized.

Qwen3.5-4B was added as a larger control with the identical prompt and policy, using public MLX revision `0e7ffd5c629ef7719d4cbc04069232580bfa9d9c` (3,061,129,077 artifact bytes). Its development result was6/8 exact, zero harm, and safe useful edits3/4, with1.594s median per candidate. Because four positive development cases were too few to reject usefulness conclusively, the unchanged policy was also evaluated on the same72-case independent set. It achieved54/72 exact (38/56 eligible), but9 harmful preservation cases; useful safe edits23/32 (71.9%), median1.646s, maximum6.738s, and zero fallbacks. It also fails promotion. The changed configuration contains only the model identity/revision difference and has SHA256 `9b7391e94f6d8a69cb6cab426f049e33f7416b8d66fda05b2d609255fceae2c1`. Artifacts: `downloads-qwen4.json`, `qwen4000-binary-dev.json`, `qwen4000-binary-fresh72.json/log`. [Original model card](https://huggingface.co/Qwen/Qwen3.5-4B), [MLX artifact](https://huggingface.co/mlx-community/Qwen3.5-4B-4bit).

### Exact occurrence and semantic-role development

The next development hypothesis exposed the exact Rust candidate occurrence. A short explanation followed by KEEP/DELETE, using before/marked/after fields, degraded MiniCPM2B to4/8 exact with3 harmful cases and Qwen4B to3/8 exact with2 harmful cases. Qwen4B native thinking with a512-token generation budget then exceeded the external10-second whole-request deadline on all seven eligible development cases. Raising its experimental MLX guideline from2GiB to4GiB left all seven timeouts unchanged. Raw fallback caused zero harm and zero useful editing; it is not a successful semantic classifier.

A separate role-label experiment keeps the original sentence continuous, surrounding only the Rust-supplied occurrence with `<target>` markers. The model classifies it as CONTENT, FILLER, REPEAT, or UNCERTAIN. Only FILLER/REPEAT authorize existing Rust IDs; malformed or unfinished output preserves the whole request. Seven balanced demonstrations remain separate from the eight development cases. Greedy decoding, non-thinking mode, eight generated tokens per candidate, and a4GiB experimental allocator guideline are fixed.

| Model | Development exact /8 | Harmful cases | Safe useful edit cases /4 | Median per-candidate seconds | Maximum whole-case seconds |
| --- | ---: | ---: | ---: | ---: | ---: |
| MiniCPM5-2B | 3 | 1 | 2 | 0.796 | 2.379 |
| Qwen3.5-4B | 6 | 0 | 4 | 1.388 | 4.413 |

The Qwen4 role configuration was frozen with SHA256 `828cca149cffc461a102c04a2a6801c66574f5d9bcfd07ac9d5c17d6fe549fa8`; classifier source `5f272e3bd3cde03d96b907c850cf35c4d325982103ee4f5aaefb584425fd3c5c`; read-only guard snapshot `4450a44c7864f8460d14beb0ceb3df2b70731ff14dd3aebfbbba28cda14451d4`. The primary agent locked a fresh80-case set before seeing this development outcome and revealed it after the freeze. Its SHA256 is `ee76248a71d5e9c3d5b7b1855a7e495cc836213231a758785b67feb9b0b006c5`. No individual72-case validation failures were used to tune these prompts.

The fresh role validation also **fails promotion**:56/80 exact,5 harmful cases,21/40 needed-edit cases receiving safe useful changes (52.5%), and36/40 preservation cases unchanged. Of61 inference-eligible cases,40 were exact;37 needed edits and24 required preservation. Nineteen cases bypassed inference, including three required edits outside the guard's vocabulary. No formatting or deadline fallbacks occurred. Median complete request latency was1.425seconds; maximum5.514seconds. This is a substantially different conclusion from the small development result; individual fresh80 failures are not used for further prompt tuning.

### Final unchanged-policy controls and product decision

The same frozen semantic-role policy was finally tested on cached Qwen3.5-0.8B and the new official IBM Granite4.2-3B MLX artifact. No system prompt, examples, labels, guard, or acceptance rule changed. These are model-only development comparisons, not new held-out results.

| Model | Development exact /8 | Harmful cases | Safe useful edit cases /4 | Median per-candidate seconds | Maximum whole-case seconds |
| --- | ---: | ---: | ---: | ---: | ---: |
| Qwen3.5-0.8B | 4 | 0 | 0 | 0.312 | 0.993 |
| Granite4.2-3B | 4 | 3 | 3 | 1.087 | 3.490 |

Qwen returned one malformed whole-case fallback and otherwise preserved all text, so zero harm did not establish useful cleanup. Granite returned valid role labels but deleted protected content in three cases. Neither qualified for fresh validation. The Granite artifact is official revision `0c6f39b1827afd5eb2c1c3b13751929857434953`, 2,066,181,467 downloaded bytes including its native `chat_template.jinja`. Cold load took 1.942 seconds and peak allocated Metal was 2,588,334,888 bytes. Its non-thinking template uses the same `enable_thinking=False` interface, without custom model code. [Official artifact](https://huggingface.co/ibm-granite/granite-4.2-3b-q4-mlx).

Stop model experimentation for this change. The study compared 14 generative checkpoints and one purpose-built encoder, using fixed comparisons, separate development data, and two independently authored fresh validation sets. No measured policy met both zero observed harmful deletion and at least 80% safe useful editing. A larger model, different response representation, and a specialist classifier did not solve the ambiguity reliably enough for automatic paste.

The accepted containment is review-only suggestions in History. The small stock LFM2.5-350M remains a fast experimental suggestion source, not a validated correction model or the best modern model. Rust still permits only exact source deletions. ASR plus the saved dictionary determines ordinary dictation text; a separate suggestion requires explicit review and Copy. This removes the demonstrated AI deletion risk from automatic dictation while retaining a useful place to evaluate the optional feature. Default-off alone would not have contained the known failure when enabled.

## 5. Runtime and resource findings

The live app runtime was MLX0.31.1/mlx-lm0.31.1. A separate environment was created for MLX0.32.2/mlx-lm0.31.3. The latter matters for fair contemporary comparison: mlx-lm0.31.2 fixed Qwen3.5 cache advancement and added non-trimmable prefix-cache reuse;0.31.3 fixed ArraysCache and generation stream handling. [Official release notes](https://github.com/ml-explore/mlx-lm/releases). No live environment was upgraded during measurement.

Cold figures mean a fresh model process with local weights and normal OS caches, not a purged disk cache. Across the initial sweep, import+load took0.65–2.57s; the first completed cleanup arrived1.00–3.63s after harness entry. The slowest initial-sweep warm eligible case was2.07s (Qwen2B long input); the subsequent MiniCPM5-2B long case took2.42s. These measurements justify seconds-scale local failure deadlines with headroom; they do not justify waiting five minutes for a hung classifier.

The stock350 process retained approximately340MiB RSS and211MiB actively allocated Metal memory after its sweep. Its three-second idle sample used0.00145 CPU seconds. That suggests quiet idle execution but retained weights; it does not establish idle power consumption. Across models, post-run RSS ranged roughly237–1341MiB. Metal allocator peaks and OS RSS are different measurements and must not be added together. Direct watt counters were unavailable because `sudo -n powermetrics` required a password.

`mx.clear_cache()` frees allocator cache, not referenced model weights. `mx.set_memory_limit()` is documented as an allocation guideline, not a hard process-RSS cap. Clearing cache after each request therefore does not unload the model or guarantee a2GiB process limit. [MLX memory management](https://ml-explore.github.io/mlx/build/html/python/memory_management.html), [memory limit semantics](https://ml-explore.github.io/mlx/build/html/python/_autosummary/mlx.core.set_memory_limit.html), [clear-cache semantics](https://ml-explore.github.io/mlx/build/html/python/_autosummary/mlx.core.clear_cache.html).

The first-token validation inherited the production2GiB allocation guideline. Qwen4B exceeded that guideline (2.68GB peak allocated Metal) and accumulated106.19 CPU seconds, versus MiniCPM2B's8.64 CPU seconds and2.04GB peak. A larger model may cause extra allocator synchronization under that inherited limit, so this is a measurement of the tested configuration, not an intrinsic model power ranking. A useful larger candidate would require a separate guideline comparison before resource promotion. Neither candidate qualified on accuracy, so that optimization has not been performed.

## 6. Implementation consequences

Source inspection found unbounded operations independent of model quality: the runtime readiness probe blocks on a Python import; startup/load runs before the cleanup timeout and recorded its PID only after loading; shutdown sends a blocking JSON request before starting its supposed three-second timeout. Preparation also had no real shared state and the cancellation command reported success without cancelling anything.

Approved implementation now moves explicit preparation and status file work off the UI thread, exposes pending/error status, serializes preparation, and validates load before the enabled draft can be accepted. Sidecar response reads and runtime commands receive independent deadlines; process identity is recorded before model loading; shutdown terminates the process without waiting for a JSON reply. Dictation bypasses cleanup while setup owns the model, preserving raw text. No experimental model or automatic-cleanup policy is promoted. The stock sparse classifier remains only as the explicitly reviewed suggestion source.

Formal application review2 additionally verified Tauri's direct process-exit behavior: managed-state Drop alone is insufficient for an in-progress sidecar load/download. Explicit Exit now terminates registered owned Child handles, including temporary model/update and Python/package-setup processes. A registry exit flag rejects late worker spawns. Weak references and actual Child state avoid signalling unrelated or recycled PIDs, and process ownership guards initialization failures. Explicit setup installs the exact tested package quartet (MLX0.32.2, mlx-lm0.31.3, transformers5.3.0, huggingface-hub1.7.2); readiness and Python inference require mlx-lm0.31.3 or newer. Existing compatible runtimes are upgraded in place, with no automatic deletion or interpreter rebuild.

No ANE claim is made for MLX/Metal inference, and no architectural rewrite or fine-tuning is proposed. Prefix-cache reuse and idle unloading require measured improvement before adoption, particularly because retaining a cache can retain transcript-derived state and weights.

### Follow-up: bare hesitation detection and review-suggestion utility

A later user test exposed a coverage bug: three bare `um` tokens and one `uh` yielded no candidates because the guard required commas. Standalone lowercase hesitations now qualify with or without a comma, while quotes, code, uppercase names, identifiers, paragraph bytes and final-token protection remain unchanged. Candidate-free bypasses are distinguished from a model selecting no edits. This is a detector repair, not evidence that every qualifying token is expendable.

A bounded same-model follow-up compared the prior prompt and three prompt-only variants on the supplied test sentence plus the unchanged 96-case regression set. An instruction that treats each ID as a distinct occurrence improved safe useful editing from 31/40 to 37/40, with harmful cases 33→31, under the expanded guard. Median eligible latency rose from 82.1ms to 99.7ms. On a new independently authored 16-case set opened only after prompt freeze, old and selected prompts were identical: 8/16 exact, five harmful cases, and 6/8 required-edit cases receiving safe useful changes. This supports a modest review-suggestion utility change; it fails automatic-cleanup safety.

Prompt representation was also significant. The first manual candidate dictionaries used `id,text,kind` order and selected `[3]`; actual Rust JSON serialization uses `id,kind,text`, for which both the old and selected prompt choose `[0]`. The final integrated production probe confirmed `[0]` with the current Rust adapter and installed runtime in 133ms. It removes only the first `um`, leaving three fillers. The example is not fully corrected, and ordinary delivery remains unchanged. Production prompt messages were verified against the frozen selected policy before adoption.

The model identity, generation/parser contract, runtime and cache locations remain unchanged. This limited follow-up does not reopen the model comparison or alter its no-automatic-promotion conclusion. Its frozen configuration, original and normalized independent fixtures, candidate-order diagnostic, six result sets, and integrated production probe are preserved in the [portable follow-up evidence](../../benchmarks/llm/model-study-2026-09-08/bare-hesitation-followup/README.md). The [follow-up specification](../specs/2026-09-08-cleanup-and-settings-followup.md) governs delivery and Settings-close changes.

## 7. Artifacts and next gate

Durable fixtures, protocol snapshots, pinned model identities, portable reproduction scripts, and compact per-case results are preserved in [the benchmark study](../../benchmarks/llm/model-study-2026-09-08/README.md). Temporary experiment weights and full logs remain under `/tmp/experiments/sotto-cleanup-overnight/`.

- `catalog.json`, `catalog-additions.json`: exact public metadata/config snapshots.
- `downloads.json`, `downloads-minicpm.json`: pinned revisions, local paths and artifact byte counts.
- `holdout-v2.json`, `holdout-v2-manifest.json`, `make_eval.py`: locked data and provenance.
- `development-v2.json`, `prompt_dev.py`: independent development examples and research prompt.
- `bench_v2.py`, `run_sweep.py`, `sweep-v2.log`, `*-fixed-v2.json/log`: reproducible per-case outputs and CPU/time/memory measurements.
- `venv-create.log`, `venv-install.log`: isolated runtime provenance.

The binary and semantic-role fresh gates both failed, as did the final unchanged-policy development controls and the specialist encoder diagnostic. The portable archived adapter built successfully, and both final controls completed through its portable role harness. Model identities and per-case result fingerprints are retained in the manifest. Any future automatic-cleanup proposal requires another independently authored validation set, zero observed substantive deletions, and safe useful edits on at least 80% of edit cases. Review-only suggestions are containment, not a claim that a failed model has become semantically accurate.
