# ASR Tail Regression and Runtime Upgrade

- **Version:** 1.0
- **Date:** 2026-09-08
- **Status:** Implemented

## Table of Contents

1. [Question and evidence](#1-question-and-evidence)
2. [Experiment](#2-experiment)
3. [Results](#3-results)
4. [Reproduction and limits](#4-reproduction-and-limits)

## 1. Question and evidence

Can we eliminate a known final-window failure without changing Parakeet model families or introducing custom chunk-merging code? Sotto used `fluidaudio-rs` 0.12.6, pinned to Swift FluidAudio 0.12.6. Both compiled backends select Parakeet TDT 0.6B v3, the same local model [VoiceInk recommends](https://tryvoiceink.com/docs/recommended-models).

Upstream [issue #747](https://github.com/FluidInference/FluidAudio/issues/747) describes a quiet multi-window recording losing its final several seconds: the short final chunk, padded mostly with zeros, emits only blank tokens. [Fix #800](https://github.com/FluidInference/FluidAudio/pull/800) ends the final window at speech-bearing audio and fills its left context with real samples. [FluidAudio 0.15.6](https://github.com/FluidInference/FluidAudio/releases/tag/v0.15.6) includes this fix and [seam-gap repair #761](https://github.com/FluidInference/FluidAudio/pull/761). This is a matching, evidence-backed bug class; no failing private recording was available to prove the user's exact incident has this cause.

Local capture logs showed 3,168,256 samples at 16 kHz for a 198-second recording: 198.016 seconds of captured audio. The previous diagnostic divided by an assumed 48 kHz and incorrectly displayed 66 seconds. C FFI uses allocated complete strings, so no fixed output buffer truncation was found. The old SDK also returns zero duration for multi-window results; history now uses captured samples and the real rate.

## 2. Experiment

A local `say -v Samantha` fixture reads a 111-word body followed by a 17-word final instruction ending in **silver telescope**. Sixteen variations span 52.326–62.326 seconds: normal volume, whole-recording quiet, quiet tail, no appended silence, and 0–10-second pauses before the tail at two low gains. Maximum quiet peaks range from about 0.5% to 2.5% of full scale.

The baseline harness links the already-built 0.12.6 Rust bridge. The updated harness links the actual vendored bridge and Swift SDK 0.15.6, commit `4dbf4f9f9a5ff3a53ade848d7ba4e3df13db859b`. Both use the same local v3 cache. The supported downloader added the new required joint artifact in the existing directory; no cache was wiped or relocated. Audio and results remained local under `/tmp/experiments/sotto-asr-tail-2026-09-07/`.

## 3. Results

| Check | Baseline 0.12.6 | Updated 0.15.6 |
| --- | --- | --- |
| Final 17-word instruction | Preserved in 16/16 | Preserved in 16/16 |
| Transcript word count | 128 in every case | 128 in every case |
| Normalized word sequence | Reference baseline | Identical to baseline in 16/16 |
| File duration returned | Incorrectly zero in 16/16 | Matches WAV duration in 16/16 |
| Median reported inference time | 0.473 seconds | 0.367 seconds |

These times are a single warm local pass, excluding model load and most orchestration overhead; they are not a controlled performance benchmark. Some updated outputs differ in punctuation, including a doubled period at a seam. No wording changed relative to baseline in this corpus. The fixture did not reproduce the intermittent real-audio failure on the old runtime, so its successful results are regression evidence, not proof of a repaired microphone incident.

Checked WAV serialization also has unit tests for exact sample order and final sentinels at 16/44.1/48 kHz, including a full minute of input, plus injected sample-write, silence-write, and header-finalization failures. A pipeline test sends the final five seconds during `stop()`, verifies those samples reach the file, and confirms captured history duration even when the mock ASR returns zero.

## 4. Reproduction and limits

Run the maintained fixture entry point from the repository root:

```bash
set -o pipefail
python3 scripts/check-asr-tail.py 2>&1 | tee /tmp/sotto-asr-tail-regression.txt
```

The script generates only synthetic audio in a fresh temporary directory, invokes `cargo run --example asr_fixture` from `src-tauri/` so the Swift linker configuration applies, and checks all tails, repeated body anchors, and durations. Its temporary files are removed on exit. Missing SDK model artifacts may be downloaded through the same cache path used by the app.

Required native build evidence: `/tmp/sotto-verify-cargo-build-native-fix.txt`. Baseline and updated per-fixture output: `/tmp/experiments/sotto-asr-tail-2026-09-07/{baseline-results,baseline-sweep,updated-results}.log`. Automated preservation of supplied PCM does not guarantee every spoken word is recognized. Actual microphone/hotkey testing with the user's low-volume/pause pattern remains a separate manual check; no raw app binary was launched.
