# Vocabulary-aware local ASR on Apple Silicon

- **Version:** 1.3
- **Date:** 2026-09-08
- **Status:** In Review

## Table of Contents

1. [Decision and evidence boundary](#1-decision-and-evidence-boundary)
2. [FluidAudio APIs and limitations](#2-fluidaudio-apis-and-limitations)
3. [What other applications implement](#3-what-other-applications-implement)
4. [Local ASR alternatives](#4-local-asr-alternatives)
5. [Reproducible M4 experiment](#5-reproducible-m4-experiment)
6. [Integration contract](#6-integration-contract)
7. [Remaining experiments](#7-remaining-experiments)

## 1. Decision and evidence boundary

A canonical word list can improve recognition without requiring users to enumerate wrong spellings. FluidAudio provides acoustic rescoring for this purpose, but enabling its current default pipeline is unsafe: the local experiment below reproduces false changes to ordinary words, partial replacement of a versioned identifier, and punctuation loss. Lower aggregate word error alone is therefore an insufficient acceptance criterion.

This research informs [the vocabulary, settings, and performance specification](../specs/2026-09-08-vocabulary-settings-performance.md). Implementation began after its three sequential reviews; experimental candidates remain separate from the selected production policy. All speech fixtures are synthesized locally; no private recordings, transcript history, or live-app settings are used. Temporary model downloads stay under `/tmp/experiments/sotto-vocabulary-2026-09-08/`.

Durable [benchmark sources, fixtures, and measured output](../../benchmarks/asr/README.md) preserve the evidence without audio or model weights. The measured models are a bounded comparison, not an exhaustive ranking of every recent ASR release.

## 2. FluidAudio APIs and limitations

The latest published tag verified with `git ls-remote --tags --refs` is **0.15.6**, commit `4dbf4f9f9a5ff3a53ade848d7ba4e3df13db859b`, already pinned by Sotto. Its canonical vocabulary API is `CustomVocabularyTerm(text:ctcTokenIds:)`, collected into `CustomVocabularyContext`. No aliases are required. [Pinned SDK source](https://github.com/FluidInference/FluidAudio/tree/v0.15.6)

| Mechanism | Verified behavior |
|---|---|
| Parakeet TDT v3 decoder | The batch `AsrManager.transcribe` overloads accept audio, decoder state, and optional language. They do not accept custom vocabulary. |
| CTC acoustic rescoring | A separate CTC 110M encoder scores candidate terms against the recorded audio; a constrained rescorer compares candidates with the original transcript. This is subsequent acoustic verification, rather than TDT beam-search biasing. |
| Shared session | `VocabularyBoostingSession.rescore(text:tokenTimings:audioSamples:)` consumes one aligned transcript/audio pair; failures preserve the base transcript. Its implementation hardcodes a 0.5-second margin. |
| Candidate evidence | `VocabularyRescorer.ctcTokenEvaluateCandidates` returns untouched base text, exact UTF-8 ranges, original/candidate CTC scores, similarity, effective boost, and arbitration outcome. This permits independent acceptance and exact-span application. |
| Short-term guards | The rescorer exposes boost tapering, acoustic-rescue similarity floors, and rescue disablement. These are disabled or permissive by default; they do not eliminate the observed main-path errors. |

Sources: [batch manager](https://github.com/FluidInference/FluidAudio/blob/v0.15.6/Sources/FluidAudio/ASR/Parakeet/SlidingWindow/TDT/AsrManager.swift), [shared session](https://github.com/FluidInference/FluidAudio/blob/v0.15.6/Sources/FluidAudio/ASR/Parakeet/SlidingWindow/CustomVocabulary/VocabularyBoostingSession.swift), [candidate API](https://github.com/FluidInference/FluidAudio/blob/v0.15.6/Sources/FluidAudio/ASR/Parakeet/SlidingWindow/CustomVocabulary/Rescorer/VocabularyRescorer%2BTokenRescoring.swift).

The documentation's example `AsrManager.transcribe(..., customVocabulary:)` does not match the shipped API. Its published iPhone memory and accuracy figures are not measurements of Sotto on this M4. Upstream issues independently describe short-word false insertions; their fixes add opt-in controls rather than establishing that every word list is safe. [Vocabulary documentation](https://github.com/FluidInference/FluidAudio/blob/v0.15.6/Documentation/ASR/CustomVocabulary.md), [issue 702](https://github.com/FluidInference/FluidAudio/issues/702), [issue 724](https://github.com/FluidInference/FluidAudio/issues/724).

The SDK's `AsrModels.load`, `loadFromCache`, and `downloadAndLoad` share `ModelHub.loadModels`, whose non-transient CoreML error recovery purges the repository before retrying. Production now uses a small pinned-v3 direct loader with the public `AsrModels` initializer, unchanged CPU-only preprocessor and CPU/ANE encoder/decoder/joint configuration, and local vocabulary parsing. Missing artifacts still use the supported downloader in the verified SDK folder. Cached-load failures preserve artifacts; no global offline flag is toggled while vocabulary preparation may be downloading. Three isolated cache fault cases passed, and one real synthetic transcription through the new loader exactly preserved its final sentence. [Recovery implementation](https://github.com/FluidInference/FluidAudio/blob/v0.15.6/Sources/FluidAudio/Shared/Download/ModelHub.swift), [reproducible cache probe](../../scripts/check-asr-cache.py).

The auxiliary artifact is `FluidInference/parakeet-ctc-110m-coreml`. The experiment downloaded **102,803,869 bytes** across the model bundles and tokenizer/configuration assets. The supported production cache is `~/Library/Application Support/FluidAudio/Models/parakeet-ctc-110m-coreml/`; the TDT v3 cache is separate. SDK 0.15.6 uses `~/Library/Application Support/FluidAudio/Models/parakeet-tdt-0.6b-v3/` (without `-coreml`), whereas the older runtime used the remote repository slug as its folder. The older folder remains present and untouched. Runtime initialization had already populated the SDK folder; Rust readiness now checks its four INT8 model bundles and `parakeet_vocab.json`, and reports the actual directory. This corrects the earlier research claim that both SDK versions shared one subdirectory. `CtcModels.modelsExist` checks model bundles and `vocab.json` but omits `tokenizer.json`, which `BpeTokenizer.load` requires. Readiness must include tokenizer availability and successful parsing. [CTC model loader](https://github.com/FluidInference/FluidAudio/blob/v0.15.6/Sources/FluidAudio/ASR/Parakeet/SlidingWindow/CustomVocabulary/WordSpotting/CtcModels.swift)

Mixed numeric identifiers require special handling. The CTC tokenizer emitted unknown token ID 0 for parts of `Qwen3.8-Flash-Next`. Treating that sequence as ordinary acoustic evidence produced a partial match and duplicate prefix. Storing the identifier is valid; blindly scoring its written punctuation/digits is not a sound recognition strategy.

## 3. What other applications implement

| Application/path | Verified vocabulary mechanism | Implication |
|---|---|---|
| VoiceInk, Parakeet | Current source transcribes with language hints only; it never configures a CTC vocabulary session. | Its vocabulary UI does not establish native Parakeet conditioning. |
| VoiceInk, AI enhancement | Inserts saved terms into `CUSTOM_VOCABULARY` in the text-model prompt. | Correction follows ASR and depends on enhancement. |
| Superwhisper | Official documentation separates recognition hints supplied with audio from deterministic replacements. It warns that excessive vocabulary may affect formatting/language recognition. | Useful product separation; the documentation does not expose an implementation for every local backend. |
| whisper.cpp | `initial_prompt`/prompt tokens condition the actual Whisper decoder; the header documents a typical 224-token limit. Also exposes a logits filter callback. | Native recognition prompting is available when using Whisper; it is not guaranteed exact vocabulary enforcement. |
| MacWhisper app prompts | Official examples include translation, code generation, and professional rewriting. | These are processing prompts; this page alone does not prove decoder vocabulary biasing. |
| TypeWhisper | Current Parakeet plugin implements optional CTC model/tokenizer/rescorer, disabled by default. Its Qwen3 plugin contains a context-prompt path but advertises dictionary-term support as unsupported. | CTC is a concrete optional integration; the presence of Qwen context code alone does not establish an enabled or validated dictionary feature. |

VoiceInk was inspected at commit `8f089cb4bf2c9c2f217b0cc0af909d9052ff6288` (2026-09-03); its lockfile follows FluidAudio main commit `c7b13a3942e79893f3bd76bfe3b1ed8d03e0bfc7`. Its current docs still recommend Parakeet V3 first. [VoiceInk ASR source](https://github.com/Beingpax/VoiceInk/blob/8f089cb4bf2c9c2f217b0cc0af909d9052ff6288/VoiceInk/Infrastructure/Providers/Transcription/FluidAudio/FluidAudioTranscriptionService.swift), [recommended models](https://tryvoiceink.com/docs/recommended-models), [Superwhisper vocabulary](https://superwhisper.com/docs/get-started/interface-vocabulary), [whisper.cpp header](https://github.com/ggml-org/whisper.cpp/blob/master/include/whisper.h), [MacWhisper prompts](https://docs.macwhisper.com/article/31-app-specific-dictation-prompts), [TypeWhisper Parakeet plugin](https://github.com/TypeWhisper/typewhisper-mac/blob/main/TypeWhisperPluginSDK/Plugins/ParakeetPlugin/ParakeetPlugin.swift).

Superwhisper's July 6, 2026 changelog specifically associates improved vocabulary and forced alignment with offline Whisper models. This supports a narrower, model-specific comparison; it does not establish the same mechanism in its Parakeet or cloud paths. [Official changelog](https://ai.superwhisper.com/changelog)

## 4. Local ASR alternatives

| Candidate | Mac path | Reason to test | Boundary |
|---|---|---|---|
| Parakeet TDT 0.6B v3 | Existing FluidAudio CoreML/ANE, INT8 encoder | Existing multilingual baseline; low warm latency | No direct batch vocabulary argument; extra CTC pass adds work |
| Parakeet Unified EN 0.6B | FluidAudio `UnifiedAsrManager`, CoreML, INT8 available | Newer English checkpoint; upstream reports better English accuracy and throughput in its harness | English only; requires separate artifact and real local comparison |
| Qwen3-ASR 0.6B | MLX Audio on Metal; CoreML conversion also exists | Native system-prompt/hotword conditioning; small multilingual ASR | Additional runtime/model; GPU memory and latency must be measured |
| Whisper large-v3-turbo or smaller | WhisperKit/CoreML or whisper.cpp/Metal + optional CoreML encoder | Established decoder prompt mechanism | Substantial backend work; size/quality tradeoff and vocabulary token limit |
| Granite Speech 5.0 470M TurboCTC | Official mlx-audio support; non-autoregressive CTC | New compact English model, released August 25, 2026 | Fastest measured short-clip median here, but more word errors; no tested vocabulary conditioning |
| Apple DictationTranscriber | Speech framework, on-device dictation models | Official recognition hints and custom language-model support | Requires macOS 26; this host runs 15.6 and Sotto supports 14+ |

NVIDIA released Unified EN on April 7, 2026. The converted model card reports an English accuracy/throughput advantage in one benchmark, which is a reason to test rather than a Sotto performance result. The original and conversion cards currently advertise different licenses (NVIDIA Open Model License versus CC-BY-4.0), so distribution attribution needs reconciliation before choosing it. [NVIDIA Unified card](https://huggingface.co/nvidia/parakeet-unified-en-0.6b), [CoreML conversion](https://huggingface.co/FluidInference/parakeet-unified-en-0.6b-coreml).

Qwen's official family includes 0.6B and 1.7B models under Apache 2.0. MLX Audio's current implementation passes `system_prompt` into the ASR input before audio decoding and merges `hotwords` into it. That is model conditioning rather than text-only cleanup. The official CUDA throughput claims do not predict single-utterance Mac latency. [Qwen model card](https://huggingface.co/Qwen/Qwen3-ASR-0.6B), [MLX implementation](https://github.com/Blaizzy/mlx-audio/blob/main/mlx_audio/stt/models/qwen3_asr/qwen3_asr.py).

IBM's latest small English candidate is Granite Speech 5.0 470M TurboCTC, under Apache 2.0. Its official card documents greedy CTC decoding and mlx-audio 0.5.1 or later. The local measurement below uses its original mixed BF16/F32 weights through the native MLX implementation, without remote Python code. [IBM model card](https://huggingface.co/ibm-granite/granite-speech-5.0-470m-turboctc)

Apple's `AnalysisContext.contextualStrings` documentation describes recognition hints for `DictationTranscriber`, with up to 100 short phrases, and points to custom language models for unusual pronunciations. `DictationTranscriber` uses on-device system-dictation models and excludes network-only locales. These APIs require macOS 26; the documentation does not justify assuming the distinct `SpeechTranscriber` model accepts the same vocabulary hints. [Apple context API](https://developer.apple.com/documentation/speech/analysiscontext/contextualstrings), [DictationTranscriber](https://developer.apple.com/documentation/speech/dictationtranscriber)

## 5. Reproducible M4 experiment

Host: **Apple M4, 10 CPU cores (4 performance/6 efficiency), 32 GiB unified memory**. Experiment directory: `/tmp/experiments/sotto-vocabulary-2026-09-08/`. `prepare.py` uses local macOS `say`, Samantha and Daniel, 175 words/minute, mono PCM16 at 16 kHz. There are 40 clips totaling 126.237 seconds, including 18 negative clips with ordinary near-homophones. The glossary has 14 canonical terms and no aliases.

`build.py` links a small Swift probe against the exact release archive already built for Sotto. `probe.swift` loads the existing TDT v3 model, records raw results, downloads CTC into the experiment directory, and reuses each CTC probability matrix across four rescoring policies. The run is captured in `results-first.log`; `analyze.py` produces `summary-first.json`. Other agents paused hardware measurements for this run.

| Recognition path | Word errors / 392 reference words | Exact clips / 40 | Negative clips changed incorrectly / 18 |
|---|---:|---:|---:|
| Raw TDT v3 | 35 (8.93%) | 20 | 0 |
| SDK session defaults | 21 (5.36%) | 22 | 8 |
| Default with 0.1 s margin | 21 (5.36%) | 22 | 8 |
| Short-term guards + 0.1 s | 21 (5.36%) | 22 | 8 |
| Acoustic rescue disabled | 21 (5.36%) | 22 | 8 |

The diagnostic word error metric ignores case/punctuation and splits letter/number runs; references include intended canonical glossary spellings and the model's written number format. This is a deliberately challenging synthetic development set, not a representative production WER estimate or independent holdout. Four policies returned the same text on every clip.

Observed failures include `queen → Qwen`, `cloud → Claude`, `ran → CRAN`, and `Quin 3.8 Flash Next → Quin Qwen3.8-Flash-Next`. Replacing `Fluid Audio.` removed the final period, and replacing repeated `Quen,` removed commas. Both voices preserved ordinary `Quinn` in the negative personal-name sentence, while Samantha's intended `Qwen` also remained `Quinn`: spelling choice cannot be inferred from a homophone alone.

| Measurement | Result |
|---|---:|
| Warm TDT inference median / p95 | 63.42 / 73.57 ms |
| Additional CTC inference median / p95 | 98.54 / 104.64 ms |
| Rescoring median / p95, per policy | 6.69 / 9.76 ms |
| CPU time, all 40 TDT passes | 2.240 s |
| CPU time, all 40 CTC passes | 3.525 s |
| TDT initialization | 14.47 s |
| First CTC preparation, including download | 24.90 s |
| Whole-process maximum RSS | 525,844,480 bytes (501.48 MiB) |

The RSS peak occurred during TDT initialization and did not rise after CTC loading; this does **not** establish zero incremental CTC memory. `/usr/bin/time -l` also reported a different macOS peak-footprint metric (72,238,040 bytes); neither should be conflated with model weight size or all ANE service allocations. Single-pass latency excludes microphone stop, WAV serialization, cleanup, and paste; it is not end-to-end dictation latency.

`powermetrics` is installed and supports CPU, GPU, ANE, and per-process energy-impact sampling. An unprivileged one-sample attempt returned `powermetrics must be invoked as the superuser`. No privilege change was attempted. Consequently, this experiment reports CPU time, latency, and memory—not measured joules, watts, or battery savings. CPU time is an optimization proxy, not an energy measurement.

### Native Qwen3-ASR conditioning

The isolated MLX experiment used `mlx-community/Qwen3-ASR-0.6B-8bit` revision `89e96d92ba34aca20b3e29fb10cc284097d1219f`, MLX 0.32.2, and mlx-audio 0.5.3. Each of the same 40 clips was decoded three times with rotating policy order, temperature zero, and a 256-token ceiling; no clip hit the ceiling. The model's native `hotwords` argument conditions audio decoding. A second prompted policy additionally requested exact transcription and preservation of ordinary words and personal names.

| Qwen policy | Word errors / 392 | Exact clips / 40 | Negative clips changed / 18 | Median inference |
|---|---:|---:|---:|---:|
| No context | 50 (12.76%) | 16 | 0 | 212.64 ms |
| Canonical hotwords | 31 (7.91%) | 25 | 1 | 228.32 ms |
| Preservation instruction + hotwords | 30 (7.65%) | 24 | 1 | 242.44 ms |

Hotwords corrected all six simple Qwen pronunciation cases and preserved queen/cloud/ran. However, both prompted policies changed the personal-name sentence `Quinn helped me…` into `Qwen, help me…`. Hotwords also changed the first personal name in a mixed Quinn/queen/Qwen sentence; the explicit instruction preserved that name but missed the intended model name. Native conditioning is promising, but it failed the no-new-distractor-errors gate and is not selected as the product default.

The Qwen process peaked at 853,835,776 bytes RSS and 1,631,945,436 bytes Metal allocation; these overlap and must not be added. macOS reported a peak footprint of 1,883,703,192 bytes. Its short-clip hotword latency was roughly 3.6 times raw TDT's measured median and used the GPU, whereas the shipped Parakeet encoder uses CoreML/ANE. These measurements do not establish battery consumption or a universal accuracy ranking.

### Frozen candidate policy and independent validation

After inspecting development evidence, the policy was frozen before the primary agent revealed an independent 24-sentence holdout. Its source SHA-256 at freeze was `13c2670bfb7543bcd0477f200820bdd8629cd0804e233f467b39a751c92c9ef0`; the original source is archived at `/tmp/experiments/sotto-vocabulary-2026-09-08/frozen-vocabulary-policy.rs`. No constants or acceptance rules were tuned against the independent outcomes.

The policy requires a strictly better raw CTC acoustic score than the original, without the SDK's additive boost. Terms of at most five letters also require equal source/canonical letter counts and similarity at least 0.75; longer terms require at least 0.8. Only representable alphabetic/space vocabulary candidates qualify. Replacements use the original UTF-8 range, retain punctuation outside it, deduplicate identical proposals, and decline overlapping conflicts. Numeric identifiers and unsupported tokenizer characters remain stored but cannot produce partial acoustic substitutions.

The independent source SHA-256 is `79b2062c486d086c15ecaea55b7ec6d1773e42992da21ea3c343feb7987d8687`. Two voices produced 48 clips, with 24 ordinary/distractor clips and 24 target/mixed clips. The guard improved four target clips and changed none of the ordinary clips or other target clips: MiniCPM spacing in both voices, one Quen→Qwen correction, and one Soto ASR→SottoASR correction. Word errors fell from 27 to 20 across 436 reference words, and exact clips increased from 30 to 34. Both paths use the same written-number normalization when scoring.

The acoustic pass added a median 94.83 ms (p95 99.64 ms), compared with raw TDT's 62.55 ms (p95 67.24 ms); whole-set CPU time was 4.032 s for CTC and 2.644 s for TDT. This is an independent synthetic check of a conservative spelling aid, not evidence that acoustics resolve true homophones. Intended Quinn/Qwen cases often remain unchanged. [Committed synthetic evidence](../../benchmarks/asr/vocabulary-holdout-evidence.json) allows the actual Rust policy to be regression-tested against all 48 recorded candidate sets without needing private audio or model inference.

### Unified EN comparison

`UnifiedAsrManager(encoderPrecision: .int8)` completed the same 40 development clips using a separate model in the experiment directory. It made 34 errors across 392 reference words, versus TDT v3's 35, while exact clips decreased from 20 to 19. It corrected PostgreSQL in two clips but dropped an initial “The” from an ordinary sentence and changed MLX to NLX in another. Qwen spellings remained unresolved.

Unified EN's median latency was 44.81 ms (p95 56.87), versus TDT's 63.42 ms (p95 73.57), a 29% reduction on these short fixtures. Aggregate CPU time fell from 2.240 to 0.820 seconds, while maximum RSS rose from 525.8 to 697.6 MB. Initial download/load took 44.09 seconds. These results identify a potentially economical English option, but do not demonstrate a recognition upgrade sufficient to replace multilingual TDT v3. [Per-clip and aggregate measurements](../../benchmarks/asr/measurements-2026-09-08.json)

### Granite Speech 5.0 TurboCTC comparison

The official 470M artifact at revision `18ca3c1de6cd092b5a30c39fb0f04550b38ed1a0` completed all 40 clips through mlx-audio 0.5.3. Its 946,180,704-byte safetensors artifact contains BF16 model weights and a small number of F32/integer tensors. One warmup preceded the measured clips; inference was offline, with no other builds, model inference, or model downloads active.

Granite made **38/392 word errors (9.69%)**, versus TDT v3's 35, with 20 normalized exact clips for both. The metric ignores case and punctuation consistently: Granite's lowercase, unpunctuated output was not penalized for formatting. It introduced Kubernetes and SottoASR errors and still missed Qwen spellings. The tested native greedy CTC path offers no vocabulary-conditioning argument.

Median inference was **38.96 ms**, p95 41.85 ms, with 0.534 seconds aggregate CPU time across 40 clips. Model loading took 1.19 seconds and the whole process 4.09 seconds. Peak RSS was 1,073,168,384 bytes, Metal allocation 1,092,013,642 bytes, and macOS peak footprint 1,278,707,128 bytes; these overlapping counters must not be added. This is the fastest measured raw short-clip path in this bounded comparison, but it does not establish an accuracy or vocabulary improvement over TDT, and its higher GPU memory use is not a battery measurement. Keep the current multilingual default. [Durable outputs and counters](../../benchmarks/asr/measurements-2026-09-08.json)

### CoreML compute-unit comparison

A fresh offline process for each supported configuration loaded isolated clones of the same current SDK INT8 cache. After one warmup clip, each process transcribed the identical 40 fixtures. All 160 results were byte-identical across modes: 35 word errors and 20 exact clips per mode.

| Allowed compute units | Median / p95 inference | CPU time, 40 clips | Maximum RSS | Model load |
|---|---:|---:|---:|---:|
| CPU + ANE (current default) | 62.70 / 69.72 ms | 2.271 s | 501.58 MiB | 14.78 s |
| All | 66.12 / 72.61 ms | 2.281 s | 2013.97 MiB | 15.23 s |
| CPU + GPU encoder; other models CPU + ANE | 137.63 / 143.81 ms | 2.764 s | 2374.13 MiB | 7.01 s |
| CPU only | 138.07 / 143.42 ms | 7.335 s | 1679.70 MiB | 3.41 s |

The SDK always places the preprocessor on CPU. These configurations specify permitted devices rather than proving each operation's actual accelerator placement. The current CPU/ANE configuration had the lowest warm latency, aggregate CPU time, and process RSS in this comparison; keeping it is supported by local measurements. Model-load timing includes filesystem/CoreML effects and is not a randomized cold-start ranking. Energy was not measured.

The first attempted isolated clone used the legacy folder, so all four attempts failed offline before inference. Those bootstrap failures are excluded; the successful comparison above cloned the verified SDK folder. No live cache was moved, deleted, or used for recovery experiments. [Supported SDK configuration](https://github.com/FluidInference/FluidAudio/blob/v0.15.6/Sources/FluidAudio/ASR/Parakeet/SlidingWindow/TDT/AsrModels.swift), [durable measurements](../../benchmarks/asr/measurements-2026-09-08.json).

## 6. Integration contract

Store canonical terms separately as `Settings.vocabulary: Vec<String>` (empty default, up to 100 trimmed unique terms, 120 Unicode characters per term). Preserve the existing deterministic replacement dictionary independently. Storage accepts version syntax; recognition eligibility is determined separately so unsupported characters cannot create invalid acoustic comparisons.

Prepare a detached auxiliary model on a blocking worker, outside the resident ASR lock. Transfer the completed retained model into the engine in a short serialized step; a cleared list drops it instead. This prevents a slow public download or CoreML compilation from blocking ordinary dictation. Concurrent preparation requests share one operation, and a later nonempty save restarts preparation if a preceding clear declined attachment.

Explicit preparation repairs missing or invalid `tokenizer.json` independently, preserving all model bundles. It parses a replacement in a private staging directory before atomically replacing only that metadata file. Cached-only startup never downloads. The targeted metadata request uses a 30-second idle timeout and a 120-second resource timeout; model transfers use a 120-second idle timeout and a 256 KiB/30-second stall watchdog. The SDK retains four retry attempts and its own defaults for repository listing. Neither CoreML compilation nor the bridge semaphore has an application-wide deadline, so setup can remain pending for a long time while ordinary ASR continues. Do not release or reuse a native handle merely because a caller-side timer expires.

The offline `scripts/check-vocabulary-cache.py` probe compiles the real helper with the pinned SDK and tests missing and malformed metadata, cached-only behavior, a valid cache, download failure, invalid replacement, and atomic-write failure. Its seven cases preserve a model sentinel and leave no staging directory behind. Production uses `CtcModels.loadDirect` after download, avoiding the SDK's generic load-and-purge recovery path. Failed model loading remains visible without deleting existing weights.

Expose `VocabularyStatus { supported, downloaded, loaded, preparing, download_size_mb, error }`, `get_vocabulary_status`, and idempotent `prepare_vocabulary_model`. First saving a nonempty list can initiate the disclosed approximately 103 MB preparation. Preparation errors retain both the list and normal ASR, with a retry action. Empty vocabulary means no CTC inference; clearing it releases auxiliary models. Never download from the recording-completion path.

Each recording uses a stable vocabulary snapshot. `AsrResult.unboosted_text: Option<String>` records the untouched decoder text only when vocabulary changes the output. Every history path must retain that original before subsequent AI cleanup or explicit replacements. Unavailable auxiliary models fall back to the base transcript. A vocabulary-applied indicator requires an actual accepted change.

Do not apply the SDK's rewritten transcript directly. Evaluate candidates against unchanged ASR text, validate exact UTF-8 span boundaries and original contents, preserve surrounding punctuation/whitespace, reject overlapping or partial identifier edits, and apply accepted edits right-to-left. The frozen guard passed the independent synthetic gate above with limited target coverage. Keep that limitation explicit: the list improves some spelling decisions, while ambiguous homophones may still need explicit replacements.

Move synchronous ASR initialization and transcription out of Tokio worker threads using one shared `spawn_blocking` helper around exclusively owned engine access. A minimal `Arc<TokioMutex<Box<dyn AsrEngine>>>` permits `blocking_lock` inside the blocking pool; all production and test pipeline call sites should share this path. Await completion before releasing request-owned audio or decoder state. Avoid timeout code that merely abandons an in-flight blocking operation.

## 7. Remaining experiments

- Completed candidate evidence export and frozen-policy validation on 48 independent synthetic clips.
- Completed Qwen3-ASR 0.6B native-conditioning comparison; do not promote the tested policies because they introduced personal-name errors.
- Completed Unified EN comparison; retain multilingual TDT v3 because the faster English model did not establish an accuracy improvement.
- Completed the latest small Granite Speech 5.0 TurboCTC comparison; faster short-clip inference but more word errors and no demonstrated vocabulary advantage.
- Completed four supported CoreML compute-unit configurations on identical cached weights; CPU/ANE remains the measured choice for this M4.
- Measure an independent holdout with ordinary speech, distractor-heavy vocabulary, repeated terms, silence, and long recordings with terms crossing chunk boundaries.
- Measure idle and warm memory separately; repeat only justified finalist timing windows with other measurements paused. Energy remains unavailable without an authorized privileged measurement facility.
