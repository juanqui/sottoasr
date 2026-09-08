"""Score saved JSON-lines probe output without running models."""
import argparse
import json
import re
import statistics
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('log', type=Path)
parser.add_argument('--fixtures', type=Path, default=Path(__file__).with_name('development-40.json'))
args = parser.parse_args()
fixtures = {item['id']: item for item in json.loads(args.fixtures.read_text())}
rows = [json.loads(line) for line in args.log.read_text().splitlines() if line.startswith('{')]

def tokens(text):
    return re.findall(r'[a-z]+|[0-9]+', text.casefold())

def distance(a, b):
    row = list(range(len(b) + 1))
    for i, left in enumerate(a, 1):
        nxt = [i]
        for j, right in enumerate(b, 1):
            nxt.append(min(nxt[-1] + 1, row[j] + 1, row[j-1] + (left != right)))
        row = nxt
    return row[-1]

groups = {}
for row in rows:
    if row['event'] in ('baseline', 'rescored', 'transcribed'):
        groups.setdefault(row.get('policy', row['event']), []).append(row)
summary = {}
for name, group in groups.items():
    if {row['id'] for row in group} != set(fixtures):
        raise ValueError(f'{name}: incomplete or unexpected corpus')
    times = sorted(row['seconds'] * 1000 for row in group)
    errors = sum(distance(tokens(fixtures[row['id']]['expected']), tokens(row['text'])) for row in group)
    words = sum(len(tokens(item['expected'])) for item in fixtures.values())
    summary[name] = {
        'word_errors': errors, 'reference_words': words, 'wer': errors / words,
        'exact_clips': sum(tokens(fixtures[row['id']]['expected']) == tokens(row['text']) for row in group),
        'median_ms': statistics.median(times), 'p95_ms': times[int(len(times) * .95) - 1],
        'cpu_seconds': sum(row.get('cpu_seconds', 0) for row in group),
    }
print(json.dumps(summary, indent=2))
