# Sotto FluidAudio bridge

- **Version:** 1.3
- **Date:** 2026-11-19
- **Status:** Implemented

This is the small, ASR-only interface Sotto needs, based on the synchronous C ABI pattern from [FluidInference/fluidaudio-rs](https://github.com/FluidInference/fluidaudio-rs), upstream commit `569c7cff7eac1b65e10f83a596afb50a4cdf4031` (Cargo declares MIT). It replaces the old general-purpose wrapper so Sotto can use exact **FluidAudio 0.15.6**, including upstream [final-window fix #800](https://github.com/FluidInference/FluidAudio/pull/800) and [seam repair #761](https://github.com/FluidInference/FluidAudio/pull/761).

The Swift SDK is an Apache-2.0 dependency fetched by SwiftPM. `Package.resolved` pins its commit. Four checksum-guarded patches change it: two replace recursive vocabulary alignment and optimize CTC tensor access (the latter also initializes final-window padding to silence), and two pin model downloads to fixed commits (below). `build.rs` applies each patch only to the exact upstream file it was written for and checks the result's checksum, so the build stops if either side changes. NemoTextProcessing remains checksum-pinned upstream. No model weights are vendored.

## Model revisions

The SDK builds model URLs with `main` in two places: `ModelRegistry.resolveModel` (`resolve/main`) and `HFTreeLister.listTree` (`tree/main`). `patches/model-registry-revision.patch` adds a revision map keyed by repository, and `patches/tree-lister-revision.patch` makes the tree listing use it. Other repositories still read `main`; the app does not use them.

| Model | Repository | Commit |
| --- | --- | --- |
| Speech (TDT v3, INT8) | `FluidInference/parakeet-tdt-0.6b-v3-coreml` | `7dd20fe6b1797d35f5e3307e8b1732d9a178edfe` |
| Vocabulary (CTC 110M) | `FluidInference/parakeet-ctc-110m-coreml` | `accdafd8cf8a2ff1cabe3c11e54416b405d409aa` |

Both are the commits whose files match the caches the app was qualified with (checked 2026-11-19 against the tree API's Git LFS SHA-256 and Git blob IDs).

`model-manifest.json` lists the SHA-256 and size of exactly the files the bridge loads: every file in `Preprocessor`, `Encoder`, `Decoder` and `JointDecisionv3.mlmodelc` plus `parakeet_vocab.json` (21 files, 483 MB), and every file in `MelSpectrogram` and `AudioEncoder.mlmodelc` plus `vocab.json` and `tokenizer.json` (12 files, 103 MB). The app (`src-tauri/src/asr/integrity.rs`) checks both models the same way before CoreML loads anything:

- A file is fresh when the current preparation downloaded or replaced it. The files present at the start are saved before any download, in a folder created for that on a first install; if that save fails, nothing downloads. A preparation that stops early (quit, lost network, read error) is therefore judged against its real start next time.
- A fresh file that fails is an error and nothing loads. It was never in use, so it stays on disk only until the next preparation that may download (the next launch, "Try again", preparing the vocabulary model, or a re-download), which deletes it with any `.partial` leftovers and fetches it again. A pin change in a later release keeps these rejections and the saved start.
- A failing file that was already present loads and sets `ModelStatus.integrity_warning` (one message for both models). Settings then offers a re-download, the only path that replaces such a file. It moves the failing files of both models to `.sotto-replaced-<id>/`, downloads them again, verifies, and deletes the moved copies only after that succeeds; on failure it moves them back and removes the failed download's `.partial` files. The vocabulary model is included only if it was prepared. Dictation keeps using the loaded model while files download; only the reload pauses it. While vocabulary files are moved, deleted or downloaded, dictation skips the vocabulary boost, because the SDK reads `tokenizer.json` from disk for every boosted dictation and offers no way to pass the loaded tokenizer.
- If the app quits during a re-download, the next preparation moves back every file whose place is empty, with its earlier verdict, and deletes the `.sotto-replaced-*` folder once the model verifies.
- `.sotto-integrity.json` in each model folder keeps every file's verdict by size, modification time and inode, so a file is hashed again only after it changes. This includes a warned file: the warning stays without rehashing 483 MB at each launch.

When a future release pins a different commit, the record of the old commit is ignored and every file is hashed against the new manifest. Files from the old commit are present and mismatched: they load with a warning and are replaced only by the confirmed re-download. Files the new manifest adds are missing and download as fresh files.

To regenerate the manifest, for example after choosing a new commit:

```sh
python3 scripts/check-model-manifest.py --revision speech=<commit> --revision vocabulary=<commit> --write
```

Small files are downloaded at the commit, or read from an existing cache with `--cache "$HOME/Library/Application Support/FluidAudio/Models"`; either way their Git blob ID must match the commit's tree. Without `--write` the script reports whether the committed manifest still matches. Update the commits in `patches/model-registry-revision.patch` to match, then its patched checksum in `build.rs`. A unit test fails when the manifest and the patch disagree.

Local changes: only batch ASR is exposed; ownership permits `Send` but not `Sync`; calls require a mutable Rust reference; errors retain the underlying Swift reason; each recording has a fresh decoder state; WAV metadata supplies duration. The v3 checkpoint and FluidAudio cache root remain in use. SDK 0.15.6 caches its four INT8 bundles (including `JointDecisionv3.mlmodelc`) and `parakeet_vocab.json` in `Models/parakeet-tdt-0.6b-v3/`, without the remote slug’s `-coreml` suffix. The older `Models/parakeet-tdt-0.6b-v3-coreml/` directory is preserved; the SDK populated its current directory through its supported downloader. Rust readiness checks the current SDK directory. Earlier notes incorrectly stated that both versions used the same subdirectory.

`AsrCache.swift` downloads missing TDT artifacts with the SDK's supported downloader, as a separate step that never loads. Loading reads the verified bundles directly through CoreML and the public `AsrModels` initializer, and never downloads. The pinned SDK's ordinary load methods can delete the whole repository after a CoreML failure; Sotto does not enter that recovery path. Download and verification run without the resident engine lock; the lock covers only loading and compilation. An invalid cached bundle or vocabulary surfaces an error and stays intact. `scripts/check-asr-cache.py` tests incomplete-cache, offline-download, wrong-folder, actual CoreML-load, and malformed-vocabulary failures in disposable fixtures, and that both model repositories resolve to their pinned commits; its optional synthetic smoke clones the supplied cache before inference. The failed-model tests keep online permission enabled, proving preservation does not depend on the SDK's global offline flag.

Optional canonical vocabulary uses the SDK's separate CTC 110M acoustic model. Preparation downloads, verifies and loads it into an independently owned model handle, so downloads and CoreML setup do not hold the resident ASR lock. The bridge exports candidate scores and exact ranges alongside untouched TDT text; Sotto's Rust policy decides which substitutions are safe. The SDK's rewritten transcript is never applied directly. Empty vocabulary adds no CTC inference, and inference never downloads models. For recordings of 30 s or less the bridge decodes the audio once and gives the same samples to both models: at that size `AsrManager.transcribe(url)` itself decodes with the same stateless `AudioConverter` and passes the samples on. Above it the SDK streams from disk through a different converter, so longer recordings keep the file path and a second decode.

`VocabularyCache.swift` repairs only missing or invalid tokenizer metadata after explicit preparation. A replacement must pass the real SDK parser before an atomic write. Model bundles and failed replacement inputs remain intact. The offline `scripts/check-vocabulary-cache.py` probe exercises seven cache and failure cases using temporary files. Setup has per-transfer network limits, but no overall CoreML deadline.

`scripts/check-alignment.py` exercises the patched alignment against the pinned recursive source. `scripts/check-ctc-tensors.py` compares the actual patched conversion routines against element access across numeric types and non-contiguous strides. Both accept `--sdk-build` pointing to the Cargo-generated `swift-build` directory. Patches fail the build if the upstream source checksum changes.

To update: change the exact Swift SDK version, resolve/build, inspect upstream ASR/API and artifact changes, then rerun Sotto's WAV tests and the real multi-window speech regression corpus. Do not silently switch model version or cache paths. The app intentionally does not use streaming, VAD, TTS, or diarization APIs.
