"""Run supported CoreML modes serially against isolated clones of one model."""
import argparse
import os
import shutil
import signal
import subprocess
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('root', type=Path, help='Prepared corpus, cloned Models, and compiled compute_probe')
args = parser.parse_args()
root = args.root.resolve()
for mode in ['ane', 'all', 'gpu_encoder', 'cpu']:
    isolated = root / ('mode-' + mode)
    isolated.mkdir(exist_ok=False)
    shutil.copyfile(root / 'manifest.json', isolated / 'manifest.json')
    for wav in root.glob('*.wav'):
        subprocess.run(['cp', '-c', str(wav), str(isolated / wav.name)], check=True)
    subprocess.run(['cp', '-cR', str(root / 'Models'), str(isolated / 'Models')], check=True)
    with (isolated / 'results.log').open('x') as log:
        child = subprocess.Popen(['/usr/bin/time', '-l', str(root / 'compute_probe'), str(isolated), mode],
                                 stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            code = child.wait(timeout=180)
            print(f'{mode}: exit={code}; log={isolated / "results.log"}', flush=True)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            child.wait()
            print(f'{mode}: exceeded 180 seconds; owned process group terminated', flush=True)
