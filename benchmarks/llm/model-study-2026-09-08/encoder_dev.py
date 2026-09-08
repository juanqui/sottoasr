"""Public ModernBERT ONNX diagnostic on separate synthetic development data only."""
import argparse,json,os,pathlib,resource,statistics,subprocess,time
p=argparse.ArgumentParser();p.add_argument('--model',type=pathlib.Path,required=True);p.add_argument('--output',type=pathlib.Path,required=True);a=p.parse_args()
exp=pathlib.Path(__file__).parent
import numpy as np
import onnxruntime as ort
from tokenizers import Tokenizer
options=ort.SessionOptions();options.intra_op_num_threads=1;options.inter_op_num_threads=1
started=time.perf_counter();session=ort.InferenceSession(str(a.model/'DisfluencyClassifier.onnx'),options,providers=['CPUExecutionProvider']);load_s=time.perf_counter()-started
names={x.name for x in session.get_inputs()};labels=json.loads((a.model/'label_map.json').read_text());tokenizer=Tokenizer.from_file(str(a.model/'tokenizer.json'))
proc=subprocess.Popen([os.environ.get('SOTTO_GUARD_ADAPTER',str(exp/'guard-adapter/target/debug/cleanup_edits'))],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True)
def rust(value):
 proc.stdin.write(json.dumps(value)+'\n');proc.stdin.flush();d=json.loads(proc.stdout.readline());assert 'error' not in d,d;return d
def subseq(needle,hay):
 it=iter(hay);return all(any(c==h for h in it) for c in needle)
rows=[]
for case in json.loads((exp/'development-bench-v2.json').read_text()):
 raw=case['raw'];candidates=rust({'text':raw})['candidates'] if len(raw.split())>=5 else [];start=time.perf_counter();cpu=time.process_time();selected=[];judgments=[]
 if candidates:
  encoding=tokenizer.encode(raw,add_special_tokens=True)
  inputs={'input_ids':np.asarray([encoding.ids],dtype=np.int64),'attention_mask':np.asarray([encoding.attention_mask],dtype=np.int64)}
  logits=session.run(None,{k:v for k,v in inputs.items() if k in names})[0]
  predicted=np.argmax(logits[0],axis=-1)
  for candidate in candidates:
   context=rust({'text':raw,'context_id':candidate['id']});begin=len(context['before']);end=begin+len(candidate['text'])
   indices=[i for i,(x,y) in enumerate(encoding.offsets) if x<end and y>begin]
   tags=[labels[str(int(predicted[i]))] for i in indices]
   wanted='FP' if candidate['kind']=='hesitation' else 'RP'
   delete=bool(tags) and all(tag==wanted for tag in tags)
   if delete:selected.append(candidate['id'])
   judgments.append({'id':candidate['id'],'tokens':[encoding.tokens[i] for i in indices],'offsets':[encoding.offsets[i] for i in indices],'labels':tags,'selected':delete})
 output=rust({'text':raw,'delete_ids':selected})['text']
 row={**case,'eligible':bool(candidates),'selected_ids':selected,'output':output,'exact':output==case['expected'],'harmful':not subseq(case['expected'],output),'changed':output!=raw,'elapsed_s':time.perf_counter()-start,'cpu_s':time.process_time()-cpu,'judgments':judgments};rows.append(row);print(json.dumps(row,ensure_ascii=False),flush=True)
summary={'cases':len(rows),'eligible':sum(x['eligible'] for x in rows),'exact':sum(x['exact'] for x in rows),'harmful':sum(x['harmful'] for x in rows),'safe_useful_cases':sum(x['raw']!=x['expected'] and x['changed'] and not x['harmful'] for x in rows),'load_s':load_s,'median_eligible_s':statistics.median(x['elapsed_s'] for x in rows if x['eligible']),'max_s':max(x['elapsed_s'] for x in rows),'cpu_s':sum(x['cpu_s'] for x in rows),'peak_rss_bytes':resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,'runtime':ort.__version__,'providers':session.get_providers(),'input_names':sorted(names),'model':'sam-castro/mow-disfluency-classifier','revision':'e1f59b45e03988dd55b8ff307c602f0d4567bf8c','license_caveat':'Converter declares CC BY4; original fine-tune license/provenance unresolved. Research diagnostic only.'}
a.output.write_text(json.dumps({'summary':summary,'results':rows},indent=2,ensure_ascii=False)+'\n');print(json.dumps(summary),flush=True);proc.stdin.close();proc.wait(timeout=5)
