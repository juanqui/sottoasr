import importlib.util
import json
from pathlib import Path
import sys
import time

import psutil

spec = importlib.util.spec_from_file_location('cleanup', Path(__file__).with_name('protocol')/'llm_cleanup.py')
cleanup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cleanup)
cleanup.cached_model_path = lambda: Path(sys.argv[1])
import candidate_classifier
candidate_classifier.install(cleanup)

def memory():
    import mlx.core as mx
    return {'rss_bytes': psutil.Process().memory_info().rss, 'metal_active_bytes': mx.get_active_memory(), 'metal_cache_bytes': mx.get_cache_memory(), 'metal_peak_bytes': mx.get_peak_memory()}

for line in sys.stdin:
    started = time.perf_counter()
    cpu = time.process_time()
    try:
        request = json.loads(line)
        if request['action'] == 'load':
            cleanup.load_model()
            result = {}
        else:
            ids, _ = cleanup.select_deletions(request['text'], request['candidates'])
            result = {'delete_ids': ids}
        response = {'ok': True, **result}
    except Exception as error:
        response = {'ok': False, 'error': f'{type(error).__name__}: {error}'}
    print(json.dumps({**response, 'elapsed_s': time.perf_counter() - started, 'cpu_s': time.process_time() - cpu, 'memory': memory()}), flush=True)
