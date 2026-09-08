"""Frozen research-only first-token classifier; Rust supplies every exact edit."""
import json
import os
from pathlib import Path
import subprocess
import time

ROOT = Path(__file__).resolve().parents[3]
CONFIG = json.loads(Path(os.environ.get('SOTTO_EXPERIMENT_CLASSIFIER_CONFIG', Path(__file__).with_name('binary-classifier-v1.json'))).read_text())


def install(cleanup):
    def select_deletions(text, candidates):
        cleanup.validate_candidates(candidates)
        if not isinstance(text, str) or len(text) > CONFIG['input_char_limit']:
            raise ValueError('Input exceeds cleanup limit')
        if not candidates:
            return [], 0
        cleanup.load_model()
        import mlx.core as mx
        from mlx_lm import stream_generate
        labels = [cleanup._tokenizer.encode(x, add_special_tokens=False) for x in ['0', '1']]
        if any(len(tokens) != 1 for tokens in labels):
            raise ValueError('Classifier requires single-token labels')
        base = [{'role': 'system', 'content': CONFIG['system']}]
        def content(original, edited):
            return json.dumps({'original_dictation': original, 'proposed_edit': edited}, ensure_ascii=False)
        for original, edited, label in CONFIG['examples']:
            base.extend([{'role': 'user', 'content': content(original, edited)}, {'role': 'assistant', 'content': label}])
        selected = []
        started = time.perf_counter()
        try:
            for choice in candidates:
                request = json.dumps({'text': text, 'delete_ids': [choice['id']]}) + '\n'
                response = subprocess.run([str(Path(__file__).with_name('guard-adapter')/'target/debug/cleanup_edits')], input=request, text=True, capture_output=True, timeout=1, check=True)
                proposed = json.loads(response.stdout)
                if 'error' in proposed:
                    raise ValueError('Rust rejected edit proposal')
                prompt = cleanup._tokenizer.apply_chat_template(
                    base + [{'role': 'user', 'content': content(text, proposed['text'])}],
                    add_generation_prompt=True, tokenize=False, enable_thinking=False,
                )
                last = None
                for result in stream_generate(cleanup._model, cleanup._tokenizer, prompt=prompt, max_tokens=1, sampler=cleanup._sampler):
                    last = result
                if time.perf_counter() - started >= CONFIG['timeout_seconds']:
                    raise TimeoutError('Classifier exceeded its complete-request deadline')
                if last is None or last.token not in (labels[0][0], labels[1][0]):
                    raise ValueError('Classifier did not return a valid decision label')
                if last.token == labels[1][0]:
                    selected.append(choice['id'])
                mx.clear_cache()
            return selected, int((time.perf_counter() - started) * 1000)
        finally:
            mx.clear_cache()
    cleanup.select_deletions = select_deletions
