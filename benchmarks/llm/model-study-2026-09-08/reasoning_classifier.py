"""Development-only native-thinking classifier; exact spans come from Rust."""
import json, os, re, subprocess, time
from pathlib import Path
CONFIG=json.loads(Path(os.environ['SOTTO_EXPERIMENT_CLASSIFIER_CONFIG']).read_text())
ADAPTER=os.environ.get('SOTTO_GUARD_ADAPTER',str(Path(__file__).with_name('guard-adapter')/'target/debug/cleanup_edits'))

def rust(value):
    p=subprocess.run([ADAPTER],input=json.dumps(value)+'\n',text=True,capture_output=True,timeout=1,check=True)
    result=json.loads(p.stdout)
    if 'error' in result: raise ValueError(result['error'])
    return result

def content(raw, cid):
    c=rust({'text':raw,'context_id':cid})
    return json.dumps({'before':c['before'],'marked':c['span'],'after':c['after']},ensure_ascii=False)

def install(cleanup):
    base=[{'role':'system','content':CONFIG['system']}]
    for (raw,edited,label),reason in zip(CONFIG['examples'],CONFIG['reasons']):
        cid=next(c['id'] for c in rust({'text':raw})['candidates'] if rust({'text':raw,'delete_ids':[c['id']]})['text']==edited)
        base.extend([{'role':'user','content':content(raw,cid)},{'role':'assistant','content':f'Reason: {reason}\nDecision: '+('DELETE' if label=='1' else 'KEEP')}])
    def select(text,candidates):
        cleanup.validate_candidates(candidates)
        if not isinstance(text,str) or len(text)>CONFIG['input_char_limit']: raise ValueError('Input exceeds cleanup limit')
        cleanup.load_model()
        import mlx.core as mx
        from mlx_lm import stream_generate
        if CONFIG.get('metal_guideline_bytes'):
            mx.set_memory_limit(CONFIG['metal_guideline_bytes'])
        start=time.perf_counter(); selected=[]; outputs=[]
        try:
            for choice in candidates:
                prompt=cleanup._tokenizer.apply_chat_template(base+[{'role':'user','content':content(text,choice['id'])}],add_generation_prompt=True,tokenize=False,enable_thinking=True)
                pieces=[];last=None
                for result in stream_generate(cleanup._model,cleanup._tokenizer,prompt=prompt,max_tokens=512,sampler=cleanup._sampler):
                    pieces.append(result.text);last=result
                raw=''.join(pieces).strip()
                if '</think>' not in raw: raise ValueError('Missing complete native reasoning')
                final=raw.split('</think>',1)[1].strip()
                match=re.fullmatch(r'Reason: [^\n]+\nDecision: (KEEP|DELETE)',final)
                outputs.append({'id':choice['id'],'raw_generation':raw,'generation_tokens':last.generation_tokens})
                if not match or last.finish_reason!='stop': raise ValueError('Incomplete decision')
                if match[1]=='DELETE': selected.append(choice['id'])
                if time.perf_counter()-start>=10: raise TimeoutError('Complete request exceeded10s')
                mx.clear_cache()
            cleanup._experiment_outputs=outputs
            return selected,int((time.perf_counter()-start)*1000)
        finally: mx.clear_cache()
    cleanup.select_deletions=select
