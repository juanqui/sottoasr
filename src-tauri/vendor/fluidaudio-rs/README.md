# Sotto FluidAudio bridge

- **Version:** 1.1
- **Date:** 2026-09-08
- **Status:** Implemented

This is the small, ASR-only interface Sotto needs, based on the synchronous C ABI pattern from [FluidInference/fluidaudio-rs](https://github.com/FluidInference/fluidaudio-rs), upstream commit `569c7cff7eac1b65e10f83a596afb50a4cdf4031` (Cargo declares MIT). It replaces the old general-purpose wrapper so Sotto can use exact **FluidAudio 0.15.6**, including upstream [final-window fix #800](https://github.com/FluidInference/FluidAudio/pull/800) and [seam repair #761](https://github.com/FluidInference/FluidAudio/pull/761).

The Swift SDK remains an unmodified Apache-2.0 dependency fetched by SwiftPM. `Package.resolved` pins its commit; its NemoTextProcessing binary dependency is checksum-pinned upstream. No model weights are vendored.

Local changes: only batch ASR is exposed; ownership permits `Send` but not `Sync`; calls require a mutable Rust reference; errors retain the underlying Swift reason; each recording has a fresh decoder state; WAV metadata supplies duration. The v3 checkpoint and FluidAudio cache root remain in use. SDK 0.15.6 caches its four INT8 bundles (including `JointDecisionv3.mlmodelc`) and `parakeet_vocab.json` in `Models/parakeet-tdt-0.6b-v3/`, without the remote slug’s `-coreml` suffix. The older `Models/parakeet-tdt-0.6b-v3-coreml/` directory is preserved; the SDK populated its current directory through its supported downloader. Rust readiness checks the current SDK directory. Earlier notes incorrectly stated that both versions used the same subdirectory.

`AsrCache.swift` loads cached TDT bundles directly through CoreML and the public `AsrModels` initializer. The pinned SDK's ordinary load methods can delete the whole repository after a CoreML failure; Sotto does not enter that recovery path. Missing artifacts use the supported downloader at the existing SDK directory. An invalid cached bundle or vocabulary surfaces an error and stays intact. `scripts/check-asr-cache.py` tests incomplete-cache, actual CoreML-load, and malformed-vocabulary failures in disposable fixtures; its optional synthetic smoke clones the supplied cache before inference. The failed-model tests keep online permission enabled, proving preservation does not depend on the SDK's global offline flag.

Optional canonical vocabulary uses the SDK's separate CTC 110M acoustic model. Preparation returns an independently owned model handle so public downloads and CoreML setup do not hold the resident ASR lock. The bridge exports candidate scores and exact ranges alongside untouched TDT text; Sotto's Rust policy decides which substitutions are safe. The SDK's rewritten transcript is never applied directly. Empty vocabulary adds no CTC inference, and inference never downloads models.

`VocabularyCache.swift` repairs only missing or invalid tokenizer metadata after explicit preparation. A replacement must pass the real SDK parser before an atomic write. Model bundles and failed replacement inputs remain intact. The offline `scripts/check-vocabulary-cache.py` probe exercises seven cache and failure cases using temporary files. Setup has per-transfer network limits, but no overall CoreML deadline.

To update: change the exact Swift SDK version, resolve/build, inspect upstream ASR/API and artifact changes, then rerun Sotto's WAV tests and the real multi-window speech regression corpus. Do not silently switch model version or cache paths. The app intentionally does not use streaming, VAD, TTS, or diarization APIs.
