"""Measure the official Granite Speech 5.0 TurboCTC artifact through native MLX."""
import json
import os
import resource
import sys
import time
import wave
from pathlib import Path

os.environ['HF_HUB_OFFLINE'] = '1'
os.environ['HF_HUB_DISABLE_TELEMETRY'] = '1'
os.environ['TOKENIZERS_PARALLELISM'] = 'false'

import mlx.core as mx
import numpy as np
from mlx_audio.stt import load

root = Path(sys.argv[1]).resolve()
manifest = json.loads((root / 'manifest.json').read_text())

def emit(**values):
    print(json.dumps(values, sort_keys=True), flush=True)

def cpu():
    value = resource.getrusage(resource.RUSAGE_SELF)
    return value.ru_utime + value.ru_stime

def audio(item):
    with wave.open(str(root / item['file'])) as wav:
        assert wav.getframerate() == 16000 and wav.getnchannels() == 1
        return np.frombuffer(wav.readframes(wav.getnframes()), dtype='<i2').astype(np.float32) / 32768

started = time.perf_counter()
model = load(str(root / 'Models/granite-speech-5.0-470m-turboctc'))
mx.eval(model.parameters())
emit(event='loaded', seconds=time.perf_counter()-started,
     peak_rss_bytes=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
     metal_active_bytes=mx.get_active_memory(), metal_peak_bytes=mx.get_peak_memory())
model.generate(audio(manifest[0]), verbose=False)
mx.synchronize()
for item in manifest:
    samples = audio(item)
    started, cpu_start = time.perf_counter(), cpu()
    result = model.generate(samples, verbose=False)
    mx.synchronize()
    emit(event='transcribed', id=item['id'], text=result.text,
         seconds=time.perf_counter()-started, cpu_seconds=cpu()-cpu_start,
         peak_rss_bytes=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
         metal_active_bytes=mx.get_active_memory(), metal_peak_bytes=mx.get_peak_memory(),
         generation_tokens=result.generation_tokens)
    mx.clear_cache()
