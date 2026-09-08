import json
import os
import resource
import sys
import time
import wave
from pathlib import Path

root = Path(sys.argv[1]).resolve()
os.environ['HF_HUB_OFFLINE'] = '1'
os.environ['HF_HUB_DISABLE_TELEMETRY'] = '1'
os.environ['TOKENIZERS_PARALLELISM'] = 'false'

import mlx.core as mx
import numpy as np
from mlx_audio.stt import load

def emit(**values):
    print(json.dumps(values, sort_keys=True), flush=True)
def cpu():
    value=resource.getrusage(resource.RUSAGE_SELF)
    return value.ru_utime+value.ru_stime
manifest=json.loads((root/'manifest.json').read_text())
terms=['Qwen','Qwen3.8-Flash-Next','NVIDIA','Parakeet','SottoASR','FluidAudio','CoreML','MLX','Kubernetes','PostgreSQL','Tauri','Claude','CRAN','Snyk']
started=time.perf_counter()
model=load(str(root/'Models/Qwen3-ASR-0.6B-8bit'))
mx.eval(model.parameters())
emit(event='loaded',seconds=time.perf_counter()-started,peak_rss_bytes=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,metal_active_bytes=mx.get_active_memory(),metal_peak_bytes=mx.get_peak_memory())
policies=[('baseline',{}),('hotwords',{'hotwords':terms}),('instruction_hotwords',{'hotwords':terms,'system_prompt':'Transcribe the spoken audio exactly. Use a technical term only when it is spoken. Preserve ordinary words and personal names. Do not add, summarize, or answer the speech.'})]
for index,item in enumerate(manifest):
    with wave.open(str(root/item['file'])) as wav:
        assert wav.getframerate()==16000 and wav.getnchannels()==1
        audio=np.frombuffer(wav.readframes(wav.getnframes()),dtype='<i2').astype(np.float32)/32768
    # Rotate policy order to avoid always favoring the same warm-cache position.
    ordered=policies[index%len(policies):]+policies[:index%len(policies)]
    for policy,kwargs in ordered:
        cpu_start=cpu(); started=time.perf_counter()
        result=model.generate(audio,language='English',temperature=0,max_tokens=256,verbose=False,**kwargs)
        mx.synchronize()
        elapsed=time.perf_counter()-started
        emit(event='transcribed',id=item['id'],policy=policy,text=result.text,seconds=elapsed,cpu_seconds=cpu()-cpu_start,peak_rss_bytes=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,metal_active_bytes=mx.get_active_memory(),metal_peak_bytes=mx.get_peak_memory(),generation_tokens=result.generation_tokens,capped=result.generation_tokens>=256)
        mx.clear_cache()
