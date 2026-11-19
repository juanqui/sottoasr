# Default Speech Model: Parakeet Ultra and the Nemotron Models

- **Version:** 1.0
- **Date:** 2026-11-19
- **Status:** Implemented

## Table of Contents

1. [Question and decision](#1-question-and-decision)
2. [Nemotron Ultra ASR and the candidates](#2-nemotron-ultra-asr-and-the-candidates)
3. [Method](#3-method)
4. [Results](#4-results)
5. [Why the other candidates were not chosen](#5-why-the-other-candidates-were-not-chosen)
6. [Limits](#6-limits)
7. [Reproduction](#7-reproduction)

## 1. Question and decision

The question was whether "Nemotron Ultra ASR" should replace Parakeet TDT v3 as SottoASR's default speech model. No model with that name exists. This note compares v3 with the speech models that do exist under the Nemotron name, and with Parakeet Ultra, a post-training of v3.

**Decision:** the default speech model changes from Parakeet TDT v3 to Parakeet Ultra. Ultra needs FluidAudio 0.17.3 or later (`AsrModelVersion.ultra`), so the SDK moves from 0.15.6 to 0.18.3. The [specification](../specs/2026-11-19-parakeet-ultra-default.md) describes the change, including how existing installations keep v3 until Ultra is downloaded and verified.

Ultra made fewer word errors than v3 on LibriSpeech test-other, and the paired 95% interval for the difference excludes zero. On SottoASR's own synthetic clips the two are equal within noise, with the same punctuation and number format. The cost is a larger download (632 MB against 483 MB) and about 157 MiB more peak memory.

| Claim | Source |
|---|---|
| Word errors, WER, decode times, punctuation counts and peak memory in [section 4](#4-results) | Measured here, on the host in [section 3](#3-method) |
| Model origin, architecture, languages, licences, release dates, parameter counts and hardware needs in [section 2](#2-nemotron-ultra-asr-and-the-candidates) | Published by the model publishers |
| v3 and Ultra WER on LibriSpeech test-other on an M5 Pro (4.12% and 3.81%), and equal speed there | Published by FluidAudio |
| FLEURS 24-language mean (14.81% to 11.67%) | Published by FluidAudio, measured in its own harness |
| TED-LIUM long-form (2.71% to 1.94%) | Published upstream |

The probe, the scoring scripts and all per-clip outputs are in the repository. The [benchmark README](../../benchmarks/asr/README.md) lists them, and the [measurements file](../../benchmarks/asr/measurements-2026-11-19.json) holds the aggregates and the per-clip text.

## 2. Nemotron Ultra ASR and the candidates

"Nemotron Ultra ASR" does not exist. Nemotron 3 Ultra is NVIDIA's 550B-parameter LLM, not a speech model. The table lists the current default, Parakeet Ultra and the Nemotron speech models.

| Candidate | Publisher, release, licence | Kind | Measured here |
|---|---|---|---|
| [Parakeet TDT v3](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3) (current default) | NVIDIA, CC-BY-4.0 | FastConformer-TDT, 600M parameters, 25 European languages | Yes: [CoreML conversion](https://huggingface.co/FluidInference/parakeet-tdt-0.6b-v3-coreml), installed INT8 cache |
| [Parakeet Ultra](https://huggingface.co/moondream/parakeet-ultra) | moondream, CC-BY-4.0 | Full-precision post-training of `nvidia/parakeet-tdt-0.6b-v3`: same architecture, tokenizer, 25 languages and output conventions (punctuation, capitalization, digits) | Yes: [CoreML conversion](https://huggingface.co/FluidInference/parakeet-ultra-coreml) by FluidInference |
| [Nemotron Labs Unified ASR 0.6b](https://huggingface.co/nvidia/nemotron-labs-unified-asr-0.6b) | NVIDIA, 2026-10-27, OpenMDW-1.1 | RNN-T decoder; needs a language prompt or `auto` | Yes: [CoreML conversion](https://huggingface.co/FluidInference/nemotron-labs-unified-asr-0.6b-coreml), int8 and fp16 encoders |
| Nemotron 3.5 ASR streaming multilingual | NVIDIA, 2026-06-04 | Streaming | Yes: [CoreML conversion](https://huggingface.co/FluidInference/Nemotron-3.5-ASR-Streaming-Multilingual-0.6b-CoreML), `latin` variant, en-US, 2240 ms chunks |
| Nemotron ASR Streaming EN | NVIDIA, 2026-03-13 | Streaming, English | Yes: [CoreML conversion](https://huggingface.co/FluidInference/nemotron-speech-streaming-en-0.6b-coreml), 2240 ms chunks |
| [Nemotron 3.5 Transcribe 30B-A4B](https://huggingface.co/nvidia/NVIDIA-Nemotron-3.5-Transcribe-30B-A4B) | NVIDIA, 2026-10-28 | Speech LLM, 34B parameters; its model card specifies one H100 80 GB | No: not runnable on a Mac |

Licences that this research did not record are left out. The CoreML repository names are the ones FluidAudio 0.18.3 downloads from.

## 3. Method

**Host:** Apple M2 Pro, 32 GB, macOS 26.7.3, Swift 6.4, FluidAudio 0.18.3 (commit `c8b41f98eae668794dd0083bbe028fe7e6585051`). All models used the SDK's default CoreML compute units: CPU and Neural Engine.

**Probe:** [`benchmarks/asr/model-probe`](../../benchmarks/asr/model-probe/Sources/probe/main.swift) is a SwiftPM package that depends on FluidAudio 0.18.3. Each model and fixture set ran in its own process, one after another, under `/usr/bin/time -l`, so timings and peak memory stay separate. The probe loads the model, transcribes the first clip once to warm up, then writes one JSON line per clip with the text, the decode time, the CPU time and the peak resident memory.

Decode time is the wall time of one call. It includes reading and resampling the audio file. Long recordings are decoded in memory, in one call with all samples.

| Probe name | Model | Settings |
|---|---|---|
| `v3` | Parakeet TDT v3 | APFS clone of the installed INT8 cache, loaded with `AsrModels.loadLocal` (no network) |
| `ultra` | Parakeet Ultra | `AsrModels.downloadAndLoad` with `.ultra` into the experiment folder |
| `nemotron-unified-int8`, `nemotron-unified-fp16` | Nemotron Labs Unified ASR 0.6b | `UnifiedAsrManager`, `en` prompt, int8 or fp16 encoder |
| `nemotron-unified-auto-int8` | Nemotron Labs Unified ASR 0.6b | `auto` prompt, int8 encoder |
| `nemotron35-ml-2240` | Nemotron 3.5 ASR streaming multilingual | `StreamingNemotronMultilingualAsrManager`, en-US, 2240 ms chunks |
| `nemotron-en-2240` | Nemotron ASR Streaming EN | `StreamingNemotronAsrManager`, 2240 ms chunks |

The streaming models receive the whole recording at once. The probe reads their text after `finish()`.

**Fixtures:**

| Set | Content | Reference words |
|---|---|---:|
| dev-40 | [`development-40.json`](../../benchmarks/asr/development-40.json): 20 sentences in two macOS voices, 40 clips, 18 ordinary-speech (negative) clips | 392 |
| holdout-48 | [`independent-24.json`](../../benchmarks/asr/independent-24.json): 24 sentences in two voices, 48 clips, 24 negative clips | 436 |
| dev-40 padded | The dev-40 clips with 0.5 s of silence before and after | 392 |
| long-dev, long-holdout | All clips of a set in one file, each clip followed by 0.6 s of silence: 149.7 s and 172.5 s | 392, 436 |
| LibriSpeech test-other | The full set from [OpenSLR](https://www.openslr.org/resources/12/test-other.tar.gz): 2,939 utterances | 52,343 |

`benchmarks/asr/prepare.py` synthesizes the clips with `say` (Samantha and Daniel). The padded and long sets are derived from those files ([section 7](#7-reproduction)).

**Metric:**

- Fixtures: the metric of `benchmarks/asr/analyze.py`. Text is casefolded and split into tokens `[a-z]+` and `[0-9]+`, and word errors are the edit distance between token lists. Case and punctuation are ignored. A number written as words where the reference has digits counts as errors.
- LibriSpeech: casefold, tokens `[a-z']+` and `[0-9]+`. Paired bootstrap with 2,000 resamples of utterances.
- An output "ends in . ? !" when its last character, after trailing white space, is one of these.
- Peak RSS is the largest resident memory the probe reported in its process.

## 4. Results

### 4.1 LibriSpeech test-other, full set: 2,939 utterances, 52,343 reference words

| Model | Word errors | WER % | Total decode s | Median ms | p95 ms | Outputs ending in . ? ! | Peak RSS MiB |
|---|---:|---:|---:|---:|---:|---:|---:|
| Parakeet TDT v3 (installed INT8 cache) | 2,156 | 4.12 | 225 | 68 | 152 | 2,550 | 487 |
| Parakeet Ultra | 1,990 | 3.80 | 254 | 76 | 171 | 2,658 | 644 |
| Nemotron Labs Unified ASR 0.6b, int8, `en` prompt | 1,880 | 3.59 | 371 | 101 | 263 | 1,721 | 643 |

| Comparison | Utterances better | Utterances worse | WER difference, 95% interval (points) |
|---|---:|---:|---|
| Ultra against v3 | 295 | 169 | [−0.45, −0.19] |
| Unified against v3 | 399 | 224 | [−0.68, −0.38] |
| Unified against Ultra | 321 | 253 | [−0.34, −0.08] |

FluidAudio publishes 4.12% and 3.81% for v3 and Ultra on the same set (M5 Pro), so the result reproduces. Peak memory rises by about 157 MiB from v3 to Ultra (487 to 644 MiB).

### 4.2 SottoASR synthetic fixtures: 88 clips, 828 reference words

The sets are dev-40 (392 words) and holdout-48 (436 words); the metric is that of `analyze.py`.

| Model | Errors dev-40 (392 words) | Errors holdout-48 (436 words) | Total | Clips ending in . ? ! (of 88) | Median ms dev / holdout | Peak RSS MiB |
|---|---:|---:|---:|---:|---|---:|
| Parakeet TDT v3 | 35 | 27 | 62 | 88 | 58.4 / 55.1 | 500 |
| Parakeet Ultra | 33 | 25 | 58 | 88 | 59.1 / 56.6 | 663 |
| Nemotron Unified int8 (`en`) | 35 | 24 | 59 | 64 | 67.7 / 67.1 | 672 |
| Nemotron Unified fp16 (`en`) | 34 | 24 | 58 | 64 | 63.1 / 62.7 | 1,200 |
| Nemotron Unified int8 (`auto`) | 35 | 24 | 59 | not counted | 65.3 / 64.8 | — |
| Nemotron 3.5 ASR streaming multilingual (latin, en-US, 2240 ms) | 63 | 55 | 118 | 0 | 71.4 / 76.8 | 638 |
| Nemotron ASR Streaming EN (2240 ms) | 54 | 57 | 111 | 0 | 61.4 / 58.8 | 631 |

- v3 made the same 35 dev-40 errors as in the [September run](2026-09-08-vocabulary-asr.md#5-reproducible-m4-experiment) on an M4 with FluidAudio 0.15.6. 37 of 40 outputs were byte-identical. `say` synthesized the audio again, so this is not an exact comparison.
- Ultra fixes "PostgreSQL" in two clips and turns "lunch" into "launch" in one clip. Differences of a few words in 828 are noise.
- Negative (ordinary-speech) clips wrong, dev / holdout: v3 0 / 5, Ultra 1 / 5, Unified int8 1 / 3.

### 4.3 Long recordings

The same clips, concatenated with 0.6 s pauses: 149.7 s and 172.5 s, 828 reference words, decoded in memory.

| Model | Errors long-dev | Errors long-holdout | Total | Decode s (dev / holdout) |
|---|---:|---:|---:|---|
| Parakeet TDT v3 | 43 | 22 | 65 | 0.55 / 0.53 |
| Parakeet Ultra | 42 | 25 | 67 | 0.53 / 0.54 |
| Nemotron Unified int8 | 32 | 27 | 59 | 1.43 / 1.64 |
| Nemotron 3.5 streaming multilingual | 54 | 50 | 104 | 1.45 / 1.94 |
| Nemotron Streaming EN | 50 | 41 | 91 | 2.11 / 2.33 |

### 4.4 Controls

- **Leading silence.** The `say` clips start speaking 2 to 26 ms after the file starts. With 0.5 s of silence added before and after (dev-40), the errors were: v3 34, Ultra 34, Unified int8 34, Nemotron 3.5 streaming 60, Nemotron Streaming EN 51.
- With the padding, first-word errors of the two streaming models drop from 7 and 9 to 2, the same as v3. They still end no clip with punctuation and spell numbers out ("three point eight", "thirty eight"). Unified ends 28 of the 40 padded clips with punctuation.
- **Number format.** With number formats made equal in scoring, Nemotron Streaming EN made 35 and Nemotron 3.5 streaming 44 errors on padded dev-40, against 34 for v3. This rescoring is not part of `score_models.py`.

## 5. Why the other candidates were not chosen

| Candidate | Reason |
|---|---|
| "Nemotron Ultra ASR" | Does not exist. Nemotron 3 Ultra is NVIDIA's 550B-parameter LLM. |
| Nemotron Labs Unified ASR 0.6b | Lowest WER here, but see below. |
| Nemotron 3.5 ASR streaming, Nemotron ASR Streaming EN | No terminal punctuation, numbers spelled out, about twice the errors of v3. Suited to a live preview, not to the final transcript. |
| Nemotron 3.5 Transcribe 30B-A4B | A 34B-parameter speech LLM whose model card specifies one H100 80 GB. Not runnable on a Mac. |

Unified has the best WER on LibriSpeech and on the long recordings. It was not chosen for these reasons:

- It drops the final period on about a quarter of the fixture clips. 41% of its LibriSpeech outputs lack terminal punctuation, against 13% for v3.
- It is about 3 times slower than v3 on the long recordings.
- It needs a language prompt or `auto`.
- It has a different decoder (RNN-T) and a different SDK loader, with purge-and-retry recovery.
- SottoASR's vocabulary guard would need validation again.

Ultra keeps the architecture, tokenizer, languages and output conventions of v3. The vocabulary path and the output format therefore stay the same. The bridge changes only in how it downloads and loads the model.

## 6. Limits

- The 88 clips are synthetic speech in two macOS voices, English only. LibriSpeech is read English.
- Non-English speech, noisy speech and real long recordings were not measured. Ultra's published gains are largest there: FLEURS 24-language mean 14.81% to 11.67% in FluidAudio's harness, TED-LIUM long-form 2.71% to 1.94% upstream.
- Ultra took 13% longer in total on LibriSpeech on this M2 Pro (254 s against 225 s). FluidAudio reports equal speed on an M5 Pro. Short clips showed no difference.
- No energy measurement.
- Each configuration ran once. The timings are single runs, not averages.
- The probe downloaded Ultra from the repository's `main` branch. The application pins commit `95eaa59a39d4394f047a4dc5cce480388a60d1b6`, which was the head of that branch. A later run of the probe gets the head at that time.
- The probe decodes in memory. The application sends recordings above the SDK's streaming threshold through `transcribeDiskBacked`, which this research did not exercise. The spec's real-model checks cover that path.
- SottoASR's dictionary, vocabulary and cleanup steps were not applied. The outputs are the SDK's text.

## 7. Reproduction

Run from the repository root. Use a new folder under `/tmp/experiments/`; `prepare.py` refuses to write into an existing folder. `say` must have the Samantha and Daniel voices.

Synthesize the fixtures, then derive the padded and long sets:

```bash
E=/tmp/experiments/sotto-asr-models
python3 benchmarks/asr/prepare.py "$E/dev"
python3 benchmarks/asr/prepare.py "$E/holdout" --independent
python3 - "$E" <<'EOF'
import json, shutil, sys, wave
from pathlib import Path
root = Path(sys.argv[1])


def frames(path):
    with wave.open(str(path)) as w:
        return w.readframes(w.getnframes())


def write(path, data):
    with wave.open(str(path), 'wb') as w:
        w.setnchannels(1), w.setsampwidth(2), w.setframerate(16000), w.writeframes(data)


(root / 'dev-pad').mkdir()
for item in json.loads((root / 'dev/manifest.json').read_text()):  # 0.5 s of silence before and after
    write(root / 'dev-pad' / item['file'], bytes(16000) + frames(root / 'dev' / item['file']) + bytes(16000))
shutil.copy(root / 'dev/manifest.json', root / 'dev-pad/manifest.json')
for name in ['dev', 'holdout']:  # all clips in one file, each followed by 0.6 s of silence
    items = json.loads((root / name / 'manifest.json').read_text())
    (root / f'long-{name}').mkdir()
    write(root / f'long-{name}/long.wav', b''.join(frames(root / name / i['file']) + bytes(19200) for i in items))
    expected = ' '.join(i['expected'] for i in items)
    manifest = [{'id': f'long_{name}', 'file': 'long.wav', 'expected': expected, 'negative': False}]
    (root / f'long-{name}/manifest.json').write_text(json.dumps(manifest, indent=2))
EOF
```

Download LibriSpeech test-other and write its manifest:

```bash
mkdir "$E/libri"
curl -L https://www.openslr.org/resources/12/test-other.tar.gz | tar -xz -C "$E/libri"
python3 - "$E" <<'EOF'
import json, sys
from pathlib import Path
libri = Path(sys.argv[1]) / 'libri'
items = []
for trans in libri.glob('LibriSpeech/test-other/*/*/*.trans.txt'):
    for line in trans.read_text().splitlines():
        ident, text = line.split(' ', 1)
        items.append({'id': ident, 'file': str(trans.parent.relative_to(libri) / f'{ident}.flac'),
                      'expected': text.lower(), 'negative': False})
(libri / 'manifest.json').write_text(json.dumps(sorted(items, key=lambda i: i['id'])))
EOF
```

Clone the installed v3 cache. The clone leaves the app's cache untouched, and the probe loads v3 only from it. The other models download into `$E/Models`.

```bash
mkdir -p "$E/Models"
cp -cR "$HOME/Library/Application Support/FluidAudio/Models/parakeet-tdt-0.6b-v3" "$E/Models/"
```

Build the probe. `--scratch-path` keeps the build output out of the repository.

```bash
(cd benchmarks/asr/model-probe && swift build -c release --scratch-path "$E/probe-build")
P="$(cd benchmarks/asr/model-probe && swift build -c release --scratch-path "$E/probe-build" --show-bin-path)/probe"
```

Run one model per process, one after another. The original run did not run `nemotron-unified-fp16` and `nemotron-unified-auto-int8` on `dev-pad`, nor `nemotron-unified-auto-int8` on the long sets.

```bash
for set in dev holdout dev-pad long-dev long-holdout; do
  for m in v3 ultra nemotron-unified-int8 nemotron-unified-fp16 nemotron-unified-auto-int8 nemotron35-ml-2240 nemotron-en-2240; do
    /usr/bin/time -l "$P" "$E" "$E/$set" "$m" > "$E/$set-$m.log" 2> "$E/$set-$m.err"
  done
done
for m in v3 ultra nemotron-unified-int8; do
  /usr/bin/time -l "$P" "$E" "$E/libri" "$m" > "$E/libri-$m.log" 2> "$E/libri-$m.err"
done
```

Score:

```bash
python3 benchmarks/asr/score_models.py "$E" dev,holdout,dev-pad,long-dev,long-holdout v3,ultra,nemotron-unified-int8,nemotron-unified-fp16,nemotron-unified-auto-int8,nemotron35-ml-2240,nemotron-en-2240
python3 benchmarks/asr/score_librispeech.py "$E" v3,ultra,nemotron-unified-int8
python3 benchmarks/asr/score_librispeech.py "$E" ultra,nemotron-unified-int8
```

`score_models.py` prints one table per set; `--diff` also lists the clips that differ from the first model. `score_librispeech.py` compares every model with the first one, so the comparison of Unified with Ultra needs the second command.
