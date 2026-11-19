# Sotto FluidAudio bridge

- **Version:** 1.4
- **Date:** 2026-11-19
- **Status:** Implemented

This is the small, ASR-only interface Sotto needs, based on the synchronous C ABI pattern from [FluidInference/fluidaudio-rs](https://github.com/FluidInference/fluidaudio-rs), upstream commit `569c7cff7eac1b65e10f83a596afb50a4cdf4031` (Cargo declares MIT). It replaces the old general-purpose wrapper so Sotto can use exact **FluidAudio 0.18.3**, the first pinned version that has the Parakeet Ultra model (`AsrModelVersion.ultra`, added in 0.17.3).

The Swift SDK is an Apache-2.0 dependency fetched by SwiftPM. `Package.resolved` pins its commit (`c8b41f98eae668794dd0083bbe028fe7e6585051`). Two checksum-guarded patches change it: they pin model downloads to fixed commits (below). `build.rs` applies each patch only to the exact upstream file it was written for and checks the result's checksum, so the build stops if either side changes. NemoTextProcessing remains checksum-pinned upstream. No model weights are vendored.

The two patches that 0.15.6 needed are gone, because 0.18.3 contains both changes: vocabulary alignment is iterative (FluidAudio #961), and CTC output is read in bulk with the input buffer cleared first (#1039). Upstream still copies the CTC input one sample at a time. A typed copy was measured and recovers 4% of the vocabulary step, so no patch is kept for it.

## Model revisions

The SDK builds model URLs in two places: `ModelRegistry.resolveModel` (`resolve/<revision>`) and `HFTreeLister.listTree` (`tree/<revision>`), with `main` as the revision of the repositories below. `patches/model-registry-revision.patch` adds a map of pinned commits keyed by repository and uses it when the revision is `main`; `patches/tree-lister-revision.patch` makes the tree listing use it. Other repositories are unchanged; the app does not use them.

| Model | Repository | Commit |
| --- | --- | --- |
| Speech (Parakeet Ultra) | `FluidInference/parakeet-ultra-coreml` | `95eaa59a39d4394f047a4dc5cce480388a60d1b6` |
| Vocabulary (CTC 110M) | `FluidInference/parakeet-ctc-110m-coreml` | `accdafd8cf8a2ff1cabe3c11e54416b405d409aa` |
| Previous speech (TDT v3, INT8), never downloaded | `FluidInference/parakeet-tdt-0.6b-v3-coreml` | `7dd20fe6b1797d35f5e3307e8b1732d9a178edfe` |

The Ultra commit is the repository's head since 2026-09-24 and holds the files the [model comparison](../../../docs/research/2026-11-19-parakeet-ultra.md) measured. The other two are the commits whose files match the caches the app was qualified with (checked 2026-11-19 against the tree API's Git LFS SHA-256 and Git blob IDs).

The pin is only in the URL. FluidAudio 0.18.3 can pin a repository itself (`Repo.revision`, `ModelRegistry.revisionOverrides`), and Sotto must not use that: with a revision other than `main`, the SDK's `ModelCache` deletes the model folder when its `.fluidaudio-revision` marker differs, deletes every local file the remote listing lacks, and deletes a whole bundle when one of its files differs. That would remove `.sotto-integrity.json`, the `.sotto-replaced-*` copies a re-download keeps, and warned files that are in use. With `main`, `ModelHub.download` differs from 0.15.6 only in passing the revision on, and a present file is never replaced. `scripts/check-asr-cache.py` fails if the SDK starts to pin these repositories.

`model-manifest.json` lists the SHA-256 and size of exactly the files the bridge loads. `speech` and `previous_speech` each hold every file in `Preprocessor`, `Encoder`, `Decoder` and `JointDecisionv3.mlmodelc` plus `parakeet_vocab.json` (Ultra: 18 files, 632 MB; v3: 21 files, 483 MB). `vocabulary` holds every file in `MelSpectrogram` and `AudioEncoder.mlmodelc` plus `vocab.json` and `tokenizer.json` (12 files, 103 MB). The app (`src-tauri/src/asr/integrity.rs`) checks every model the same way before CoreML loads anything:

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

## Speech model selection

The folder name selects the model: `AsrCache.load` loads `Models/parakeet-ultra/` as Parakeet Ultra and `Models/parakeet-tdt-0.6b-v3/` as TDT v3, and refuses any other folder. `AsrCache.download` fetches only Parakeet Ultra.

The app (`src-tauri/src/asr/model.rs`) chooses before each load, without the network:

1. Parakeet Ultra, when its files can load.
2. Otherwise TDT v3, when an earlier release left its files and they can load. Dictation works at once and onboarding does not open. Ultra then downloads in the background, is verified, and replaces v3 in memory when no dictation is in progress. If the download fails, v3 stays in use and the next start tries again.
3. Otherwise (a new installation) it downloads and verifies Ultra, then loads it.

"Can load" means that no file is missing and no newly downloaded file failed the manifest. If the verified Ultra files do not load in CoreML and v3 can load, the app loads v3 and Settings says so. Nothing deletes `Models/parakeet-tdt-0.6b-v3/` or the older `Models/parakeet-tdt-0.6b-v3-coreml/`; the first is this fallback, the second is not a cache of this SDK. Both can be deleted by hand once Settings shows Parakeet Ultra.

## Bridge behaviour

Local changes: only batch ASR is exposed; ownership permits `Send` but not `Sync`; calls require a mutable Rust reference; errors retain the underlying Swift reason; each recording has a fresh decoder state; WAV metadata supplies duration. The SDK caches a repository in `Models/<Repo.folderName>/`, which is the remote slug without its `-coreml` suffix for the speech models.

`AsrCache.swift` downloads missing Ultra artifacts with the SDK's supported downloader, as a separate step that never loads. Loading uses the SDK's local loader (`AsrModels.loadLocal`), which reads the verified bundles directly through CoreML and never downloads. The SDK's downloading load methods can delete the whole repository after a CoreML failure; Sotto does not enter that recovery path. Download and verification run without the resident engine lock; the lock covers only loading and compilation. An invalid cached bundle or vocabulary surfaces an error and stays intact. `scripts/check-asr-cache.py` tests incomplete-cache, offline-download, wrong-folder, actual CoreML-load, and malformed-vocabulary failures in disposable fixtures, that both downloaded repositories resolve to their pinned commits, and that the SDK does not pin them itself; its optional synthetic smoke clones the supplied cache before inference. The failed-model tests keep online permission enabled, proving preservation does not depend on the SDK's global offline flag.

Optional canonical vocabulary uses the SDK's separate CTC 110M acoustic model. Preparation downloads, verifies and loads it into an independently owned model handle, so downloads and CoreML setup do not hold the resident ASR lock. The bridge exports candidate scores and exact ranges alongside untouched TDT text; Sotto's Rust policy decides which substitutions are safe. The SDK's rewritten transcript is never applied directly. Empty vocabulary adds no CTC inference, and inference never downloads models. For recordings of 30 s or less the bridge decodes the audio once and gives the same samples to both models: at that size `AsrManager.transcribe(url)` itself decodes with the same stateless `AudioConverter` and passes the samples on. Above it the SDK streams from disk through a different converter, so longer recordings keep the file path and a second decode.

`VocabularyCache.swift` repairs only missing or invalid tokenizer metadata after explicit preparation. A replacement must pass the real SDK parser before an atomic write. Model bundles and failed replacement inputs remain intact. The offline `scripts/check-vocabulary-cache.py` probe exercises seven cache and failure cases using temporary files. Setup has per-transfer network limits, but no overall CoreML deadline.

To update: change the exact Swift SDK version, resolve/build, port the two revision patches and record their checksums, inspect upstream ASR/API, download and cache changes (`ModelHub.download`, `ModelCache`, `FileDownloader`), then rerun `scripts/check-asr-cache.py`, `scripts/check-vocabulary-cache.py`, Sotto's WAV tests and the real multi-window speech regression corpus. Do not silently switch model version or cache paths. The app intentionally does not use streaming, VAD, TTS, or diarization APIs.
