"""Offline v5/v6 development comparison. Never import a model or discover datasets.

The explicit, hashed inputs.json allowlist contains only already-revealed data.
Gold outputs/annotations enter scoring only, never the source validator.
"""
from collections import Counter
from dataclasses import asdict
import hashlib
import importlib.util
import json
from pathlib import Path
import platform
import sys
import unicodedata

HERE = Path(__file__).resolve().parent
BASE = HERE.parent


def read(path):
    return json.loads(path.read_text())


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


freeze = read(HERE / 'freeze.json')
assert sha(HERE / 'source_validation.py') == freeze['validator_sha256']
assert sha(BASE / 'validation-prototype-v5/source_validation.py') == freeze['predecessor_sha256']
validators = {
    'v5': load('compare_v5', BASE / 'validation-prototype-v5/source_validation.py'),
    'v6': load('compare_v6', HERE / 'source_validation.py'),
}
spans = load('canonical_spans', BASE / 'qualification/span_metrics.py')
exact = load('canonical_exact', BASE / 'qualification/score_semantic.py')


def reconstruct(source, result):
    data = source.encode('utf-8')
    for position, before, after in result.capitalization_bytes:
        assert data[position:position+1] == before.encode('ascii')
        data = data[:position] + after.encode('ascii') + data[position+1:]
    gaps = set(result.gap_insertions)
    assert gaps <= {a for a, _ in result.deletion_bytes}
    previous_start = len(data)
    for start, end in reversed(result.deletion_bytes):
        assert 0 <= start < end <= previous_start
        data = data[:start] + (b' ' if start in gaps else b'') + data[end:]
        previous_start = start
    assert data.decode('utf-8') == result.output
    assert result.accepted or result.output == source


def score(output, case, completed=True):
    targets = [case['expected'], *case.get('acceptable_outputs', [])]
    reference_scores = [spans.score_case(case['raw'], target, output) for target in targets]
    canonical = reference_scores[0]
    # Reference alternatives are authored before evaluation. Prefer an exact
    # lexical match, then a preservation-valid reference; retain the primary
    # score separately so changing a denominator is explicit and inspectable.
    chosen_index = max(range(len(targets)), key=lambda i: (
        reference_scores[i]['lexically_complete'], reference_scores[i]['preservation_valid'],
        -reference_scores[i]['required_word_losses'], -reference_scores[i]['source_word_additions'], -i))
    accepted_reference = reference_scores[chosen_index]
    available = case['protected_annotations_available']
    protected = exact.protected_failures(output, case['protected_spans']) if available else None
    lexical = any(spans.words(output) == spans.words(t) for t in targets)
    edit_required = spans.words(case['raw']) != spans.words(case['expected'])
    return {
        'quality_eligible': completed,
        'accepted_exact': completed and output in targets,
        'accepted_lexical': completed and lexical,
        'complete_cleanup_with_fidelity': completed and edit_required and lexical and not protected,
        'source_exact': completed and output == case['raw'],
        'canonical_span_score': canonical,
        'accepted_reference_index': chosen_index,
        'accepted_reference_span_score': accepted_reference,
        'protected_failures': protected,
        'detected_harm': completed and (not any(s['preservation_valid'] for s in reference_scores) or bool(protected)),
    }


def aggregate(rows, stage):
    result = {}
    for group in ['all', *sorted({row['group'] for row in rows})]:
        chosen = [row for row in rows if group == 'all' or row['group'] == group]
        scores = [row[stage]['score'] for row in chosen]
        eligible = [s for s in scores if s['quality_eligible']]
        result[group] = {
            'cases': len(chosen),
            'quality_eligible_cases': len(eligible),
            'accepted_exact': sum(s['accepted_exact'] for s in scores),
            'accepted_lexical': sum(s['accepted_lexical'] for s in scores),
            'complete_cleanup_with_fidelity': sum(s['complete_cleanup_with_fidelity'] for s in scores),
            'source_exact': sum(s['source_exact'] for s in scores),
            'protected_annotation_cases': sum(r['protected_annotations_available'] for r in chosen),
            'protected_failure_cases': sum(bool(s['protected_failures']) for s in eligible),
            'detected_harm_cases': sum(s['detected_harm'] for s in scores),
            'accepted_proposals': sum(r[stage]['result']['accepted'] for r in chosen) if stage != 'raw' else None,
            'primary_reference_span_metrics': spans.summarize([s['canonical_span_score'] for s in eligible]),
            'accepted_reference_span_metrics': spans.summarize([s['accepted_reference_span_score'] for s in eligible]),
        }
    return result


def main():
    inputs = read(HERE / 'inputs.json')
    for relative, expected in inputs['files'].items():
        assert sha(BASE / relative) == expected, relative
    summary, changes, new_harms, checks = {}, [], [], 0
    for profile in inputs['profiles']:
        cases = []
        for relative in profile['gold']:
            cases.extend(read(BASE / relative))
        external = {}
        if profile.get('protected'):
            annotated = read(BASE / profile['protected'])
            external = {c['id']: c['protected_spans'] for c in annotated['cases']}
        for case in cases:
            case['protected_annotations_available'] = case.get(
                'protected_annotations_available', 'protected_spans' in case or case['id'] in external)
            case['protected_spans'] = case.get('protected_spans', external.get(case['id'], []))
            data, previous_end = case['raw'].encode('utf-8'), 0
            for protected in case['protected_spans']:
                start, end = protected['start_byte'], protected['end_byte']
                assert previous_end <= start < end <= len(data)
                assert data[start:end] == protected['text'].encode('utf-8')
                previous_end = end
        original = read(BASE / profile['raw'])
        observed = [r for r in original['rows'] if not r.get('is_warmup')]
        assert Counter(r['id'] for r in observed) == Counter(c['id'] for c in cases)
        by_id = {c['id']: c for c in cases}
        assert len(by_id) == len(cases)
        details = []
        for row in observed:
            case = by_id[row['id']]
            if 'source' in row:
                assert row['source'] == case['raw']
            output = row['final_output'].strip()
            completed = row['completed'] and row['error'] is None and row['finish_reason'] == 'stop'
            detail = {
                'id': row['id'], 'group': case.get('group', 'old_cleanup' if spans.words(case['raw']) != spans.words(case['expected']) else 'old_preserve'),
                'source': case['raw'], 'expected': case['expected'],
                'acceptable_outputs': case.get('acceptable_outputs', []),
                'protected_annotations_available': case['protected_annotations_available'],
                'completed': completed, 'finish_reason': row['finish_reason'], 'error': row['error'],
                'raw': {'output': output, 'score': score(output, case, completed)},
            }
            for stage, validator in validators.items():
                # Do not pass gold outputs or protected annotations to validation.
                result = validator.validate(case['raw'], output, dictionary=(), completed=completed)
                reconstruct(case['raw'], result)
                checks += 1
                detail[stage] = {'output': result.output, 'result': asdict(result), 'score': score(result.output, case)}
            if detail['v5']['output'] != detail['v6']['output']:
                change = {'profile': profile['name'], 'id': row['id'], 'source': case['raw'],
                          'raw': output, 'v5': detail['v5']['output'], 'v6': detail['v6']['output'],
                          'v5_harm': detail['v5']['score']['detected_harm'], 'v6_harm': detail['v6']['score']['detected_harm']}
                changes.append(change)
            if detail['v6']['score']['detected_harm'] and not detail['v5']['score']['detected_harm']:
                new_harms.append({'profile': profile['name'], 'id': row['id'], 'source': case['raw'],
                                  'raw': output, 'v5': detail['v5']['output'], 'v6': detail['v6']['output']})
            details.append(detail)
        stages = {s: aggregate(details, s) for s in ['raw', 'v5', 'v6']}
        summary[profile['name']] = {
            'stages': stages,
            'v5_to_v6_changed_ids': [c['id'] for c in changes if c['profile'] == profile['name']],
            'v5_to_v6_new_harm_ids': [c['id'] for c in new_harms if c['profile'] == profile['name']],
            'v6_remaining_harm_ids': [r['id'] for r in details if r['v6']['score']['detected_harm']],
        }
        artifact = {'profile': profile, 'validator_sha256': freeze['validator_sha256'],
                    'predecessor_sha256': freeze['predecessor_sha256'], 'gold_used_only_for_scoring': True,
                    'dictionary': [], 'language_bypass': False,
                    'all_data_is_revealed_development': True,
                    'summary': summary[profile['name']], 'cases': details}
        (HERE / f"{profile['name']}.json").write_text(json.dumps(artifact, indent=2, ensure_ascii=False)+'\n')
    (HERE / 'summary.json').write_text(json.dumps(summary, indent=2)+'\n')
    review = {'validator_sha256': freeze['validator_sha256'], 'profiles': len(inputs['profiles']),
              'proposals': checks//2, 'reconstruction_checks': checks,
              'python': platform.python_version(), 'unicode': unicodedata.unidata_version,
              'language_bypass': False, 'all_data_is_revealed_development': True,
              'v5_to_v6_changed_outputs': changes, 'v5_to_v6_new_harms': new_harms,
              'v6_remaining_harms': {p: s['v6_remaining_harm_ids'] for p, s in summary.items()}}
    (HERE / 'review.json').write_text(json.dumps(review, indent=2, ensure_ascii=False)+'\n')
    print(json.dumps({k: v for k, v in review.items() if k != 'v5_to_v6_changed_outputs'}, indent=2))


if __name__ == '__main__':
    main()
