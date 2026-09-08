#!/usr/bin/env python3
"""Isolated D3/MiniCPM static-prefix cache equivalence and timing experiment.

No MLX import or model work without --execute. --self-test is stdlib only.
Run only after qualification and an exclusive hardware handoff. This driver
never writes a prompt cache, reads app data, or changes any model/cache files.
"""

import argparse
import copy
import hashlib
import importlib.metadata
import json
import resource
import signal
import statistics
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
MODEL_ID = 'openbmb/MiniCPM5-2B-MLX'
REVISION = '32f8dd5df1188512a20413f1297083238306634c'
WEIGHT_SHA = 'c207798696a4a454e7ac211b25227625466c693335941cee8904fb922f295cc1'
PROMPT_SHA = 'ffe72a3812d42fb9f455e428f62ff300d232ace46acdf668e6d012425e7d48e5'
QUALIFICATION_RUNNER_SHA = '31713c2262119e4a65dfa9279aa097dd7f1de7bb7e808dd10c24ab319e4fa0f6'
FILE_HASHES = {
    'config.json': 'deb9ca33e863cbc84a9ab7209cd924fc05505dc33270c807d47f1b78fbd53a50',
    'tokenizer.json': '3e065a558a034185fe299917b398685c1facd0169a9eea1e629eb30c171fed81',
    'tokenizer_config.json': 'b89503c3e5070c6b6d33daf2e20cb4a5c88537c1670d9b7e0cfb4506a61448a9',
    'chat_template.jinja': 'cc945752db555d60949b16989df4ccfeb52a313d6b4b5c5229dd786e2e9fcf1c',
    'generation_config.json': '9ac4f32e5f32358697a9f438a3ea89ef80e6ba786c72c49e932f9f21c122fdb1',
    'model.safetensors.index.json': 'ccf202e0a06fe3c7eb8f354cfb29412a5e64956ad895413d4d9267ae4b3a6045',
}
RUNTIME_PINS = {'mlx': '0.32.2', 'mlx-lm': '0.31.3', 'transformers': '5.3.0',
                'huggingface-hub': '1.7.2', 'psutil': '7.2.2'}
MARKER = '__SOTTO_STATIC_BOUNDARY_7BDAEE31__'
CASES = [
    {'id': 'A_first', 'raw': 'We um need the the violet folder by Thursday.'},
    {'id': 'B_between', 'raw': 'Quinn confirmed that the backup must not run before 18:30.'},
    {'id': 'A_after_B', 'raw': 'We um need the the violet folder by Thursday.'},
    {'id': 'leading_newline', 'raw': '\n\nUm, please keep the terminal clause intact.'},
    {'id': 'leading_spaces', 'raw': '  Uh the sensor reports 3.75 volts.'},
    {'id': 'leading_quote', 'raw': '"um" is the exact label in this example.'},
    {'id': 'combining_unicode', 'raw': 'Cafe\u0301 is um the spelling in the imported note.'},
    {'id': 'non_bmp', 'raw': '🧭 The marker is uh next to the second entrance.'},
    {'id': 'leading_punctuation', 'raw': '(Actually, that number is correct.) Keep both parentheses.'},
    {'id': 'about_40_words', 'raw': 'Please review the local backup schedule before Friday. We um need the archive to remain on this computer, and the second copy should start only after the first verification finishes. Keep the existing filenames and report any missing files.'},
    {'id': 'about_150_words', 'raw': 'I checked the recording workflow this morning and found that the final sentence was present in the original audio. The export should keep that sentence and preserve the exact names of the two microphones. We um need the first comparison to use the same recording on both machines, with every background download paused. Please write down the elapsed time, the model version, and whether the computer was connected to power. Do not change the input gain between runs. After the comparison, leave the original recordings in their current folder and save the measurements beside the synthetic test notes. The the second experiment can use a longer passage, but its wording must remain fixed across all attempts. If an operation fails, record the failure instead of replacing it with a successful retry. This will give us a useful reference when we evaluate the next software build tomorrow.'},
]


def sha(path):
    digest = hashlib.sha256()
    with path.open('rb') as source:
        for block in iter(lambda: source.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def build_messages(config, raw):
    """Exact selected run_inline.py few_shot/system_inline construction."""
    assert config['example_format'] == 'system_inline'
    def user_data(text):
        return config.get('user_prefix', '') + text + config.get('user_suffix', '')
    examples = ['\n\n<examples>']
    for example in config['fewshot']:
        examples.append('<example>\nInput:\n' + user_data(example['raw'])
                        + '\nOutput:\n' + example['cleaned'] + '\n</example>')
    examples.append('</examples>')
    return [{'role': 'system', 'content': config['system'] + '\n'.join(examples)},
            {'role': 'user', 'content': user_data(raw)}]


def encode_prompt(tokenizer, prompt):
    # Matches qualification/run_qualification.py and mlx-lm0.31.3 string handling.
    add_special = tokenizer.bos_token is None or not prompt.startswith(tokenizer.bos_token)
    return list(tokenizer.encode(prompt, add_special_tokens=add_special))


def prefix_matches(prefix, full):
    return bool(prefix) and len(full) > len(prefix) and full[:len(prefix)] == prefix


def equivalent(a, b):
    return (a['completed'] and b['completed']
            and a['token_ids'] == b['token_ids'] and a['text'] == b['text']
            and a['finish_reason'] == b['finish_reason'])


def self_test():
    config = {'system': 'S', 'example_format': 'system_inline',
              'fewshot': [{'raw': 'A', 'cleaned': 'B'}], 'user_prefix': '<t>', 'user_suffix': '</t>'}
    assert build_messages(config, 'C') == [
        {'role': 'system', 'content': 'S\n\n<examples>\n<example>\nInput:\n<t>A</t>\nOutput:\nB\n</example>\n</examples>'},
        {'role': 'user', 'content': '<t>C</t>'}]
    assert prefix_matches([1, 2], [1, 2, 3])
    assert not prefix_matches([1, 2], [1, 3, 4])
    assert not prefix_matches([1, 2], [1, 2])
    good = {'completed': True, 'token_ids': [2, 1], 'text': 'x', 'finish_reason': 'stop'}
    assert equivalent(good, dict(good))
    assert not equivalent(good, {**good, 'token_ids': [2, 130073]})
    assert not equivalent(good, {**good, 'completed': False})
    class FakeTokenizer:
        bos_token = '<s>'
        def encode(self, prompt, add_special_tokens):
            return [int(add_special_tokens), len(prompt)]
    assert encode_prompt(FakeTokenizer(), '<s>x') == [0, 4]
    assert encode_prompt(FakeTokenizer(), 'x') == [1, 1]
    assert 35 <= len(CASES[-2]['raw'].split()) <= 45
    assert 140 <= len(CASES[-1]['raw'].split()) <= 160
    assert not any(name == 'mlx' or name.startswith('mlx.') for name in sys.modules)
    print('Pure layout, prefix, EOS equality and no-MLX-import checks passed.')


def execute(args):
    if sha(args.prompt) != PROMPT_SHA:
        raise ValueError('D3 prompt hash mismatch')
    for name, expected in FILE_HASHES.items():
        if sha(args.model / name) != expected:
            raise ValueError(f'Pinned artifact hash mismatch: {name}')
    if (args.model / 'model.safetensors').stat().st_size != 1_416_035_216:
        raise ValueError('Pinned weight size mismatch')
    if args.verify_weights and sha(args.model / 'model.safetensors') != WEIGHT_SHA:
        raise ValueError('Pinned weight hash mismatch')
    versions = {name: importlib.metadata.version(name) for name in RUNTIME_PINS}
    if versions != RUNTIME_PINS:
        raise ValueError(f'Runtime does not match qualification: {versions}')
    config = json.loads(args.prompt.read_text())
    context_limit = json.loads((args.model / 'config.json').read_text())['max_position_embeddings']

    # The only MLX/runtime imports are below the explicit --execute boundary.
    import mlx.core as mx
    import numpy as np
    import psutil
    from mlx_lm import load, stream_generate
    from mlx_lm.generate import generate_step
    from mlx_lm.models.cache import make_prompt_cache
    from mlx_lm.sample_utils import make_sampler, make_logits_processors

    mx.set_memory_limit(4 * 1024**3)
    mx.set_cache_limit(128 * 1024**2)
    process = psutil.Process()
    clock = lambda: (time.perf_counter(), time.process_time())
    def measurement(started):
        return {'wall_s': time.perf_counter() - started[0], 'cpu_s': time.process_time() - started[1],
                'rss_after_bytes': process.memory_info().rss,
                'process_peak_rss_bytes': resource.getrusage(resource.RUSAGE_SELF).ru_maxrss,
                'active_metal_bytes': mx.get_active_memory(), 'peak_metal_bytes': mx.get_peak_memory()}

    started = clock()
    load_rss = process.memory_info().rss
    model, tokenizer = load(str(args.model), tokenizer_config={'local_files_only': True, 'trust_remote_code': False})
    mx.synchronize()
    load_metrics = {'rss_before_bytes': load_rss, **measurement(started)}
    assert set(tokenizer.eos_token_ids) == {1, 130073}
    sampler = make_sampler(temp=0.0, top_k=0)
    processors = make_logits_processors(repetition_penalty=None, repetition_context_size=20)

    def render(raw):
        return tokenizer.apply_chat_template(build_messages(config, raw), add_generation_prompt=True,
                                             tokenize=False, enable_thinking=False)

    # No benchmark transcript contributes to this persistent baseline.
    static_render = render(MARKER)
    assert static_render.count(MARKER) == 1
    static_text = static_render.split(MARKER)[0]
    static_ids = encode_prompt(tokenizer, static_text)
    # The final separately encoded token can merge across a BPE boundary.
    # Omit it using static data only; still verify each full request's token IDs.
    prefix_ids = static_ids[:-1]
    assert prefix_ids

    def fingerprint(cache):
        """Full numeric KV-state hash and offsets, outside request timing.

        Float16/BFloat16->float32 is exact; dtype/shape are also hashed. Host
        conversion avoids a mutable on-device reference masquerading as a copy.
        Include unused allocated capacity: a shared tail must not silently pick
        up private request values even if the baseline offset stays unchanged.
        """
        digest = hashlib.sha256()
        offsets = []
        for layer in cache:
            assert type(layer).__name__ == 'KVCache', 'This driver is pinned to MiniCPM/Llama KVCache'
            offsets.append(layer.offset)
            digest.update(str((type(layer).__name__, layer.offset, layer.meta_state)).encode())
            for value in (layer.keys, layer.values):
                digest.update(str((value.shape, value.dtype)).encode())
                host = np.asarray(value.astype(mx.float32))
                if not np.isfinite(host).all():
                    raise AssertionError('Non-finite static cache state')
                digest.update(host.tobytes(order='C'))
        return {'sha256': digest.hexdigest(), 'offsets': offsets}

    def timeout(_signal, _frame):
        raise TimeoutError(f'Request exceeded {args.deadline} seconds')

    previous_alarm = signal.signal(signal.SIGALRM, timeout)
    baseline = None
    try:
        mx.reset_peak_memory()
        started = clock()
        cache_rss = process.memory_info().rss
        baseline = make_prompt_cache(model)
        signal.setitimer(signal.ITIMER_REAL, args.deadline)
        prefill = generate_step(mx.array(prefix_ids), model, max_tokens=0, prompt_cache=baseline)
        try:
            for _ in prefill:
                pass
            mx.eval([layer.state for layer in baseline])
            mx.synchronize()
        finally:
            signal.setitimer(signal.ITIMER_REAL, 0)
            prefill.close()
        build_metrics = {'rss_before_bytes': cache_rss, **measurement(started)}
        assert all(layer.offset == len(prefix_ids) for layer in baseline), 'max_tokens=0 prefill offset mismatch'
        guard = fingerprint(baseline)

        def request(case, cached, fault=None):
            assert fingerprint(baseline) == guard, 'Baseline changed before request'
            mx.reset_peak_memory()
            rss_before = process.memory_info().rss
            started = clock()
            generator = request_cache = response = None
            tokens, pieces = [], []
            error = last = None
            clone_s = first_s = generation_s = None
            prompt_count = suffix_count = budget = None
            boundary_ok = False
            mx.random.seed(42)
            try:
                signal.setitimer(signal.ITIMER_REAL, args.deadline)
                prompt_ids = encode_prompt(tokenizer, render(case['raw']))
                prompt_count = len(prompt_ids)
                budget = min(8192, max(128, 2 * len(tokenizer.encode(case['raw'])) + 32))
                if prompt_count + budget > context_limit:
                    raise ValueError('Formatted prompt plus reserved generation exceeds model context')
                boundary_ok = prefix_matches(prefix_ids, prompt_ids)
                input_ids = prompt_ids
                if cached and boundary_ok:
                    clone_started = time.perf_counter()
                    request_cache = copy.deepcopy(baseline)
                    mx.eval([layer.state for layer in request_cache])
                    mx.synchronize()
                    clone_s = time.perf_counter() - clone_started
                    assert all(layer.offset == len(prefix_ids) for layer in request_cache)
                    assert all(a is not b for a, b in zip(baseline, request_cache))
                    input_ids = prompt_ids[len(prefix_ids):]
                suffix_count = len(input_ids)
                generation_started = time.perf_counter()
                generator = stream_generate(model, tokenizer, prompt=input_ids, max_tokens=budget,
                                            sampler=sampler, logits_processors=processors,
                                            prompt_cache=request_cache)
                for response in generator:
                    if first_s is None:
                        first_s = time.perf_counter() - started[0]
                    tokens.append(int(response.token))  # Includes the final EOS.
                    pieces.append(response.text)        # Includes final detokenizer tail.
                    last = {key: getattr(response, key) for key in
                            ['finish_reason', 'prompt_tokens', 'prompt_tps', 'generation_tokens', 'generation_tps']}
                    if fault == 'cancel':
                        raise RuntimeError('Injected cancellation after first generated token')
                    if fault == 'timeout':
                        raise TimeoutError('Injected timeout after first generated token')
                generation_s = time.perf_counter() - generation_started
                assert last and last['prompt_tokens'] == suffix_count, 'Explicit prompt-ID count mismatch'
                assert last['generation_tokens'] == len(tokens), 'Generated token capture omitted or duplicated a token'
            except Exception as exc:
                error = f'{type(exc).__name__}: {exc}'
            finally:
                signal.setitimer(signal.ITIMER_REAL, 0)
                if generator is not None:
                    try:
                        generator.close()
                    except Exception as exc:
                        error = error or f'Generator close failed: {type(exc).__name__}'
                generator = request_cache = response = None
                mx.clear_cache()
                mx.synchronize()
            metrics = measurement(started)
            if metrics['wall_s'] > args.deadline:
                error = error or f'Request exceeded {args.deadline} seconds'
            validation_started = time.perf_counter()
            intact = fingerprint(baseline) == guard
            assert intact, 'Static baseline mutated by a request'
            last = last or {}
            return {'id': case['id'], 'cached_requested': cached,
                    'cache_used': cached and boundary_ok, 'prefix_boundary_verified': boundary_ok,
                    'fault': fault, 'completed': error is None and last.get('finish_reason') == 'stop',
                    'error': error, 'text': ''.join(pieces), 'token_ids': tokens,
                    'finish_reason': last.get('finish_reason'), 'budget': budget,
                    'full_prompt_tokens': prompt_count, 'cached_prefix_tokens': len(prefix_ids) if cached and boundary_ok else 0,
                    'generation_input_tokens': suffix_count, 'clone_s': clone_s,
                    'first_token_s': first_s, 'generation_wall_s': generation_s,
                    'mlx_prefill_s': last.get('prompt_tokens', 0) / last['prompt_tps'] if last.get('prompt_tps') else None,
                    'mlx_decode_s': last.get('generation_tokens', 0) / last['generation_tps'] if last.get('generation_tps') else None,
                    'generation_tokens': last.get('generation_tokens'), 'rss_before_bytes': rss_before,
                    **metrics, 'baseline_unchanged': intact,
                    'baseline_verification_s_excluded': time.perf_counter() - validation_started}

        report = {'model_id': MODEL_ID, 'measured_revision': REVISION, 'model_path': str(args.model),
                  'weight_sha256': WEIGHT_SHA, 'weight_hash_checked_this_run': args.verify_weights,
                  'prompt_sha256': PROMPT_SHA, 'driver_sha256': sha(Path(__file__)), 'runtime': versions,
                  'qualification_runner_sha256': QUALIFICATION_RUNNER_SHA, 'python': sys.version.split()[0],
                  'deadline_s': args.deadline, 'context_limit': context_limit, 'repeats': args.repeats,
                  'prefix_tokens': len(prefix_ids), 'static_prefix_text_sha256': hashlib.sha256(static_text.encode()).hexdigest(),
                  'baseline': guard, 'model_load': load_metrics, 'cache_build': build_metrics,
                  'cases': CASES, 'warmup': [], 'rows': [], 'pairs': [], 'fault_checks': [],
                  'notes': ['No transcript-derived cache is reused or written to disk.',
                            'Cache build and full-state verification are excluded from request latency; cloning is included.',
                            'RSS peak is process-lifetime, not per-request; Metal peak includes resident weights and static cache.',
                            'MLX prefill includes first-token work; decode duration is reconstructed from upstream rates.',
                            'A failed boundary check falls back uncached and cannot establish a cache speedup.',
                            'SIGALRM plus post-completion wall check matches qualification; native operations may defer Python signals.',
                            'Use an external process deadline when executing; this is not a hardened production process supervisor.']}
        def save():
            args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
        warm = {'id': 'warmup', 'raw': 'Please um keep the final words exactly as spoken.'}
        report['warmup'] = [request(warm, False), request(warm, True)]
        report['warmup_equivalent'] = equivalent(*report['warmup'])
        for repeat in range(args.repeats):
            for index, case in enumerate(CASES):
                order = [False, True] if (repeat + index) % 2 == 0 else [True, False]
                pair = [request(case, mode) for mode in order]
                for row in pair:
                    row['repeat'] = repeat
                report['rows'].extend(pair)
                report['pairs'].append({'id': case['id'], 'repeat': repeat, 'cached_first': order[0],
                                        'equivalent': equivalent(*pair),
                                        'cache_exercised': any(row['cache_used'] for row in pair)})
                save()
                print(json.dumps(report['pairs'][-1]), flush=True)
        reference = next(row for row in report['rows'] if row['id'] == 'A_first' and not row['cached_requested'])
        aba_rows = [row for row in report['rows'] if row['id'] in ('A_first', 'A_after_B')]
        report['aba_isolated'] = all(equivalent(reference, row) for row in aba_rows)
        for fault in ['cancel', 'timeout']:
            failed = request(CASES[1], True, fault)
            recovered = request(CASES[0], True)
            report['fault_checks'].append({'fault': fault, 'failed_request': failed, 'recovery': recovered,
                                           'passed': bool(failed['error']) and not failed['completed']
                                           and failed['baseline_unchanged'] and equivalent(reference, recovered)})
            save()
        clean_pairs = [pair for pair in report['pairs'] if pair['equivalent'] and pair['cache_exercised']]
        report['equivalence_passed'] = (len(clean_pairs) == len(report['pairs']) and report['aba_isolated']
                                        and report['warmup_equivalent']
                                        and all(row['passed'] for row in report['fault_checks']))
        report['timing_summary'] = {}
        for cached in [False, True]:
            rows = [row for row in report['rows'] if row['cached_requested'] == cached and row['completed']]
            eligible = [row for row in rows if not cached or row['cache_used']]
            report['timing_summary']['cached' if cached else 'uncached'] = {
                'completed': len(rows), 'timing_eligible': len(eligible),
                'median_full_request_s': statistics.median(row['wall_s'] for row in eligible) if eligible else None,
                'median_cpu_s': statistics.median(row['cpu_s'] for row in eligible) if eligible else None,
                'median_clone_s': statistics.median(row['clone_s'] for row in eligible) if cached and eligible else None}
        save()
        print(json.dumps({'equivalence_passed': report['equivalence_passed'], 'timings': report['timing_summary']}, indent=2))
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        signal.signal(signal.SIGALRM, previous_alarm)
        baseline = None
        mx.clear_cache()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--execute', action='store_true')
    parser.add_argument('--self-test', action='store_true')
    parser.add_argument('--model', type=Path, default=Path('/tmp/experiments/sotto-cleanup-overnight/models/minicpm2000'))
    parser.add_argument('--prompt', type=Path, default=HERE / 'prompt-inline.json')
    parser.add_argument('--output', type=Path, default=HERE / 'prefix-cache-comparison.json')
    parser.add_argument('--repeats', type=int, default=2)
    parser.add_argument('--deadline', type=int, default=10)
    parser.add_argument('--verify-weights', action='store_true')
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    if not args.execute:
        parser.error('Explicit --execute required; model work must wait for hardware handoff')
    if not 1 <= args.repeats <= 10 or args.deadline != 10:
        parser.error('Use 1–10 repeats and the frozen 10-second deadline')
    if args.output.exists() or args.output.resolve() in {args.prompt.resolve(), Path(__file__).resolve()}:
        parser.error('Choose a new output path; never overwrite an existing artifact')
    execute(args)


if __name__ == '__main__':
    main()
