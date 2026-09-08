"""Synthesize the frozen development corpus into a new experiment directory."""
import argparse
import json
import subprocess
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('output', type=Path)
parser.add_argument('--independent', action='store_true', help='Use the frozen 24-sentence holdout in two voices')
args = parser.parse_args()
root = args.output.resolve()
root.mkdir(parents=True, exist_ok=False)
fixtures = json.loads(Path(__file__).with_name('development-40.json').read_text())
if args.independent:
    holdout = json.loads(Path(__file__).with_name('independent-24.json').read_text())
    fixtures = []
    for voice in ['Samantha', 'Daniel']:
        for item in holdout['cases']:
            identifier = voice.lower() + '_' + item['id']
            expected = item['expected'].replace('eight dollars', '$8').replace('three point eight', '3.8').replace('seventy two', '72')
            fixtures.append({**item, 'id': identifier, 'file': identifier + '.wav', 'voice': voice,
                             'expected': expected, 'negative': item['kind'] == 'ordinary'})
    (root / 'vocabulary.json').write_text(json.dumps(holdout['vocabulary'], indent=2) + '\n')
for item in fixtures:
    subprocess.run(['say', '-v', item['voice'], '-r', '175', '-o', str(root / item['file']),
                    '--data-format=LEI16@16000', item['spoken']], check=True, capture_output=True)
(root / 'manifest.json').write_text(json.dumps(fixtures, indent=2) + '\n')
print(f'Prepared {len(fixtures)} synthetic clips in {root}')
