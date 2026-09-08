#!/usr/bin/env python3
"""Benchmark the production Rust candidates and production Python classifier.

Build the adapter once: cargo build --example cleanup_edits (in src-tauri).
Uses a complete downloaded experiment snapshot, never live model configuration.
"""

import argparse
import importlib.util
import json
import statistics
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--model-path', type=Path, required=True)
    parser.add_argument('--dataset', type=Path, default=Path(__file__).with_name('sparse-holdout.json'))
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--adapter', type=Path, default=ROOT / 'src-tauri/target/debug/examples/cleanup_edits')
    args = parser.parse_args()
    spec = importlib.util.spec_from_file_location('production_cleanup', ROOT / 'src-tauri/sidecar/llm_cleanup.py')
    cleanup = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(cleanup)
    # Only this isolated benchmark process resolves the explicitly supplied model.
    cleanup.cached_model_path = lambda: args.model_path.resolve()
    proc = subprocess.Popen([str(args.adapter)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    def rust(request):
        proc.stdin.write(json.dumps(request) + '\n')
        proc.stdin.flush()
        value = json.loads(proc.stdout.readline())
        if 'error' in value:
            raise ValueError(value['error'])
        return value
    results = []
    try:
        for case in json.loads(args.dataset.read_text()):
            text = case['raw']
            choices = rust({'text': text})['candidates'] if len(text.split()) >= 5 else []
            started = time.perf_counter()
            reason = None
            try:
                if choices:
                    ids, _ = cleanup.select_deletions(text, choices)
                    output = rust({'text': text, 'delete_ids': ids})['text']
                else:
                    ids, output = [], text
            except (ValueError, RuntimeError) as error:
                reason = str(error)
                ids, output = [], text
            elapsed = time.perf_counter() - started
            row = {**case, 'candidates': choices, 'selected_ids': ids, 'output': output,
                   'exact': output == case['expected'], 'changed': output != text,
                   'fallback_reason': reason, 'elapsed_s': round(elapsed, 4)}
            results.append(row)
            print(json.dumps({k: row[k] for k in ('id', 'exact', 'changed', 'fallback_reason', 'elapsed_s')}), flush=True)
    finally:
        proc.stdin.close()
        proc.wait(timeout=5)
    processed = [r for r in results if r['candidates']]
    summary = {
        'cases': len(results), 'exact': sum(r['exact'] for r in results),
        'needs_edits': sum(r['raw'] != r['expected'] for r in results),
        'changed': sum(r['changed'] for r in results),
        'fallbacks': sum(r['fallback_reason'] is not None for r in results),
        'median_inference_and_apply_s': statistics.median(r['elapsed_s'] for r in processed) if processed else 0,
        'max_inference_and_apply_s': max((r['elapsed_s'] for r in processed), default=0),
    }
    import mlx.core as mx
    from importlib.metadata import version
    summary['peak_metal_memory_gib'] = mx.get_peak_memory() / 1024**3
    summary['mlx_lm_version'] = version('mlx-lm')
    args.output.write_text(json.dumps({'model_path': str(args.model_path), 'summary': summary, 'results': results}, indent=2, ensure_ascii=False))
    print(json.dumps(summary), flush=True)


if __name__ == '__main__':
    main()
