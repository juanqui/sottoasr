"""Paired corpus WER on LibriSpeech test-other from model-probe logs.

Usage:
    python3 benchmarks/asr/score_librispeech.py /tmp/experiments/sotto-asr-models v3,ultra,nemotron-unified-int8

Reads <root>/libri/manifest.json and <root>/libri-<model>.log. The normalizer is
crude: casefold, tokens [a-z']+ and [0-9]+. Scores only the utterances that every
listed model transcribed. Each later model is compared with the first one: counts of
better and worse utterances, and a paired bootstrap 95% interval (2,000 resamples of
utterances, seed 0) for the WER difference in percentage points.
"""
import argparse
import json
import random
import re
import statistics
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
parser.add_argument('root', type=Path, help='experiment folder')
parser.add_argument('models', help='comma-separated probe model names; the first is the baseline')
args = parser.parse_args()
R = args.root
tok = lambda t: re.findall(r"[a-z']+|[0-9]+", t.casefold().replace('’', "'"))


def dist(a, b):
    row = list(range(len(b) + 1))
    for i, left in enumerate(a, 1):
        nxt = [i]
        for j, right in enumerate(b, 1):
            nxt.append(min(nxt[-1] + 1, row[j] + 1, row[j - 1] + (left != right)))
        row = nxt
    return row[-1]


def rows(path):
    out = {}
    # CoreML can print a diagnostic on stdout without a newline before the first JSON row.
    for line in path.read_text().splitlines():
        if '{"' in line:
            r = json.loads(line[line.index('{"'):])
            if r['event'] == 'transcribed':
                out[r['id']] = r
    return out


models = args.models.split(',')
fx = {i['id']: i for i in json.loads((R / 'libri' / 'manifest.json').read_text())}
got = {m: rows(R / f'libri-{m}.log') for m in models}
ids = [k for k in fx if all(k in got[m] for m in models)]
ref = {k: tok(fx[k]['expected']) for k in ids}
err = {m: {k: dist(ref[k], tok(got[m][k]['text'])) for k in ids} for m in models}
digit_free = [k for k in ids if not any(re.search(r'\d', got[m][k]['text']) for m in models)]
print(f'{len(ids)} utterances scored by all models ({len(fx)} in corpus); {len(digit_free)} with no digits in any hypothesis')
print('| model | errors | corpus WER % | WER %, digit-free subset | exact utterances | total decode s | audio/decode '
      '| median ms | p95 ms | ending in . ? ! | peak RSS MiB |')
print('|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|')
audio_s = 5.342 * 3600  # LibriSpeech test-other duration from the corpus documentation
for m in models:
    words = sum(len(ref[k]) for k in ids)
    wd = sum(len(ref[k]) for k in digit_free)
    secs = sum(got[m][k]['seconds'] for k in ids)
    times = sorted(got[m][k]['seconds'] * 1000 for k in ids)
    print(f"| {m} | {sum(err[m].values())} | {100 * sum(err[m].values()) / words:.2f} "
          f"| {100 * sum(err[m][k] for k in digit_free) / wd:.2f} | {sum(e == 0 for e in err[m].values())} "
          f"| {secs:.0f} | {audio_s / secs:.0f}x | {statistics.median(times):.0f} | {times[int(len(times) * .95) - 1]:.0f} "
          f"| {sum(got[m][k]['text'].rstrip().endswith(('.', '?', '!')) for k in ids)} "
          f"| {max(got[m][k]['peak_rss_bytes'] for k in ids) / 2**20:.0f} |")
random.seed(0)
base = models[0]
for m in models[1:]:
    better = sum(err[m][k] < err[base][k] for k in ids)
    worse = sum(err[m][k] > err[base][k] for k in ids)
    deltas = []
    for _ in range(2000):
        sample = random.choices(ids, k=len(ids))
        w = sum(len(ref[k]) for k in sample)
        deltas.append(100 * (sum(err[m][k] for k in sample) - sum(err[base][k] for k in sample)) / w)
    deltas.sort()
    print(f'{m} vs {base}: utterances better {better}, worse {worse}; WER delta 95% bootstrap CI '
          f'[{deltas[50]:+.2f}, {deltas[1949]:+.2f}] pp')
