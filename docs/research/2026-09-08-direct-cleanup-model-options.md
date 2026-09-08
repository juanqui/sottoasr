# Direct Transcript Cleanup: Current Model and Runtime Sources

- **Version:** 1.0
- **Date:** 2026-09-08
- **Status:** In Review

## Table of Contents

1. [Scope and correction](#1-scope-and-correction)
2. [Requested comparison artifacts](#2-requested-comparison-artifacts)
3. [VoiceInk reference](#3-voiceink-reference)
4. [Other current candidates](#4-other-current-candidates)
5. [Experiment requirements](#5-experiment-requirements)

## 1. Scope and correction

The user reopened cleanup model selection and explicitly requested a direct comparison of LFM2.5, MiniCPM5/Pollard, and Spark. This follow-up supplements the [earlier model study](2026-09-08-small-cleanup-models.md). The earlier deletion-ID classification results do not establish how well these models produce cleaned transcript text through their native instruction interfaces. This document records source facts and reproducible model identities; it does not claim any unmeasured model is best or authorize a production change.

The current stock `LiquidAI/LFM2.5-350M-MLX-4bit` derives from the **instruction-tuned** `LiquidAI/LFM2.5-350M`. The separately named `LFM2.5-350M-Base` is pretrained. Calling the stock model an accidental base-model selection would be incorrect. The original Sotto fine-tune did start from Base, which is a separate historical fact. [Official checkpoint distinction](https://huggingface.co/LiquidAI/LFM2.5-350M), [March 31 release](https://www.liquid.ai/blog/lfm2-5-350m-no-size-left-behind).

## 2. Requested comparison artifacts

Public Hugging Face API metadata, pinned model cards, configurations, and templates were read on September 8. Byte totals below are downloaded files, not resident RAM. Parameter totals describe model weights rather than active-per-token marketing sizes. Repository timestamps are not necessarily release dates.

The cached official MiniCPM comparison uses revision `32f8dd5df1188512a20413f1297083238306634c`.
The table records the subsequently checked `014e7591ff4b1b6330cda7f45803b27f46288261`.
The primary Hugging Face API confirms identical weights, configuration, template,
tokenizers, generation configuration and index at both revisions. Both weight
files have SHA256 `c207798696a4a454e7ac211b25227625466c693335941cee8904fb922f295cc1`;
the comparison is not using different inference artifacts despite the repository
revision difference.

| Candidate | Exact artifact and revision | Size and native behavior |
| --- | --- | --- |
| LFM2.5-2.6B | [Official MLX 4-bit](https://huggingface.co/LiquidAI/LFM2.5-2.6B-MLX-4bit/tree/04efa23776ce61ec34ec95ec34c859854c89542b) | About 2.697B parameters; 1,583,152,892-byte weights. Affine group64, 4-bit except 6-bit embeddings. Upstream `lfm2` MLX implementation. LFM Open License1.0. |
| MiniCPM5-2B, official control | [Official MLX](https://huggingface.co/openbmb/MiniCPM5-2B-MLX/tree/014e7591ff4b1b6330cda7f45803b27f46288261) | 2.517B parameters; 1,426,229,683 total artifact bytes. Post-trained stock model, Llama architecture, native MLX, Apache2.0. |
| MiniCPM5-2B, requested Pollard | [Pollard MLX](https://huggingface.co/PollardWeights/MiniCPM5-2B-Pollard-MLX/tree/e07a8344ac3543dcf354cdb9792ce7b58ad1ad98) | Same 2.517B stock model, quantized without claimed cleanup training. Author recommends root mixed precision: 2,037,317,211-byte weights, 2,047,357,621 total root bytes. Apache2.0 card. |
| Spark-X2.5-4B | [Requested community MLX4bit](https://huggingface.co/abenzerps/Spark-X2.5-4B-MLX-4bit/tree/b23819d4d60c2767fbf6ee3b3527f5f33205be7e) | About 4.112B parameters; 2,313,395,808-byte weights. Affine4bit group64. `spark2_5` needs the official Spark adapter described below. Apache2.0. |

LFM2.5-2.6B was released August4. It is post-trained for instruction and agent tasks, rather than the separately distributed Base checkpoint. Its official card explicitly identifies it as **always reasoning**, and its native template always opens `<think>`. `enable_thinking=False` does not disable that behavior. Do not silently truncate its reasoning at a tiny output limit or compare unfinished reasoning against completed answers. No official non-thinking 2.6B variant was verified. [Official card and template guidance](https://huggingface.co/LiquidAI/LFM2.5-2.6B#chat-template), [release](https://www.liquid.ai/blog/lfm2-5-2-6b).

Its official generation recommendation is temperature `0.1`, top-k `50` and
repetition penalty `1.1`. A greedy diagnostic is a controlled profile, not that
recommended sampling configuration. If it spends excessive time reasoning, test
the published settings in a separately labeled, seeded pilot before attributing
the entire delay to model architecture. Preserve final-output fidelity in that
comparison: a repetition penalty is not automatically beneficial for copying a
transcript's intended words. [Generation settings](https://huggingface.co/LiquidAI/LFM2.5-2.6B#%EF%B8%8F-model-details).

MiniCPM5-2B was released September7, after MiniCPM5-1B on May19. The final checkpoints include SFT, reinforcement learning, and on-policy distillation; Base and SFT-only variants are separately named. Its native template supports explicitly disabling thinking. [Official release log](https://github.com/OpenBMB/MiniCPM#-changelog), [2B card](https://huggingface.co/openbmb/MiniCPM5-2B).

Pollard changes precision allocation, not the task objective. Its root mix assigns 107 quantized modules 8 bits and 189 modules 4 bits. The `q4/` alternative still preserves nine modules at 8 bits: embeddings, output head, and all seven projections in the final layer. Its weights are 1,707,015,353 bytes, with 1,717,052,296 total subdirectory bytes. `q8/` holds a third model copy. The author says perplexity and Mean-KLD benchmarking is pending, so improved cleanup quality cannot be assumed. [Pinned card](https://huggingface.co/PollardWeights/MiniCPM5-2B-Pollard-MLX/blob/e07a8344ac3543dcf354cdb9792ce7b58ad1ad98/README.md).

Pollard's tokenizer JSON, generation configuration, and chat template match the official MiniCPM5-2B MLX files byte-for-byte. The tokenizer configuration only omits the official `local_files_only: false` field. This supports a controlled same-prompt comparison of quantizations. Avoid downloading the whole Pollard repository: it includes all three variants.

Spark's upstream source is [`XHToken/Spark-X2.5-4B` at `ea14618d20e76b5b093d3ee20a5b9d733bb12410`](https://huggingface.co/XHToken/Spark-X2.5-4B/tree/ea14618d20e76b5b093d3ee20a5b9d733bb12410). It is post-trained from Spark-X2.5-4B-Base using SFT, reinforcement learning, and multi-domain on-policy distillation. Its architecture combines three sliding-attention layers per full-attention layer. The template supports `enable_thinking=False`; published general benchmark scores are in thinking mode and are not transcript-cleanup evidence. [Official card](https://huggingface.co/XHToken/Spark-X2.5-4B).

### Spark runtime preparation

Current upstream `mlx-lm` did not expose `spark2_5` during this check. The official [Spark-MLX-LLM adapter](https://github.com/XHToken/Spark-MLX-LLM/tree/de2b4379fa1e2f2e1f99d84c83f0e008f651d86c) provides its actual architecture. It requires `mlx-lm>=0.31.3,<0.32`, matching the existing isolated experiment environment. It must not be replaced with a guessed Llama configuration.

The adapter's `__init__.py`, `loader.py`, `registration.py`, and `model.py` were inspected before preparation. Its registration only adds the architecture module to the current Python process, and its loader uses strict weight validation. Those exact files, license, project metadata, README, and a SHA256 provenance manifest were placed in `/tmp/experiments/sotto-direct-cleanup/spark-runtime`. No package installation or production runtime modification was made.

Set that directory as the experiment process's `PYTHONPATH`, then call `spark_mlx_llm.registration.register_model()` before normal `mlx_lm.load(local_snapshot_path)`. The adapter preserves sensitive attention output gates in BF16. An actual successful load and end-to-end generation remain necessary; source compatibility alone is not runtime validation.

### Download boundaries

Use explicit immutable revisions and root files: `config.json`, `generation_config.json`, `chat_template.jinja`, `tokenizer.json`, `tokenizer_config.json`, `model.safetensors`, and `model.safetensors.index.json`, plus the model card and applicable license. For Pollard's optional q4 control, request only `q4/*` and load that subdirectory. The separate Liquid `-MLX-4bit` repository avoids the parent `-MLX` repository's many precision variants.

Pinned weight SHA256 values:

| Artifact | SHA256 |
| --- | --- |
| LFM2.5-2.6B4bit | `ef350b75815752f8bff8c064c9fc8bd8b5709577d72943436852013d2bcd4691` |
| Pollard root mix | `a6d5ec85704dbe0091c0c19578410382c2c47b041065f5ab3d500093a2b211dd` |
| Pollard q4 | `b072b44c3791d00e0701434c5b709c9a067494a7ecfb12a20c72c469762a7a54` |
| Spark4bit | `1fd6370f641e7fbffb2f57562793bf0493f48d55e3e85c8fe9091bd870a87e8b` |

## 3. VoiceInk reference

Current VoiceInk source contains a dedicated **VoiceInk Refine V1** path. The app pins `beingpax/VoiceInk-Refine-V1` revision `ad665418d3850e379e29236e66be3ddc0ac0bf04`. It is a task-specific Qwen3.5-2B fine-tune, quantized to4bits, for ASR transcript refinement. Its stated tasks include punctuation, capitalization, fillers, repetitions, spoken formatting, lists, and email paragraphs. The text model has approximately1.882B parameters; its weights are1,059,404,951bytes and the full artifact is1,079,479,368bytes. [Model card](https://huggingface.co/beingpax/VoiceInk-Refine-V1), [immutable application model selection](https://github.com/Beingpax/VoiceInk/blob/8f089cb4bf2c9c2f217b0cc0af909d9052ff6288/VoiceInk/Features/ModelLibrary/State/VoiceInkRefineService.swift).

Refine runs through native MLX Swift in an XPC service. The app restricts that option to Apple Silicon with at least16GiB physical RAM. It uses direct final-text generation, a short cleanup instruction, temperature0.3, and `enable_thinking:false`. Its output-token limit scales with input length and has an8192token ceiling. This is a distinct pipeline from deletion-ID classification. [Pinned inference engine](https://github.com/Beingpax/VoiceInk/blob/8f089cb4bf2c9c2f217b0cc0af909d9052ff6288/VoiceInkRefineXPC/VoiceInkRefineInferenceEngine.swift).

**Refine's published model license restricts use to VoiceInk and disallows use in other software, redistribution, and commercial use without the copyright holder's permission.** It is not a permissible Sotto drop-in under those published terms. Benchmark the installed VoiceInk application itself, or obtain the model owner's permission before use outside it. No Refine weights were downloaded or executed by this research task. [Pinned model license](https://huggingface.co/beingpax/VoiceInk-Refine-V1/resolve/ad665418d3850e379e29236e66be3ddc0ac0bf04/LICENSE.md).

VoiceInk also has a separate generic enhancement path: its prompt asks for minimal cleanup while preserving meaning and wording, encloses the transcript as input data, and returns cleaned text directly. The Refine branch bypasses the generic prompt selection. Neither source path uses candidate-ID classification. These implementation choices justify testing the direct-text task we actually want, while separately measuring factual preservation. [Generic prompt](https://github.com/Beingpax/VoiceInk/blob/8f089cb4bf2c9c2f217b0cc0af909d9052ff6288/VoiceInk/Core/Enhancement/AIPrompts.swift), [dispatch and transcript formatting](https://github.com/Beingpax/VoiceInk/blob/8f089cb4bf2c9c2f217b0cc0af909d9052ff6288/VoiceInk/Features/Enhancement/Workflows/AIEnhancementService.swift).

## 4. Other current candidates

These are research alternatives, not additions to the user's requested first comparison.

| Candidate | Verified relevance and limit |
| --- | --- |
| Qwen3.5-0.8B | Post-trained sub1B control with native MLX text support. No official sub1B Qwen3.8 checkpoint was found in the current publisher catalog. [0.8B card](https://huggingface.co/Qwen/Qwen3.5-0.8B), [publisher catalog](https://huggingface.co/Qwen). |
| MiniCPM5-1B | Actual1.081B; May19 release; official approximately618MB MLX artifact and Apache2.0. Slightly above the preferred1B ceiling. [Card](https://huggingface.co/openbmb/MiniCPM5-1B), [MLX artifact](https://huggingface.co/openbmb/MiniCPM5-1B-MLX). |
| Granite4.2-3B | August25 release, actual3.660B dense model, Apache2.0; official approximately2.066GB q4MLX artifact. Supports thinking and non-thinking modes. [Card](https://huggingface.co/ibm-granite/granite-4.2-3b), [MLX artifact](https://huggingface.co/ibm-granite/granite-4.2-3b-q4-mlx). |
| Nemotron3 Nano4B | March17 release, actual3.974B dense hybrid model; NVIDIA Open Model License. Upstream MLX architecture and community4bit artifact exist, but this specific Mac load was not measured here. It is older than the March31 LFM350 release. [Official announcement](https://huggingface.co/blog/nvidia/nemotron-3-nano-4b), [card](https://huggingface.co/nvidia/NVIDIA-Nemotron-3-Nano-4B-BF16), [community MLX](https://huggingface.co/mlx-community/NVIDIA-Nemotron-3-Nano-4B-4bit). |
| Gemma4 E2B-IT | Instruction-tuned, but E2B is an effective-size label: about5.1B full parameters. E4B is about8B full. They do not satisfy a literal2–4B total-weight budget. [Official card](https://huggingface.co/google/gemma-4-E2B-it). |

## 5. Experiment requirements

1. Compare direct cleaned-text output from the same source transcripts with model-native templates. Keep the current350M as a reference and the official MiniCPM quant as a matched control.
2. Give always-reasoning LFM enough total tokens to complete its final answer. Record reasoning mode, sampling settings, token budget, stop reason, complete-response latency, and memory separately. A timeout or output limit is a runtime result, not a scored final answer.
3. Score useful filler/repetition cleanup independently from factual preservation, self-corrections, names, numbers, negation, code, quotations, paragraph boundaries, and dictated instructions. Test short, normal, and long inputs.
4. Freeze prompts before an independently written validation set. Do not select from only the one motivating example or tune against the validation answers.
5. Compare against actual VoiceInk output inside the installed application when available. Do not infer its quality from model identity or marketing examples.
6. Preserve raw ASR and fail safely on incomplete output during any later integration. A successful experiment informs a reviewed implementation proposal; it does not silently change the installed model or ordinary paste behavior.
