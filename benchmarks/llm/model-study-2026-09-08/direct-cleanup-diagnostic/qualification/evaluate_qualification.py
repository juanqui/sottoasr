"""Apply one frozen source validator; score raw and delivered text separately."""
import argparse
from dataclasses import asdict
import hashlib
import importlib.util
import json
from pathlib import Path
import statistics
import sys
import time


def load_module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--gold', type=Path, required=True)
    parser.add_argument('--results', type=Path, required=True)
    parser.add_argument('--validator', type=Path, required=True)
    parser.add_argument('--protected', type=Path)
    parser.add_argument('--embedded-protected', action='store_true')
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    here = Path(__file__).resolve().parent
    validator = load_module('qualification_validator', args.validator)
    spans = load_module('qualification_spans', here / 'span_metrics.py')
    exact = load_module('qualification_exact', here / 'score_semantic.py')
    cases = json.loads(args.gold.read_text())
    raw_result = json.loads(args.results.read_text())
    by_id = {row['id']: row for row in raw_result['rows'] if not row.get('is_warmup')}
    if len(by_id) != sum(not row.get('is_warmup') for row in raw_result['rows']) or len(by_id) != len(cases) or set(by_id) != {c['id'] for c in cases}:
        raise ValueError('Incomplete, duplicate or mismatched qualification results')
    annotations = None
    if args.protected:
        annotations = {c['id']: c['protected_spans'] for c in json.loads(args.protected.read_text())['cases']}
        if set(annotations) != {c['id'] for c in cases}:
            raise ValueError('Protected annotation IDs must exactly match qualification gold')
    if args.embedded_protected:
        annotations = {c['id']: c['protected_spans'] for c in cases}
    if annotations is not None:
        for case in cases:
            previous_end = 0
            for span in annotations[case['id']]:
                start, end = span['start_byte'], span['end_byte']
                if not (previous_end <= start < end <= len(case['raw'].encode('utf-8'))) or case['raw'].encode('utf-8')[start:end] != span['text'].encode('utf-8'):
                    raise ValueError('Invalid protected source byte annotation')
                previous_end = end
    details = []
    for case in cases:
        row = by_id[case['id']]
        proposal = row['final_output'].strip()
        completed = row['completed'] and row['error'] is None and row['finish_reason'] == 'stop'
        started = time.perf_counter()
        validated = validator.validate(case['raw'], proposal, dictionary=(), completed=completed)
        validation_s = time.perf_counter() - started
        targets = [case['expected'], *case.get('acceptable_outputs', [])]
        stages = {}
        for stage, output in [('raw', proposal), ('delivered', validated.output)]:
            quality_eligible = stage == 'delivered' or completed
            stages[stage] = {
                'output': output,
                'quality_eligible': quality_eligible,
                'accepted_exact': quality_eligible and output in targets,
                'canonical_span_score': spans.score_case(case['raw'], case['expected'], output),
                'protected_failures': exact.protected_failures(output, annotations[case['id']]) if annotations is not None else None,
            }
        details.append({'id': case['id'], 'group': case.get('group', case.get('category', 'unspecified')), 'source': case['raw'],
                        'expected': case['expected'], 'acceptable_outputs': case.get('acceptable_outputs', []),
                        'completed': completed, 'finish_reason': row['finish_reason'], 'error': row['error'],
                        'validation': asdict(validated), 'validation_s': validation_s,
                        'model_request_s': row['request_elapsed_s'],
                        'serial_model_plus_offline_validation_s': row['request_elapsed_s'] + validation_s,
                        **stages})
    groups = sorted({r['group'] for r in details})
    summary = {}
    for group in ['all', *groups]:
        chosen = [r for r in details if group == 'all' or r['group'] == group]
        summary[group] = {'cases': len(chosen), 'model_completed': sum(r['completed'] for r in chosen),
                          'proposals_accepted': sum(r['validation']['accepted'] for r in chosen),
                          'model_median_s': statistics.median(r['model_request_s'] for r in chosen),
                          'validation_median_s': statistics.median(r['validation_s'] for r in chosen)}
        for stage in ['raw', 'delivered']:
            scored = [r[stage]['canonical_span_score'] for r in chosen if r[stage]['quality_eligible']]
            summary[group][stage] = {'quality_eligible_cases': len(scored),
                'accepted_exact': sum(r[stage]['accepted_exact'] for r in chosen),
                'protected_failure_cases': sum(bool(r[stage]['protected_failures']) for r in chosen) if annotations is not None else None,
                'span_metrics': spans.summarize(scored)}
    args.out.write_text(json.dumps({'model_result_sha256': hashlib.sha256(args.results.read_bytes()).hexdigest(),
        'gold_sha256': hashlib.sha256(args.gold.read_bytes()).hexdigest(),
        'validator_sha256': hashlib.sha256(args.validator.read_bytes()).hexdigest(),
        'protected_annotation_sha256': hashlib.sha256(args.gold.read_bytes()).hexdigest() if args.embedded_protected else hashlib.sha256(args.protected.read_bytes()).hexdigest() if args.protected else None,
        'protected_annotations_embedded': args.embedded_protected,
        'summary': summary, 'cases': details}, indent=2, ensure_ascii=False)+'\n')
    print(json.dumps(summary, indent=2))


if __name__ == '__main__':
    main()
