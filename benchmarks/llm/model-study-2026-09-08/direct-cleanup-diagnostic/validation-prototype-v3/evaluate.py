"""Offline development evaluation of frozen v2/v3; no inference or release data."""
from collections import Counter
from dataclasses import asdict
import hashlib
import importlib.util
import json
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
BASE = HERE.parent


def import_file(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def read(path):
    return json.loads(path.read_text())


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


frozen = read(HERE / 'freeze.json')
assert sha(HERE / 'source_validation.py') == frozen['validator_sha256']
prior = import_file('v2_evaluation', BASE / 'validation-prototype/evaluate.py')
v2 = prior.validator
v3 = import_file('v3_source_validation', HERE / 'source_validation.py')


def reconstruct(source, result):
    data = source.encode('utf-8')
    for position, before, after in result.capitalization_bytes:
        assert data[position:position+1] == before.encode('ascii')
        data = data[:position] + after.encode('ascii') + data[position+1:]
    gaps = set(getattr(result, 'gap_insertions', ()))
    assert gaps <= {a for a, _ in result.deletion_bytes}
    previous_start = len(data)
    for start, end in reversed(result.deletion_bytes):
        assert 0 <= start <= end <= previous_start
        data = data[:start] + (b' ' if start in gaps else b'') + data[end:]
        previous_start = start
    assert data.decode('utf-8') == result.output
    assert result.accepted or result.output == source


def detected_loss(score):
    return bool(score['protected_failures'] or score['added_word_diagnostic']
                or score['required_word_loss_diagnostic'])


def aggregate(rows, stage):
    result = {}
    groups = ['all', 'ordinary_cleanup', 'ordinary_preserve',
              'adversarial_cleanup', 'adversarial_preserve', 'old_cleanup', 'old_preserve']
    for group in groups:
        selected = [r for r in rows if group == 'all' or r['group'] == group]
        if not selected:
            continue
        scores = [r[stage]['score'] for r in selected]
        result[group] = {
            'cases': len(selected),
            'completed': sum(s['completed'] for s in scores),
            'exact': sum(s['exact'] for s in scores),
            'lexical_exact': sum(s['lexical_exact'] for s in scores),
            'complete_cleanup': sum(s['complete_cleanup'] for s in scores)
                if group.endswith('_cleanup') else None,
            'source_exact': sum(s['source_exact'] for s in scores),
            'added_word_diagnostic_cases': sum(s['added_word_diagnostic'] for s in scores),
            'required_word_loss_diagnostic_cases': sum(s['required_word_loss_diagnostic'] for s in scores),
            'protected_fidelity_failure_cases': sum(bool(s['protected_failures']) for s in scores),
            'any_detected_loss_cases': sum(detected_loss(s) for s in scores),
            'accepted_proposals': sum(r[stage]['result']['accepted'] for r in selected)
                if stage != 'raw' else None,
            'fallbacks': sum(not r[stage]['result']['accepted'] for r in selected)
                if stage != 'raw' else None,
            'changed_from_source': sum(not s['source_exact'] for s in scores),
        }
    return result


def main():
    cases = read(BASE / 'semantic60.json')
    old_cases = read(BASE / 'cases.json')
    by_id = {c['id']: c for c in [*cases, *old_cases]}
    annotations = read(BASE / 'semantic60-protected.json')
    assert sha(BASE / 'semantic60.json') == annotations['dataset_sha256']
    protections = {c['id']: c['protected_spans'] for c in annotations['cases']}
    for case in cases:
        data = case['raw'].encode('utf-8')
        for span in protections[case['id']]:
            assert data[span['start_byte']:span['end_byte']].decode('utf-8') == span['text']
    profiles = {
        p: BASE / f'results/semantic60-{p}.json' for p in [
            'minicpm2000-common', 'pollard_mixed-common', 'spark4000-common',
            'spark4000-best-dev', 'qwen4000-common', 'qwen4000-best-dev']
    }
    profiles.update({p: HERE / f'inputs/{p}.json' for p in ['minicpm2000-D1', 'minicpm2000-D3', 'minicpm2000-D4']})
    summary, reconstruction_checks = {}, 0
    for profile, path in profiles.items():
        original = read(path)
        observed = [r for r in original['rows'] if not r.get('is_warmup')]
        semantic = [r for r in observed if r['id'].startswith('semantic_')]
        assert Counter(r['id'] for r in semantic) == Counter(c['id'] for c in cases)
        assert len(observed) == (69 if profile.endswith(('D1', 'D3', 'D4')) else 60)
        details = []
        for row in observed:
            case = by_id[row['id']]
            output = row.get('final_output', row['raw_output'])
            completed = row['error'] is None and row['finish_reason'] == 'stop'
            annotations = protections.get(row['id'], [])
            detail = {
                'id': row['id'], 'group': case.get('group',
                    'old_cleanup' if prior.lex(case['expected']) != prior.lex(case['raw']) else 'old_preserve'),
                'source': case['raw'], 'expected': case['expected'],
                'acceptable_outputs': case.get('acceptable_outputs', []),
                'protected_annotations_available': row['id'] in protections,
                'raw': {'output': output, 'score': prior.score(output, case, annotations, completed)},
            }
            for stage, validator in [('v2', v2), ('v3', v3)]:
                # Reference outputs and annotated spans NEVER enter this call.
                validated = validator.validate(case['raw'], output, dictionary=(), completed=completed)
                reconstruct(case['raw'], validated)
                reconstruction_checks += 1
                detail[stage] = {
                    'output': validated.output, 'result': asdict(validated),
                    'score': prior.score(validated.output, case, annotations),
                }
            details.append(detail)
        observed60 = [r for r in details if r['id'].startswith('semantic_')]
        stages = {s: aggregate(observed60, s) for s in ['raw', 'v2', 'v3']}
        old = [r for r in details if not r['id'].startswith('semantic_')]
        summary[profile] = {
            'semantic60': stages,
            'old9': {s: aggregate(old, s) for s in ['raw', 'v2', 'v3']} if old else None,
            'remaining_detected_loss_ids': [r['id'] for r in details if detected_loss(r['v3']['score'])],
            'remaining_semantic60_incomplete_cleanup_ids': [r['id'] for r in observed60
                if r['group'].endswith('_cleanup') and not r['v3']['score']['complete_cleanup']],
            'v2_to_v3_changed_outputs': [r['id'] for r in details if r['v2']['output'] != r['v3']['output']],
            'fallback_reasons': dict(Counter(r['v3']['result']['reason'] for r in details
                                           if not r['v3']['result']['accepted'])),
        }
        artifact = {
            'profile': profile, 'validator_sha256': frozen['validator_sha256'],
            'predecessor_sha256': frozen['predecessor_sha256'],
            'input_path': str(path.relative_to(BASE)), 'input_sha256': sha(path),
            'dictionary': [], 'gold_used_only_for_scoring': True,
            'semantic60_is_development': True, 'summary': summary[profile], 'cases': details,
        }
        (HERE / f'{profile}.json').write_text(json.dumps(artifact, indent=2, ensure_ascii=False)+'\n')
    (HERE / 'summary.json').write_text(json.dumps(summary, indent=2)+'\n')
    print(json.dumps({'reconstruction_checks': reconstruction_checks,
                     'validator_sha256': frozen['validator_sha256'],
                     'headline': {p: {s: {
                         'ordinary_cleanup': rows['semantic60'][s]['ordinary_cleanup']['complete_cleanup'],
                         'ordinary_preserve': rows['semantic60'][s]['ordinary_preserve']['source_exact'],
                         'adversarial_cleanup': rows['semantic60'][s]['adversarial_cleanup']['complete_cleanup'],
                         'adversarial_preserve': rows['semantic60'][s]['adversarial_preserve']['source_exact'],
                         'detected_losses': rows['semantic60'][s]['all']['any_detected_loss_cases'],
                     } for s in ['raw', 'v2', 'v3']} for p, rows in summary.items()}}, indent=2))


if __name__ == '__main__':
    main()
