#!/usr/bin/env python3
"""Fixed production sparse protocol, frozen independent synthetic holdout.
One model per process. Data never leaves this machine. No live cache writes.
"""
import argparse,hashlib,importlib.util,json,os,resource,signal,statistics,subprocess,sys,time
from pathlib import Path
from importlib.metadata import version
START=time.perf_counter(); CPU_START=time.process_time()
ROOT=Path(__file__).resolve().parents[3]; EXP=Path(__file__).resolve().parent
p=argparse.ArgumentParser(); p.add_argument('--model-path',type=Path,required=True); p.add_argument('--label',required=True); p.add_argument('--dataset',type=Path,default=EXP/'holdout-v2.json'); p.add_argument('--output',type=Path,required=True); p.add_argument('--idle-seconds',type=float,default=3); p.add_argument('--prompt-variant',choices=['production','balanced','binary'],default='production'); a=p.parse_args()
import psutil
selfproc=psutil.Process()
def mem():
 import mlx.core as mx
 return {'rss_bytes':selfproc.memory_info().rss,'peak_rss_bytes':resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,'metal_active_bytes':mx.get_active_memory(),'metal_cache_bytes':mx.get_cache_memory(),'metal_peak_bytes':mx.get_peak_memory()}
def subsequence(needle,haystack):
 it=iter(haystack); return all(any(c==h for h in it) for c in needle)
def stats(rows):
 eligible=[r for r in rows if r['eligible']]; needed=[r for r in rows if r['raw']!=r['expected']]; preserve=[r for r in rows if r['raw']==r['expected']]
 return {'cases':len(rows),'exact':sum(r['exact'] for r in rows),'needs_edits':len(needed),'needed_exact':sum(r['exact'] for r in needed),'needed_safe_useful':sum(r['changed'] and not r['harmful_deletion'] for r in needed),'preserve_cases':len(preserve),'preserve_unchanged':sum(not r['changed'] for r in preserve),'changed':sum(r['changed'] for r in rows),'harmful_cases':sum(r['harmful_deletion'] for r in rows),'fallbacks':sum(r['fallback_reason'] is not None for r in rows),'median_s':statistics.median(r['elapsed_s'] for r in eligible) if eligible else None,'p95_s':sorted(r['elapsed_s'] for r in eligible)[int(.95*(len(eligible)-1))] if eligible else None,'max_s':max((r['elapsed_s'] for r in eligible),default=0)}
spec=importlib.util.spec_from_file_location('production_cleanup',EXP/'protocol/llm_cleanup.py'); cleanup=importlib.util.module_from_spec(spec);spec.loader.exec_module(cleanup); cleanup.cached_model_path=lambda:a.model_path.resolve()
if a.prompt_variant == 'binary':
 import candidate_classifier
 candidate_classifier.install(cleanup)
elif a.prompt_variant != 'production':
 import prompt_dev
 prompt_dev.install(cleanup,a.prompt_variant)
proc=subprocess.Popen([str(EXP/'guard-adapter/target/debug/cleanup_edits')],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True)
def rust(request):
 proc.stdin.write(json.dumps(request)+'\n');proc.stdin.flush(); value=json.loads(proc.stdout.readline())
 if 'error' in value: raise ValueError(value['error'])
 return value
summary={'label':a.label,'prompt_variant':a.prompt_variant,'model_path':str(a.model_path),'dataset_sha256':hashlib.sha256(a.dataset.read_bytes()).hexdigest(),'protocol_sha256':hashlib.sha256((EXP/'protocol/llm_cleanup.py').read_bytes()).hexdigest(),'runtime':{x:version(x) for x in ['mlx','mlx-lm','transformers','huggingface-hub']},'python':sys.version.split()[0]}
results=[]
def save():
 a.output.write_text(json.dumps({'summary':summary,'results':results},indent=2,ensure_ascii=False)+'\n')
def alarm_handler(signum,frame): raise TimeoutError('Experiment case exceeded 30 seconds')
signal.signal(signal.SIGALRM,alarm_handler)
try:
 t=time.perf_counter(); c=time.process_time(); cleanup.load_model(); summary.update(load_s=time.perf_counter()-t,load_cpu_s=time.process_time()-c,process_to_loaded_s=time.perf_counter()-START,after_load=mem())
 import mlx.core as mx,mlx_lm
 capture={}; original_stream=mlx_lm.stream_generate
 def tracked_stream(*args,**kwargs):
  pieces=[]; last=None
  for response in original_stream(*args,**kwargs):
   pieces.append(response.text);last=response;yield response
  capture.update(raw_generation=''.join(pieces),generation_tokens=getattr(last,'generation_tokens',None),prompt_tokens=getattr(last,'prompt_tokens',None),prompt_tps=getattr(last,'prompt_tps',None),generation_tps=getattr(last,'generation_tps',None),finish_reason=getattr(last,'finish_reason',None))
 mlx_lm.stream_generate=tracked_stream
 first=True
 for case in json.loads(a.dataset.read_text()):
  raw=case['raw']; choices=rust({'text':raw})['candidates'] if len(raw.split())>=5 else []; capture.clear(); started=time.perf_counter();cpu=time.process_time();reason=None
  try:
   if choices:
    signal.alarm(30); ids,_=cleanup.select_deletions(raw,choices); signal.alarm(0);output=rust({'text':raw,'delete_ids':ids})['text']
   else: ids,output=[],raw
  except Exception as e:
   signal.alarm(0); reason=f'{type(e).__name__}: {e}'; ids,output=[],raw
  elapsed=time.perf_counter()-started
  row={**case,'candidates':choices,'eligible':bool(choices),'selected_ids':ids,'output':output,'exact':output==case['expected'],'changed':output!=raw,'harmful_deletion':not subsequence(case['expected'],output),'fallback_reason':reason,'elapsed_s':elapsed,'cpu_s':time.process_time()-cpu,**capture}
  results.append(row)
  if choices and first: summary['first_cleanup_s']=elapsed;summary['process_to_first_result_s']=time.perf_counter()-START;first=False
  print(json.dumps({k:row[k] for k in ['id','eligible','exact','harmful_deletion','fallback_reason','elapsed_s']}),flush=True)
  save()
 summary['all']=stats(results); summary['eligible']=stats([r for r in results if r['eligible']]);summary['warm_eligible']=stats([r for r in results if r['eligible']][1:]);summary['after_run']=mem(); summary['process_cpu_s']=time.process_time()-CPU_START;summary['measured_processing_wall_s']=time.perf_counter()-START
 t=time.perf_counter(); c=time.process_time();before=mem();time.sleep(a.idle_seconds);summary['idle']={'seconds':time.perf_counter()-t,'cpu_s':time.process_time()-c,'before':before,'after':mem()};save();print(json.dumps(summary),flush=True)
except Exception as e:
 summary['fatal_error']=f'{type(e).__name__}: {e}';save();print(json.dumps(summary),flush=True)
finally:
 proc.stdin.close();proc.wait(timeout=5)
