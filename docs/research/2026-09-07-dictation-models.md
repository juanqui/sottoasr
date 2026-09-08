# Dictation Model Decisions

- **Version:** 1.0
- **Date:** 2026-09-07
- **Status:** Implemented

## Table of Contents

1. [ASR decision](#1-asr-decision)
2. [Recording-tail defect](#2-recording-tail-defect)
3. [Cleanup decision](#3-cleanup-decision)
4. [Dictionary scope](#4-dictionary-scope)

## 1. ASR decision

Keep Parakeet TDT 0.6B v3 as the default, and update its FluidAudio runtime. This matches [VoiceInk's recommended local model](https://tryvoiceink.com/docs/recommended-models), preserves the existing multilingual and CoreML/ANE path, and avoids conflating a newer runtime with a different checkpoint.

| Option | Primary-source evidence | Decision for Sotto |
| --- | --- | --- |
| [Parakeet TDT 0.6B v3](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3) | 25 languages; published English Open ASR mean WER 6.34%; current version of this TDT family | Retain; already configured in both ASR backends |
| [Parakeet Unified English 0.6B](https://huggingface.co/nvidia/parakeet-unified-en-0.6b) | Newer April 2026 English-only RNN-T model; offline WER 5.91 in NVIDIA's comparison; supports offline/streaming | Worth a future English-specific comparison; not a v4 TDT checkpoint or a demonstrated drop-in CoreML improvement |
| [Qwen3-ASR 0.6B](https://huggingface.co/Qwen/Qwen3-ASR-0.6B-hf) | Official runtime exposes vocabulary/context prompts | Promising for recognition-time hotwords; adds a different inference/decoder path requiring local accuracy and latency tests |
| [Canary-Qwen 2.5B](https://huggingface.co/nvidia/canary-qwen-2.5b) | Published mean WER 5.63 and substantially larger model | Better published aggregate accuracy is not proof of a better menu-bar app on this Mac |

These published WERs are evidence of alternatives, not a controlled Sotto benchmark: normalization, test infrastructure, language mix, and hardware differ. None establishes that every user's dictation improves. No alternative ASR checkpoint was installed as the app default in this work.

## 2. Recording-tail defect

Sotto originally locked `fluidaudio-rs` 0.12.6 at `569c7cff`, which pins FluidAudio 0.12.6. [FluidAudio issue #747](https://github.com/FluidInference/FluidAudio/issues/747) documents a quiet final partial window producing no tokens, losing several seconds. [Fix #800](https://github.com/FluidInference/FluidAudio/pull/800) end-aligns the final window; [0.15.6](https://github.com/FluidInference/FluidAudio/releases/tag/v0.15.6) also contains long-form seam-gap repairs.

The current upstream Rust bridge still pins a runtime before this fix. A small ASR-only bridge is therefore vendored with exact SDK 0.15.6 and recorded provenance. **Correction verified 2026-09-08:** the cache root remains `~/Library/Application Support/FluidAudio/Models/`, but SDK 0.15.6 uses `parakeet-tdt-0.6b-v3/`, without `-coreml`. Its supported downloader populated that directory with the current bundles, including the required joint model. The older `parakeet-tdt-0.6b-v3-coreml/` directory remains untouched. Earlier wording incorrectly claimed both versions shared a subdirectory; [the vocabulary research](2026-09-08-vocabulary-asr.md#2-fluidaudio-apis-and-limitations) records the readiness correction.

The capture code already stopped the stream before draining queued samples. A recent 198-second recording contained about 198 seconds of samples, so there is no evidence for changing stop timing speculatively. WAV sample/finalization errors were ignored and are now handled explicitly. History duration uses captured samples and the real device rate because old multi-chunk ASR results could report zero duration.

Sixteen synthetic baseline clips of roughly 52–62 seconds, including quiet endings and varied pauses, preserved the final sentence even on the old SDK. They did **not** reproduce the user's intermittent failure. The upstream defect is a strong matching cause, not proof of that particular incident. Final same-fixture regression results and microphone verification limits are recorded in the [implementation spec](../specs/2026-09-07-dictation-reliability.md).

## 3. Cleanup decision

The requirement is minimal removal, not rewriting. The existing fine-tuned 350M model produces a full replacement transcript; successful responses were accepted without a preservation check. This makes missing sentences, changed facts, or truncated generation a product defect even if average edit-distance benchmarks look good.

[Qwen3.5-0.8B](https://huggingface.co/Qwen/Qwen3.5-0.8B) is an available instruction model in a different modern family. Its March 2 release is **not newer than the specific March 31 LFM2.5-350M base** used by this project. Searches of current official model catalogs did not establish a newer Qwen family below 1B; model update timestamps must not be mistaken for a new generation. Stock LFM2.5-350M was included as a small comparator.

Initial local experiments confirmed that strict full-transcript prompts still caused summaries, demonstration echoes, and capped output. The implementation instead enumerates narrowly eligible filler/stutter spans in Rust, asks the model only for candidate IDs, validates those IDs, and reconstructs the result from the original. It cannot insert or reorder text. This trades broad cleanup coverage for preservation and keeps the default off.

The final choice is the official, stock [LFM2.5-350M MLX 4-bit instruction model](https://huggingface.co/LiquidAI/LFM2.5-350M-MLX-4bit), replacing Sotto's custom fine-tune. On 25 synthetic held-out cases using the production candidate/reconstruction implementation, stock LFM matched 24 expected outputs, Qwen matched 20, and the original fine-tuned full-rewrite pipeline matched 8. Stock LFM made a useful edit in all nine cases requiring cleanup; one case retained one of two fillers. The corpus is small and synthetic, so this is a task-specific selection, not a universal model ranking.

The original pipeline changed names and code, altered quoted text, translated Spanish, and lost the final reference in an 18,229-character case while generating a much longer output. The new path leaves oversized input intact. Stock LFM's measured median generation time was about 80 ms versus Qwen's 267 ms; download size is about 227 MB. Full results and reproduction commands are in the [cleanup journal](../journals/2026-09-08-conservative-cleanup.md).

## 4. Dictionary scope

Explicit local substitutions solve known recurring spellings without asking a language model to guess. For example, a saved `Quen` → `Qwen` alias applies after ASR even when AI is disabled. Whole-word boundaries, longest matching phrase, non-cascading edits, and protected code/URL/email spans keep behavior predictable.

This is post-ASR replacement, not acoustic vocabulary biasing: it cannot recover a word the recognizer omitted or infer every unseen spelling. A future recognition-time vocabulary feature would require a supported decoder/context API and a recorded-audio comparison. That additional machinery is unnecessary for the first useful dictionary.
