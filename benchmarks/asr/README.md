# Synthetic ASR vocabulary experiments

- **Version:** 1.2
- **Date:** 2026-11-19
- **Status:** Implemented

These fixtures contain only locally synthesized speech. No personal audio, history, model weights, or credentials are included. The [research note](../../docs/research/2026-09-08-vocabulary-asr.md) explains the decision and measurement limits. The [November 2026 model comparison](#november-2026-model-comparison) reuses the same fixtures.

## Table of Contents

1. [Evidence](#evidence)
2. [Reproduction](#reproduction)
3. [Results and limits](#results-and-limits)
4. [November 2026 model comparison](#november-2026-model-comparison)

## Evidence

| Artifact | Purpose |
|---|---|
| `development-40.json` | Twenty adversarial sentences in two macOS voices; includes spoken and scoring reference text |
| `independent-24.json` | Fresh holdout authored before the candidate policy was evaluated; two voices produce 48 clips |
| `vocabulary-holdout-evidence.json` | Actual candidate ranges/scores and approved outputs for the Rust policy's 48-case regression |
| `measurements-2026-09-08.json` | Per-clip outputs, latency/CPU/RSS counters, aggregate accuracy, model/runtime revisions |
| `frozen_policy_2026_09_08.rs` | Exact source archived when the candidate policy was frozen, before revealing the holdout |

All runs used an Apple M4 (4 performance + 6 efficiency CPU cores), 32 GiB RAM. Timing windows were serialized with other inference and builds paused. Neither measured WER nor zero observed false positives on this small synthetic corpus is a production guarantee. No power/energy measurements were available; CPU time and RSS are proxies, not watts or battery savings.

## Reproduction

Run from the repository root. Use a new directory under `/tmp/experiments/`; preparation refuses to overwrite an existing directory. `say` must have the local Samantha and Daniel voices available.

The September probes (`vocabulary_probe.swift`, `unified_probe.swift`, `compute_probe.swift`, built by `build_probes.py`) were written for FluidAudio 0.15.6 and reproduce at tag `v0.11.1`. The application now pins 0.18.3, so check out that tag before running the commands in this section.

```bash
python3 benchmarks/asr/prepare.py /tmp/experiments/sotto-asr-reproduction
mkdir -p /tmp/experiments/sotto-asr-reproduction/Models
cp -cR "$HOME/Library/Application Support/FluidAudio/Models/parakeet-tdt-0.6b-v3" /tmp/experiments/sotto-asr-reproduction/Models/
```

Build Sotto once from `src-tauri/` first, with output captured using `tee`. Pass the resulting `fluidaudio-rs-*/out/swift-build` directory explicitly; this links probes without rebuilding the SDK. The pinned SDK is 0.15.6, commit `4dbf4f9f9a5ff3a53ade848d7ba4e3df13db859b`.

```bash
python3 benchmarks/asr/build_probes.py /tmp/experiments/sotto-asr-reproduction --sdk-build /absolute/path/to/src-tauri/target/debug/build/fluidaudio-rs-HASH/out/swift-build
set -o pipefail
/usr/bin/time -l /tmp/experiments/sotto-asr-reproduction/vocabulary_probe /tmp/experiments/sotto-asr-reproduction 2>&1 | tee /tmp/experiments/sotto-asr-reproduction/tdt-ctc.log
python3 benchmarks/asr/analyze.py /tmp/experiments/sotto-asr-reproduction/tdt-ctc.log
/usr/bin/time -l /tmp/experiments/sotto-asr-reproduction/unified_probe /tmp/experiments/sotto-asr-reproduction 2>&1 | tee /tmp/experiments/sotto-asr-reproduction/unified.log
python3 benchmarks/asr/analyze.py /tmp/experiments/sotto-asr-reproduction/unified.log
```

The original TDT measurements read the installed v3 cache. SDK 0.15.6 uses the local folder `parakeet-tdt-0.6b-v3`, without the remote repository slug’s `-coreml` suffix; the legacy folder is not the current cache. The durable harness uses an isolated clone of those same weights, because the SDK's generic recovery loader can purge a failed cache. The clone command leaves the app's cache untouched. CTC 110M and Unified EN download only into the experiment's `Models` directory. The vocabulary probe deliberately exercises upstream default rescoring failures and exports raw candidate evidence; it does not activate those policies in Sotto. Production applies only the separately tested Rust guard.

For the independent set, use `prepare.py NEW_DIRECTORY --independent`, then pass its generated `manifest.json` to `analyze.py --fixtures`. The manifest uses the same written-number references for both compared recognition paths. The exact source holdout SHA-256 is `79b2062c486d086c15ecaea55b7ec6d1773e42992da21ea3c343feb7987d8687`; the frozen Rust source SHA-256 is `13c2670bfb7543bcd0477f200820bdd8629cd0804e233f467b39a751c92c9ef0`.

Qwen's optional probe requires an isolated Python environment with MLX 0.32.2 and mlx-audio 0.5.3. Download `mlx-community/Qwen3-ASR-0.6B-8bit` revision `89e96d92ba34aca20b3e29fb10cc284097d1219f` into the experiment's `Models/Qwen3-ASR-0.6B-8bit` directory, then run:

```bash
/path/to/isolated/python benchmarks/asr/qwen_probe.py /tmp/experiments/sotto-asr-reproduction 2>&1 | tee /tmp/experiments/sotto-asr-reproduction/qwen.log
python3 benchmarks/asr/analyze.py /tmp/experiments/sotto-asr-reproduction/qwen.log
```

Qwen inference sets offline mode and never downloads. The probe rotates baseline/hotword/preservation-prompt order, uses temperature 0 and 256 maximum output tokens, and records whether the cap was reached.

`compute_probe ROOT MODE` compares identical cached INT8 TDT v3 weights with `ane`, `all`, `gpu_encoder`, or `cpu`. It disables downloads, warms one clip, then measures all 40 fixtures. Each mode runs in its own process so model lifetime and peak RSS remain separate. `gpu_encoder` changes only the encoder to CPU/GPU; other components retain their default CPU/ANE configuration. The SDK always pins the preprocessor to CPU. These are allowed compute-unit sets; they do not prove every model operation ran on one particular accelerator.

Run all four modes serially, with a new per-mode APFS clone and a bounded owned process for each:

```bash
python3 benchmarks/asr/run_compute.py /tmp/experiments/sotto-asr-reproduction 2>&1 | tee /tmp/experiments/sotto-asr-reproduction/compute-modes.log
python3 benchmarks/asr/analyze.py /tmp/experiments/sotto-asr-reproduction/mode-ane/results.log
```

The official Granite probe uses the same isolated MLX environment as Qwen, without remote Python code. Download `ibm-granite/granite-speech-5.0-470m-turboctc` revision `18ca3c1de6cd092b5a30c39fb0f04550b38ed1a0` into `Models/granite-speech-5.0-470m-turboctc` inside the experiment, including JSON metadata and the original safetensors weights (946,180,704 bytes). Its native mlx-audio implementation greedily decodes CTC without a vocabulary prompt.

```bash
/usr/bin/time -l /path/to/isolated/python benchmarks/asr/granite_probe.py /tmp/experiments/sotto-asr-reproduction 2>&1 | tee /tmp/experiments/sotto-asr-reproduction/granite.log
python3 benchmarks/asr/analyze.py /tmp/experiments/sotto-asr-reproduction/granite.log
```

## Results and limits

Raw TDT v3 made 35 word errors across 392 reference words, versus 34 for Unified EN and 31 for Qwen with hotwords. Unified reduced median short-clip latency from 63.42 to 44.81 ms and aggregate CPU time from 2.240 to 0.820 s, but introduced a missing initial word and an MLX→NLX error. Qwen hotwords improved intended technical names but changed an ordinary personal name. Neither result justified replacing the multilingual TDT v3 default.

Upstream CTC defaults reduced aggregate errors but changed 8 of 18 ordinary development clips. The frozen Sotto guard improved 4 of 24 target/mixed independent clips, with 0 changes among 24 ordinary clips and no observed target regression. Ambiguous Quinn/Qwen cases often remained unchanged; versioned identifiers are stored but unsupported token sequences cannot produce partial replacements.

The measured warm inference excludes microphone shutdown, WAV serialization, history, cleanup, and paste. It also excludes production rescorer construction and tokenizer parsing, so the CTC timing is not an end-to-end dictation latency claim. Peak RSS and Metal allocation overlap and must not be added.


The controlled compute comparison produced byte-identical outputs in all four modes. CPU/ANE median latency was 62.70 ms with 2.271 seconds aggregate CPU and 501.58 MiB RSS; allowing all devices was 66.12 ms / 2.281 seconds / 2013.97 MiB. The GPU encoder and CPU-only controls were both about 138 ms. This supports retaining CPU/ANE on this M4, without claiming measured battery savings. See the research note for all p95, load-time, and memory figures.


Granite Speech 5.0 TurboCTC was faster still at 38.96 ms median (41.85 p95) and 0.534 seconds aggregate CPU, but made 38 word errors versus TDT’s 35. Both had 20 normalized exact clips; case/punctuation differences were ignored consistently. Granite used 1.073 GB peak RSS and 1.092 GB peak Metal allocation (overlapping counters). Its raw CTC output did not improve the required technical vocabulary, so it was not selected.


The final app's pinned-v3 loader avoids the SDK's destructive load-recovery path while preserving the measured model configuration. Verify the actual helper's cache-failure handling and one synthetic transcription without modifying existing models:

```bash
python3 scripts/check-asr-cache.py --model-dir /tmp/experiments/sotto-asr-reproduction/Models/parakeet-tdt-0.6b-v3 --audio /tmp/experiments/sotto-asr-reproduction/samantha_ordinary.wav 2>&1 | tee /tmp/experiments/sotto-asr-reproduction/cache-safety.log
```

The probe creates its own disposable clone. Its three fault cases preserve an existing sentinel, all model directories, and vocabulary bytes. The verified synthetic result was “Please keep the blue notebook beside the window and remember the silver telescope.” It is a functional equivalence check, not an additional timed performance claim.

## November 2026 model comparison

On 2026-11-19, Parakeet TDT v3, Parakeet Ultra and three Nemotron speech models were compared on an Apple M2 Pro (32 GB, macOS 26.7.3) with FluidAudio 0.18.3. Ultra became the default speech model. The [research note](../../docs/research/2026-11-19-parakeet-ultra.md) explains the decision, the method and the limits.

| Artifact | Purpose |
|---|---|
| `model-probe/` | SwiftPM probe on FluidAudio 0.18.3. Loads one model, warms up on one clip, then writes one JSON line per clip |
| `score_models.py` | Scores fixture sets with the metric of `analyze.py`, one table per set |
| `score_librispeech.py` | Paired corpus WER on LibriSpeech test-other, with a bootstrap interval for each model against the first |
| `measurements-2026-11-19.json` | Model revisions, aggregates, LibriSpeech comparisons, and per-clip outputs of the fixture sets |
| `vocabulary-holdout-evidence-2026-11-19.json` | Candidate ranges, scores and approved outputs of the 48 holdout clips with Parakeet Ultra and FluidAudio 0.18.3, for the Rust policy's regression test. Recorded through the application's bridge with a temporary example program, which is not in the repository |

The fixtures are the September sets (`development-40.json`, `independent-24.json`), a padded copy of dev-40, and both sets concatenated into one long file each. LibriSpeech audio and per-utterance LibriSpeech outputs are not included.

| Model | LibriSpeech test-other errors (WER) | Fixture errors, 828 words | Long-recording errors, 828 words |
|---|---:|---:|---:|
| Parakeet TDT v3 | 2,156 (4.12%) | 62 | 65 |
| Parakeet Ultra | 1,990 (3.80%) | 58 | 67 |
| Nemotron Unified int8 (`en`) | 1,880 (3.59%) | 59 | 59 |
| Nemotron 3.5 streaming multilingual | not measured | 118 | 104 |
| Nemotron Streaming EN | not measured | 111 | 91 |

Against v3, Ultra was better on 295 LibriSpeech utterances and worse on 169. The 95% interval for the WER difference is −0.45 to −0.19 points. Peak resident memory rose from 487 to 644 MiB. The research note has the full tables, including timings, punctuation and the controls.

With vocabulary on, the unchanged policy now changes 2 of the 24 target clips (4 in September) and still changes none of the 24 ordinary clips. Word errors go from 25 to 22 in 436 words. The reason is the SDK, not the model: FluidAudio 0.18.3 runs the CTC mel spectrogram on the CPU, and the spoken words then score higher (for "mini CPM", −5.2 against −5.8 for "MiniCPM"; with the mel spectrogram on the Neural Engine, as in 0.15.6, −11.0 against −10.3). The vocabulary step on a 16-minute recording took 5.6 s, against 3.3 s with the patched 0.15.6. Section 12 of the [specification](../../docs/specs/2026-11-19-parakeet-ultra-default.md) has these measurements.

Reproduce from the repository root. Section 7 of the research note has the full commands, including the Python steps that derive `dev-pad`, `long-dev`, `long-holdout` and `libri/manifest.json`. `--scratch-path` keeps the build output out of the repository.

```bash
E=/tmp/experiments/sotto-asr-models
python3 benchmarks/asr/prepare.py "$E/dev"
python3 benchmarks/asr/prepare.py "$E/holdout" --independent
# Derive dev-pad, long-dev, long-holdout and libri/ here: research note, section 7.
mkdir -p "$E/Models"
cp -cR "$HOME/Library/Application Support/FluidAudio/Models/parakeet-tdt-0.6b-v3" "$E/Models/"
(cd benchmarks/asr/model-probe && swift build -c release --scratch-path "$E/probe-build")
P="$(cd benchmarks/asr/model-probe && swift build -c release --scratch-path "$E/probe-build" --show-bin-path)/probe"
for set in dev holdout dev-pad long-dev long-holdout; do
  for m in v3 ultra nemotron-unified-int8 nemotron-unified-fp16 nemotron-unified-auto-int8 nemotron35-ml-2240 nemotron-en-2240; do
    /usr/bin/time -l "$P" "$E" "$E/$set" "$m" > "$E/$set-$m.log" 2> "$E/$set-$m.err"
  done
done
for m in v3 ultra nemotron-unified-int8; do
  /usr/bin/time -l "$P" "$E" "$E/libri" "$m" > "$E/libri-$m.log" 2> "$E/libri-$m.err"
done
python3 benchmarks/asr/score_models.py "$E" dev,holdout,dev-pad,long-dev,long-holdout v3,ultra,nemotron-unified-int8,nemotron-unified-fp16,nemotron-unified-auto-int8,nemotron35-ml-2240,nemotron-en-2240
python3 benchmarks/asr/score_librispeech.py "$E" v3,ultra,nemotron-unified-int8
python3 benchmarks/asr/score_librispeech.py "$E" ultra,nemotron-unified-int8
```

The probe loads v3 only from the clone; the other models download into `$E/Models`. Ultra downloads from the repository's `main` branch, whose head was the commit the application pins (`95eaa59a39d4394f047a4dc5cce480388a60d1b6`) at the time of the run.
