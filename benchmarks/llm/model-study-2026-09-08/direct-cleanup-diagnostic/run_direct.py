"""One isolated model, direct transcript output, deterministic native templates."""
import argparse
import difflib
import hashlib
import importlib.metadata
import json
import pathlib
import re
import resource
import signal
import statistics
import time

parser = argparse.ArgumentParser()
parser.add_argument('--model', required=True)
parser.add_argument('--label', required=True)
parser.add_argument('--cases', default='cases.json')
parser.add_argument('--prompt', default='prompt.json')
parser.add_argument('--warmup-cases')
parser.add_argument('--output', required=True)
parser.add_argument('--modes', nargs='+', default=['zero_shot', 'few_shot'])
parser.add_argument('--native-thinking', action='store_true')
parser.add_argument('--spark', action='store_true')
parser.add_argument('--deadline', type=int, default=10)
parser.add_argument('--reasoning-budget', type=int, default=2048)
args = parser.parse_args()
base = pathlib.Path(__file__).resolve().parent
config = json.loads((base / args.prompt).read_text())
cases = json.loads((base / args.cases).read_text())
if args.warmup_cases:
    cases = [{**case, 'is_warmup': True} for case in json.loads((base / args.warmup_cases).read_text())] + cases
import mlx.core as mx
import psutil
from mlx_lm import load, stream_generate
from mlx_lm.sample_utils import make_sampler

if args.spark:
    from spark_mlx_llm.registration import register_model
    register_model()
mx.set_memory_limit(4 * 1024**3)
mx.set_cache_limit(128 * 1024**2)
process = psutil.Process()
start = time.perf_counter()
model, tokenizer = load(args.model, tokenizer_config={'local_files_only': True, 'trust_remote_code': False})
load_s = time.perf_counter() - start
sampler = make_sampler(temp=0.0)

def words(text):
    return re.findall(r"\w+(?:['’]\w+)*", text.casefold())

def subseq(needle, haystack):
    iterator = iter(haystack)
    return all(any(c == h for h in iterator) for c in needle)

def word_changes(raw, output):
    a, b = raw.split(), output.split()
    return [{'op': op, 'removed': a[i:j], 'added': b[k:l]}
            for op, i, j, k, l in difflib.SequenceMatcher(None, a, b, autojunk=False).get_opcodes()
            if op != 'equal']

def timeout(signum, frame):
    raise TimeoutError(f'Request exceeded {args.deadline} seconds')

signal.signal(signal.SIGALRM, timeout)
rows = []
metadata = {'label': args.label, 'load_s': load_s,
            'loaded_model_class': type(model).__module__ + '.' + type(model).__qualname__,
            'runtime': {name: importlib.metadata.version(name) for name in ['mlx', 'mlx-lm', 'transformers', 'huggingface-hub', 'psutil']},
            'prompt_sha256': hashlib.sha256((base / args.prompt).read_bytes()).hexdigest(),
            'cases_sha256': hashlib.sha256((base / args.cases).read_bytes()).hexdigest(),
            'warmup_cases_sha256': hashlib.sha256((base / args.warmup_cases).read_bytes()).hexdigest() if args.warmup_cases else None,
            'native_thinking': args.native_thinking, 'temperature': 0.0,
            'deadline_s': args.deadline, 'reasoning_budget': args.reasoning_budget if args.native_thinking else None,
            'budget_formula': 'native reasoning budget' if args.native_thinking else 'min(8192,max(128,2*input_tokens+32))',
            'memory_limit_bytes': 4 * 1024**3, 'cache_limit_bytes': 128 * 1024**2}
for mode in args.modes:
    for case in cases:
        mx.reset_peak_memory()
        rss_before = process.memory_info().rss
        started = time.perf_counter()
        cpu_started = time.process_time()
        pieces, last, error = [], None, None
        budget = None
        generation_s = None
        try:
            signal.alarm(args.deadline)
            messages = [{'role': 'system', 'content': config['system']}]
            def user_data(raw):
                return config.get('user_prefix', '') + raw + config.get('user_suffix', '')
            if mode == 'few_shot':
                for example in config['fewshot']:
                    messages.extend([{'role': 'user', 'content': user_data(example['raw'])},
                                     {'role': 'assistant', 'content': example['cleaned']}])
            messages.append({'role': 'user', 'content': user_data(case['raw'])})
            template_args = {} if args.native_thinking else {'enable_thinking': False}
            prompt = tokenizer.apply_chat_template(messages, add_generation_prompt=True,
                                                   tokenize=False, **template_args)
            budget = args.reasoning_budget if args.native_thinking else min(8192, max(128, 2 * len(tokenizer.encode(case['raw'])) + 32))
            generation_started = time.perf_counter()
            for response in stream_generate(model, tokenizer, prompt=prompt, max_tokens=budget, sampler=sampler):
                pieces.append(response.text)
                last = response
            generation_s = time.perf_counter() - generation_started
        except Exception as exc:
            error = f'{type(exc).__name__}: {exc}'
        finally:
            signal.alarm(0)
            mx.clear_cache()
        elapsed = time.perf_counter() - started
        cpu_s = time.process_time() - cpu_started
        output = ''.join(pieces)
        reasoning, separator, final = output.partition('</think>') if args.native_thinking else ('', '', output)
        if args.native_thinking and not separator:
            final = ''
            error = error or 'Native reasoning did not produce its final-answer delimiter'
        normalized = final.strip()
        acceptable = case.get('acceptable_outputs', [case['expected']])
        completed = error is None and getattr(last, 'finish_reason', None) == 'stop'
        row = {'mode': mode, 'id': case['id'], 'is_warmup': case.get('is_warmup', False), 'raw_output': output, 'final_output': final,
               'completed': completed,
               'estimated_reasoning_tokens': len(tokenizer.encode(reasoning)) if args.native_thinking else 0,
               'finish_reason': getattr(last, 'finish_reason', None), 'error': error,
               'request_elapsed_s': elapsed, 'generation_elapsed_s': generation_s, 'cpu_s': cpu_s,
               'rss_before_bytes': rss_before, 'rss_after_bytes': process.memory_info().rss,
               'peak_rss_process_bytes': resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
               'peak_metal_request_bytes': mx.get_peak_memory(), 'active_metal_after_bytes': mx.get_active_memory(),
               'generation_tokens': getattr(last, 'generation_tokens', None),
               'prompt_tokens': getattr(last, 'prompt_tokens', None), 'budget': budget,
               'strict_exact': completed and normalized == case['expected'], 'accepted_sample': completed and normalized in acceptable,
               'lexical_exact': completed and any(words(normalized) == words(target) for target in acceptable),
               'introduced_words': not subseq(words(normalized), words(case['raw'])),
               'deleted_required_words': not any(subseq(words(target), words(normalized)) for target in acceptable),
               'changed': normalized != case['raw'], 'word_changes': word_changes(case['raw'], normalized)}
        rows.append(row)
        pathlib.Path(args.output).write_text(json.dumps({**metadata, 'rows': rows}, indent=2, ensure_ascii=False) + '\n')
        print(json.dumps({'label': args.label, **row}, ensure_ascii=False), flush=True)
summary = {}
for mode in args.modes:
    values = [v for v in rows if v['mode'] == mode and v['id'] != 'reported_sentence']
    if not values:
        continue
    summary[mode] = {'cases': len(values), 'exact': sum(v['strict_exact'] for v in values),
                     'lexical_exact': sum(v['lexical_exact'] for v in values),
                     'introduced_words_cases': sum(v['completed'] and v['introduced_words'] for v in values),
                     'deleted_required_words_cases': sum(v['completed'] and v['deleted_required_words'] for v in values),
                     'errors': sum(v['error'] is not None for v in values),
                     'unfinished': sum(v['finish_reason'] != 'stop' for v in values),
                     'median_request_s': statistics.median(v['request_elapsed_s'] for v in values),
                     'maximum_request_s': max(v['request_elapsed_s'] for v in values)}
pathlib.Path(args.output).write_text(json.dumps({**metadata, 'summary': summary, 'rows': rows}, indent=2, ensure_ascii=False) + '\n')
print(json.dumps({'label': args.label, 'summary': summary}), flush=True)
