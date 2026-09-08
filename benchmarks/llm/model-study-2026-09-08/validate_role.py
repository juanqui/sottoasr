"""Fresh evaluation with the production-equivalent ten-second process deadline."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import selectors
import sys
import statistics
import subprocess
import time

EXP = Path(__file__).parent
ROOT = Path(__file__).resolve().parents[3]
p = argparse.ArgumentParser()
p.add_argument('--dataset', type=Path, required=True)
p.add_argument('--model', required=True)
p.add_argument('--output', type=Path, required=True)
p.add_argument('--config', type=Path, default=EXP/'binary-classifier-v1.json')
a = p.parse_args()
config = json.loads(a.config.read_text())
assert hashlib.sha256((EXP/'protocol/edits.rs').read_bytes()).hexdigest() == config['rust_guard_sha256']
environment = {**os.environ, 'HF_HUB_OFFLINE':'1', 'HF_HUB_DISABLE_IMPLICIT_TOKEN':'1', 'HF_HOME':str(EXP/'hf-home'), 'SOTTO_EXPERIMENT_CLASSIFIER_CONFIG':str(a.config)}
rust_proc = subprocess.Popen([os.environ.get('SOTTO_GUARD_ADAPTER',str(Path(__file__).with_name('guard-adapter')/'target/debug/cleanup_edits'))], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
def rust(request):
    rust_proc.stdin.write(json.dumps(request)+'\n'); rust_proc.stdin.flush()
    value = json.loads(rust_proc.stdout.readline())
    assert 'error' not in value, value
    return value
worker = None
errors = a.output.with_suffix('.stderr.log').open('w')
loads = []
def stop():
    global worker
    if worker is not None:
        worker.kill(); worker.wait(timeout=5); worker = None
def request(value, timeout):
    payload = (json.dumps(value)+'\n').encode()
    worker.stdin.write(payload); worker.stdin.flush()
    selector = selectors.DefaultSelector(); selector.register(worker.stdout, selectors.EVENT_READ)
    started = time.perf_counter(); data = b''
    try:
        while b'\n' not in data:
            remaining = timeout - (time.perf_counter()-started)
            if remaining <= 0 or not selector.select(remaining):
                stop(); raise TimeoutError(f'Resident classifier exceeded {timeout}s; process killed')
            chunk = os.read(worker.stdout.fileno(), 65536)
            if not chunk:
                stop(); raise RuntimeError('Classifier process exited')
            data += chunk
        return json.loads(data)
    finally:
        selector.close()
def load():
    global worker
    started = time.perf_counter()
    worker = subprocess.Popen([sys.executable, str(EXP/'role_worker.py'), a.model], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=errors, env=environment)
    response = request({'action':'load'}, 15)
    if not response['ok']:
        stop(); raise RuntimeError(response['error'])
    loads.append({'process_to_loaded_s':time.perf_counter()-started, **response})
def subseq(needle, haystack):
    it = iter(haystack)
    return all(any(c == h for h in it) for c in needle)
def stats(rows):
    needed = [r for r in rows if r['expected'] != r['raw']]
    preserve = [r for r in rows if r['expected'] == r['raw']]
    timed = [r['elapsed_s'] for r in rows if r['eligible']]
    return {'cases':len(rows), 'exact':sum(r['exact'] for r in rows), 'needed_cases':len(needed), 'needed_exact':sum(r['exact'] for r in needed), 'safe_useful_edits':sum(r['changed'] and not r['harmful'] for r in needed), 'preserve_cases':len(preserve), 'preserve_unchanged':sum(not r['changed'] for r in preserve), 'harmful_cases':sum(r['harmful'] for r in rows), 'fallbacks':sum(r['error'] is not None for r in rows), 'deadline_fallbacks':sum(r['error'] is not None and 'TimeoutError' in r['error'] for r in rows), 'median_s':statistics.median(timed) if timed else None, 'max_s':max(timed, default=0)}
rows = []
summary = {'dataset_sha256':hashlib.sha256(a.dataset.read_bytes()).hexdigest(), 'classifier_config_sha256':hashlib.sha256(a.config.read_bytes()).hexdigest(), 'guard_sha256':config['rust_guard_sha256'], 'runtime':config['runtime'], 'model':config['model_id'], 'revision':config['revision'], 'deadline_seconds':10}
def save():
    a.output.write_text(json.dumps({'summary':summary,'loads':loads,'results':rows},indent=2,ensure_ascii=False)+'\n')
try:
    load()
    for case in json.loads(a.dataset.read_text()):
        choices = rust({'text':case['raw']})['candidates'] if len(case['raw'].split()) >= 5 else []
        if choices and worker is None:
            load()
        started = time.perf_counter(); ids = []; error = None; response = None
        try:
            if choices:
                response = request({'action':'cleanup','text':case['raw'],'candidates':choices},10)
                if not response['ok']:
                    raise ValueError(response['error'])
                ids = response['delete_ids']
        except Exception as exc:
            error = f'{type(exc).__name__}: {exc}'
        elapsed = time.perf_counter()-started
        output = rust({'text':case['raw'],'delete_ids':ids})['text']
        row = {**case,'candidates':choices,'eligible':bool(choices),'selected_ids':ids,'output':output,'exact':output==case['expected'],'changed':output!=case['raw'],'harmful':not subseq(case['expected'],output),'elapsed_s':elapsed,'error':error,'worker_response':response}
        rows.append(row); save()
        print(json.dumps({k:row[k] for k in ['id','eligible','exact','harmful','elapsed_s','error']}),flush=True)
    summary['all']=stats(rows); summary['eligible']=stats([r for r in rows if r['eligible']]); save()
    print(json.dumps(summary),flush=True)
finally:
    stop(); errors.close(); rust_proc.stdin.close(); rust_proc.wait(timeout=5)
