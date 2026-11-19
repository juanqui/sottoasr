"""Score the model-probe logs of an experiment folder with the metric of analyze.py.

Usage:
    python3 benchmarks/asr/score_models.py /tmp/experiments/sotto-asr-models dev,holdout v3,ultra

Each set is a fixture folder with a manifest.json inside the experiment folder. The
logs are <set>-<model>.log (probe stdout) and <set>-<model>.err (stderr of
/usr/bin/time -l, for the peak memory footprint). Prints one table per set. With
--diff, also lists the clips whose scored tokens differ from the first model.
"""
import argparse
import json
import re
import statistics
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
parser.add_argument('root', type=Path, help='experiment folder')
parser.add_argument('sets', help='comma-separated fixture sets, for example dev,holdout')
parser.add_argument('models', help='comma-separated probe model names, for example v3,ultra')
parser.add_argument('--diff', action='store_true', help='list clips that differ from the first model')
args = parser.parse_args()
R = args.root


def tokens(text):
    return re.findall(r'[a-z]+|[0-9]+', text.casefold())


def distance(a, b):
    row = list(range(len(b) + 1))
    for i, left in enumerate(a, 1):
        nxt = [i]
        for j, right in enumerate(b, 1):
            nxt.append(min(nxt[-1] + 1, row[j] + 1, row[j - 1] + (left != right)))
        row = nxt
    return row[-1]


def load(set_name, model):
    log = R / f'{set_name}-{model}.log'
    if not log.is_file():
        return None
    # CoreML can print a diagnostic on stdout without a newline before the first JSON row.
    rows = [json.loads(line[line.index('{"'):]) for line in log.read_text().splitlines() if '{"' in line]
    loaded = next((r for r in rows if r['event'] == 'loaded'), None)
    out = {r['id']: r for r in rows if r['event'] == 'transcribed'}
    err = (R / f'{set_name}-{model}.err').read_text() if (R / f'{set_name}-{model}.err').is_file() else ''
    foot = re.search(r'(\d+)\s+peak memory footprint', err)
    return loaded, out, int(foot.group(1)) if foot else None


sets = args.sets.split(',')
models = args.models.split(',')
for set_name in sets:
    fixtures = {i['id']: i for i in json.loads((R / set_name / 'manifest.json').read_text())}
    words = sum(len(tokens(i['expected'])) for i in fixtures.values())
    negatives = [i for i in fixtures.values() if i.get('negative')]
    print(f'\n## {set_name}: {len(fixtures)} clips, {words} reference words, {len(negatives)} negative clips')
    print('| model | errors | WER % | exact | neg wrong | median ms | p95 ms | cpu s | load s | peak RSS MiB | footprint MiB |')
    print('|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|')
    results = {}
    for model in models:
        got = load(set_name, model)
        if not got or set(got[1]) != set(fixtures):
            print(f'| {model} | incomplete |')
            continue
        loaded, out, foot = got
        results[model] = out
        errs = {k: distance(tokens(fixtures[k]['expected']), tokens(out[k]['text'])) for k in fixtures}
        times = sorted(r['seconds'] * 1000 for r in out.values())
        footprint = f'{foot / 2**20:.0f}' if foot else '?'
        print(f"| {model} | {sum(errs.values())} | {100 * sum(errs.values()) / words:.2f} "
              f"| {sum(e == 0 for e in errs.values())} | {sum(errs[i['id']] > 0 for i in negatives)} "
              f"| {statistics.median(times):.1f} | {times[int(len(times) * .95) - 1]:.1f} "
              f"| {sum(r['cpu_seconds'] for r in out.values()):.2f} | {loaded['seconds']:.1f} "
              f"| {max(r['peak_rss_bytes'] for r in out.values()) / 2**20:.0f} | {footprint} |")
    if args.diff and models[0] in results:
        base = results[models[0]]
        for model in models[1:]:
            if model not in results:
                continue
            print(f'\n### {set_name}: {model} vs {models[0]} (clips whose scored tokens differ)')
            for k, item in fixtures.items():
                a, b = base[k]['text'], results[model][k]['text']
                if tokens(a) == tokens(b):
                    continue
                ref = tokens(item['expected'])
                da, db = distance(ref, tokens(a)), distance(ref, tokens(b))
                tag = 'better' if db < da else 'worse' if db > da else 'same'
                print(f"- [{tag} {da}->{db}]{' NEG' if item.get('negative') else ''} {k}\n    ref: {item['expected']}\n    {models[0]}: {a}\n    {model}: {b}")
