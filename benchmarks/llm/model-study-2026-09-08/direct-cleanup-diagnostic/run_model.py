import argparse,difflib,hashlib,importlib.util,json,pathlib,re,resource,signal,statistics,time
p=argparse.ArgumentParser();p.add_argument('--model',required=True);p.add_argument('--label',required=True);a=p.parse_args()
exp=pathlib.Path(__file__).parent;config=json.loads((exp/'prompt.json').read_text());cases=json.loads((exp/'cases.json').read_text())
spec=importlib.util.spec_from_file_location('loader',exp/'loader.py');loader=importlib.util.module_from_spec(spec);spec.loader.exec_module(loader);loader.cached_model_path=lambda:pathlib.Path(a.model)
started=time.perf_counter();loader.load_model();load_s=time.perf_counter()-started
import mlx.core as mx
from mlx_lm import stream_generate
mx.set_memory_limit(4*1024**3)
def words(text):return re.findall(r"\w+(?:['’]\w+)*",text.casefold())
def subseq(needle,hay):
 it=iter(hay);return all(any(c==h for h in it) for c in needle)
def changes(raw,out):
 aa=raw.split();bb=out.split();return [{'op':op,'removed':aa[i:j],'added':bb[k:l]} for op,i,j,k,l in difflib.SequenceMatcher(None,aa,bb,autojunk=False).get_opcodes() if op!='equal']
def stop(signum,frame):raise TimeoutError('Direct editing case exceeded 10 seconds')
signal.signal(signal.SIGALRM,stop)
rows=[]
for mode in ['zero_shot','few_shot']:
 for case in cases:
  messages=[{'role':'system','content':config['system']}]
  if mode=='few_shot':
   for example in config['fewshot']:messages.extend([{'role':'user','content':example['raw']},{'role':'assistant','content':example['cleaned']}])
  messages.append({'role':'user','content':case['raw']})
  prompt=loader._tokenizer.apply_chat_template(messages,add_generation_prompt=True,tokenize=False,enable_thinking=False)
  budget=min(512,max(128,len(loader._tokenizer.encode(case['raw']))*2+32));start=time.perf_counter();cpu=time.process_time();pieces=[];last=None;error=None
  try:
   signal.alarm(10)
   for response in stream_generate(loader._model,loader._tokenizer,prompt=prompt,max_tokens=budget,sampler=loader._sampler):pieces.append(response.text);last=response
  except Exception as exc:error=f'{type(exc).__name__}: {exc}'
  finally:signal.alarm(0)
  output=''.join(pieces);normalized=output.strip();acceptable=case.get('acceptable_outputs',[case['expected']])
  row={'mode':mode,'id':case['id'],'raw_output':output,'finish_reason':getattr(last,'finish_reason',None),'error':error,'elapsed_s':time.perf_counter()-start,'cpu_s':time.process_time()-cpu,'generation_tokens':getattr(last,'generation_tokens',None),'prompt_tokens':getattr(last,'prompt_tokens',None),'budget':budget,'strict_exact':normalized==case['expected'],'accepted_sample':normalized in acceptable,'lexical_exact':any(words(normalized)==words(target) for target in acceptable),'introduced_words':not subseq(words(normalized),words(case['raw'])),'deleted_required_words':not any(subseq(words(target),words(normalized)) for target in acceptable),'changed':normalized!=case['raw'],'character_deletion_only':subseq(normalized,case['raw']),'word_changes':changes(case['raw'],normalized)}
  rows.append(row);mx.clear_cache();print(json.dumps({'label':a.label,**row},ensure_ascii=False),flush=True)
  (exp/(a.label+'.json')).write_text(json.dumps({'label':a.label,'load_s':load_s,'prompt_sha256':hashlib.sha256((exp/'prompt.json').read_bytes()).hexdigest(),'rows':rows},indent=2,ensure_ascii=False)+'\n')
summary={}
for mode in ['zero_shot','few_shot']:
 r=[v for v in rows if v['mode']==mode and v['id']!='reported_sentence'];summary[mode]={'exact_dev8':sum(v['strict_exact'] for v in r),'lexical_exact_dev8':sum(v['lexical_exact'] for v in r),'introduced_words_cases':sum(v['introduced_words'] for v in r),'deleted_required_words_cases':sum(v['deleted_required_words'] for v in r),'median_s':statistics.median(v['elapsed_s'] for v in r),'maximum_s':max(v['elapsed_s'] for v in r)}
summary.update(load_s=load_s,peak_metal_bytes=mx.get_peak_memory(),peak_rss_bytes=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss)
(exp/(a.label+'.json')).write_text(json.dumps({'label':a.label,'summary':summary,'prompt_sha256':hashlib.sha256((exp/'prompt.json').read_bytes()).hexdigest(),'rows':rows},indent=2,ensure_ascii=False)+'\n');print(json.dumps({'label':a.label,'summary':summary}),flush=True)
