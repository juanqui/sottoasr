"""Offline frozen-prototype evaluation; never load models or qualification data."""
from collections import Counter
import hashlib
import importlib.util
import json
import math
from pathlib import Path
import re
import statistics
import sys

HERE = Path(__file__).resolve().parent
BASE = HERE.parent
EXPECTED_VALIDATOR = '4a7becf860f7062afd2831be8db3886ebb8830865158d4290aaedc5d2faa42a1'
assert hashlib.sha256((HERE / 'source_validation.py').read_bytes()).hexdigest() == EXPECTED_VALIDATOR
spec = importlib.util.spec_from_file_location('frozen_source_validation', HERE / 'source_validation.py')
validator = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = validator
spec.loader.exec_module(validator)


def read(name):
    return json.loads((BASE / name).read_text())


def lex(text):
    return re.findall(r"\w+(?:['’]\w+)*", text.casefold())


def subsequence(short, long):
    index = 0
    for word in long:
        if index < len(short) and short[index] == word:
            index += 1
    return index == len(short)


def fidelity(output, annotations):
    cursor, missing = 0, []
    for span in sorted(annotations, key=lambda item: item['start_byte']):
        literal = span['text']
        found = False
        for candidate in re.finditer(re.escape(literal), output[cursor:]):
            start, end = cursor + candidate.start(), cursor + candidate.end()
            left, right = output[:start], output[end:]
            word_before = bool(left and (left[-1].isalnum() or left[-1] == '_'))
            word_after = bool(right and (right[0].isalnum() or right[0] == '_'))
            numeric_extension = literal.isdecimal() and (
                bool(re.match(r'[.,]\d', right)) or bool(re.search(r'\d[.,]$', left)))
            if ((literal[0].isalnum() and word_before)
                    or (literal[-1].isalnum() and word_after) or numeric_extension):
                continue
            cursor, found = end, True
            break
        if not found:
            missing.append(literal)
    return missing


def score(output, case, protected, completed=True):
    targets = [case['expected'], *case.get('acceptable_outputs', [])]
    words = lex(output)
    failures = fidelity(output, protected) if completed else []
    exact = completed and output.strip() in targets
    lexical = completed and any(words == lex(target) for target in targets)
    return {
        'completed': completed,
        'exact': exact,
        'lexical_exact': lexical,
        'protected_failures': failures,
        'added_word_diagnostic': completed and not subsequence(words, lex(case['raw'])),
        'required_word_loss_diagnostic': completed and not any(subsequence(lex(t), words) for t in targets),
        'complete_cleanup': lexical and not failures,
        'source_exact': completed and output == case['raw'],
    }


def aggregate(rows, stage):
    groups = {}
    for group in ['all', 'ordinary_cleanup', 'ordinary_preserve', 'adversarial_cleanup', 'adversarial_preserve']:
        items = [r for r in rows if group == 'all' or r['group'] == group]
        scores = [r[stage] for r in items]
        groups[group] = {
            'cases': len(items),
            'completed': sum(s['completed'] for s in scores),
            'exact': sum(s['exact'] for s in scores),
            'lexical_exact': sum(s['lexical_exact'] for s in scores),
            'complete_cleanup': sum(s['complete_cleanup'] for s in scores) if group.endswith('_cleanup') else None,
            'source_exact': sum(s['source_exact'] for s in scores),
            'added_word_diagnostic_cases': sum(s['added_word_diagnostic'] for s in scores),
            'required_word_loss_diagnostic_cases': sum(s['required_word_loss_diagnostic'] for s in scores),
            'protected_fidelity_failure_cases': sum(bool(s['protected_failures']) for s in scores),
            'validator_fallbacks': sum(not r['validator']['accepted'] for r in items) if stage == 'delivered' else None,
        }
    return groups


def main():
    cases = read('semantic60.json')
    annotations = read('semantic60-protected.json')
    cases_by_id = {c['id']: c for c in cases}
    protection = {c['id']: c['protected_spans'] for c in annotations['cases']}
    assert len(cases_by_id) == 60 and set(cases_by_id) == set(protection)
    assert hashlib.sha256((BASE / 'semantic60.json').read_bytes()).hexdigest() == annotations['dataset_sha256']
    for identifier, spans in protection.items():
        raw_bytes = cases_by_id[identifier]['raw'].encode('utf-8')
        for span in spans:
            assert raw_bytes[span['start_byte']:span['end_byte']].decode('utf-8') == span['text']
    profiles = ['minicpm2000-common', 'pollard_mixed-common', 'spark4000-common',
                'spark4000-best-dev', 'qwen4000-common', 'qwen4000-best-dev',
                'liquid1200-common', 'liquid350-common']
    archived = read('semantic60-summary.json')['profiles']
    freeze = read('semantic60-freeze.json')
    archive_models = {r['label']: r for r in read('models.json')}
    audit = {'source_quality_counts': {}, 'issues': [], 'dataset_cases': len(cases),
             'dataset_groups': dict(Counter(c['group'] for c in cases)), 'inference_rows': 0,
             'warmup_rows': 0, 'pins_checked': [], 'hash_checks': {}, 'loaded_classes': {},
             'embedded_metric_disagreements': [], 'reconstruction_checks': 0,
             'reporting_qualifications': [
                 '480 scored requests are 8 profiles of 6 distinct models on 60 cases; 8 warmups excluded.',
                 'Canonical external scorer unions primary expected output with alternatives; frozen driver accepted_sample can disagree.',
                 'Lexical diagnostics and exact protected fidelity are not comprehensive semantic adjudication.',
                 'Load timer excludes Python/MLX imports and uses normal OS caches; request timers exclude IPC.',
                 'RSS process peak and per-request Metal allocation peak have different scopes and must not be added.',
                 'Frozen driver lacks a pre-inference context guard; recorded prompt+budget is below every model context in these data.',
             ]}
    for name, expected in freeze['files'].items():
        actual = hashlib.sha256((BASE / name).read_bytes()).hexdigest()
        audit['hash_checks'][name] = actual == expected
        if actual != expected:
            audit['issues'].append(f'Frozen file hash mismatch: {name}')
    for model in freeze['models']:
        if model['revision'] != archive_models[model['label']]['revision']:
            audit['issues'].append(f'Model pin mismatch: {model["label"]}')
        audit['pins_checked'].append({k: model[k] for k in ['label', 'repo', 'revision', 'model_type']})
    summaries = {}
    for profile in profiles:
        original = read(f'results/semantic60-{profile}.json')
        observed = [r for r in original['rows'] if not r.get('is_warmup')]
        warmup = [r for r in original['rows'] if r.get('is_warmup')]
        assert len(observed) == 60 and Counter(r['id'] for r in observed) == Counter(cases_by_id.keys())
        assert len(warmup) == 1 and warmup[0]['id'] == 'reported_sentence'
        audit['inference_rows'] += len(observed)
        audit['warmup_rows'] += len(warmup)
        expected_prompt = freeze['files']['prompt.json' if 'best-dev' in profile else 'prompt-envelope.json']
        assert original['prompt_sha256'] == expected_prompt
        assert original['cases_sha256'] == annotations['dataset_sha256']
        assert all(version == freeze['runtime'][name] for name, version in original['runtime'].items())
        assert original['temperature'] == 0 and original['deadline_s'] == 10 and not original['native_thinking']
        assert original['memory_limit_bytes'] == 4*1024**3 and original['cache_limit_bytes'] == 128*1024**2
        audit['loaded_classes'][profile] = original['loaded_model_class']
        details = []
        for row in observed:
            case = cases_by_id[row['id']]
            proposed = row.get('final_output', row['raw_output'])
            completed = row['error'] is None and row['finish_reason'] == 'stop'
            raw_score = score(proposed, case, protection[row['id']], completed)
            if row['accepted_sample'] != raw_score['exact']:
                audit['embedded_metric_disagreements'].append({
                    'profile': profile, 'id': row['id'], 'driver_accepted_sample': row['accepted_sample'],
                    'canonical_accepted_exact': raw_score['exact']})
            detail = {'id': row['id'], 'group': case['group'], 'source': case['raw'],
                      'proposal': proposed, 'raw': raw_score}
            # Neither reference output nor protected annotations enters validation.
            if not profile.startswith('liquid'):
                result = validator.validate(case['raw'], proposed, dictionary=(), completed=completed)
                reconstructed = case['raw'].encode('utf-8')
                for position, before, after in result.capitalization_bytes:
                    assert reconstructed[position:position+1] == before.encode('ascii')
                    reconstructed = reconstructed[:position]+after.encode('ascii')+reconstructed[position+1:]
                previous_start = len(reconstructed)
                for start, end in reversed(result.deletion_bytes):
                    assert 0 <= start <= end <= previous_start
                    reconstructed = reconstructed[:start]+reconstructed[end:]
                    previous_start = start
                assert reconstructed.decode('utf-8') == result.output
                assert result.accepted or result.output == case['raw']
                audit['reconstruction_checks'] += 1
                detail['validator'] = {'accepted': result.accepted, 'reason': result.reason,
                                       'categories': result.categories, 'deletion_bytes': result.deletion_bytes,
                                       'capitalization_bytes': result.capitalization_bytes,
                                       'equivalent_alignments': result.equivalent_alignments}
                detail['output'] = result.output
                detail['delivered'] = score(result.output, case, protection[row['id']])
            details.append(detail)
        actual_groups = {}
        for group in archived[profile]:
            selected = [i for i, row in enumerate(observed) if group == 'all'
                        or cases_by_id[row['id']]['group'] == group
                        or cases_by_id[row['id']]['group'].startswith(group+'_')]
            rows = [observed[i] for i in selected]
            scores = [details[i]['raw'] for i in selected]
            times = sorted(row['request_elapsed_s'] for row in rows)
            metrics = {
                'cases': len(rows), 'completed': sum(s['completed'] for s in scores),
                'accepted_exact': sum(s['exact'] for s in scores),
                'lexical_exact': sum(s['lexical_exact'] for s in scores),
                'added_word_diagnostic_cases': sum(s['added_word_diagnostic'] for s in scores),
                'required_word_loss_diagnostic_cases': sum(s['required_word_loss_diagnostic'] for s in scores),
                'protected_fidelity_failure_cases': sum(bool(s['protected_failures']) for s in scores),
                'useful_edits_without_detected_loss': sum(s['completed'] and not s['added_word_diagnostic']
                    and not s['required_word_loss_diagnostic'] and not s['protected_failures']
                    and lex(rows[j].get('final_output', rows[j]['raw_output'])) != lex(cases_by_id[rows[j]['id']]['raw'])
                    and cases_by_id[rows[j]['id']]['group'].endswith('_cleanup') for j, s in enumerate(scores)),
                'median_request_s': statistics.median(times),
                'p95_request_s': times[math.ceil(.95*len(times))-1],
                'maximum_request_s': max(times), 'total_cpu_s': sum(r['cpu_s'] for r in rows),
                'peak_rss_process_bytes': max(r['peak_rss_process_bytes'] for r in rows),
                'maximum_observed_rss_after_bytes': max(r['rss_after_bytes'] for r in rows),
                'peak_metal_request_bytes': max(r['peak_metal_request_bytes'] for r in rows),
                'maximum_prompt_plus_budget_tokens': max((r['prompt_tokens'] or 0)+r['budget'] for r in rows),
            }
            for key, value in metrics.items():
                if abs(value-archived[profile][group][key]) > 1e-8:
                    audit['issues'].append(f'Summary mismatch {profile}/{group}/{key}: {value} != {archived[profile][group][key]}')
            actual_groups[group] = metrics
        audit['source_quality_counts'][profile] = actual_groups['all']
        if not profile.startswith('liquid'):
            summary = {'raw': aggregate(details, 'raw'), 'delivered': aggregate(details, 'delivered'),
                       'fallback_reasons': dict(Counter(r['validator']['reason'] for r in details if not r['validator']['accepted']))}
            summaries[profile] = summary
            artifact = {'profile': profile, 'validator_sha256': EXPECTED_VALIDATOR,
                        'dictionary': [], 'protected_annotations_used_only_for_scoring': True,
                        'source_result_sha256': hashlib.sha256((BASE / f'results/semantic60-{profile}.json').read_bytes()).hexdigest(),
                        'summary': summary, 'cases': details}
            (HERE / f'{profile}.json').write_text(json.dumps(artifact, indent=2, ensure_ascii=False)+'\n')
    (HERE / 'summary.json').write_text(json.dumps(summaries, indent=2)+'\n')
    (HERE / 'archive-audit.json').write_text(json.dumps(audit, indent=2)+'\n')
    print(json.dumps({'audit': {k: v for k, v in audit.items() if k != 'source_quality_counts'},
                      'headline': {p: {'raw': s['raw']['all'], 'delivered': s['delivered']['all'],
                                       'ordinary_cleanup': s['delivered']['ordinary_cleanup'],
                                       'adversarial_preserve': s['delivered']['adversarial_preserve']}
                                   for p, s in summaries.items()}}, indent=2))


if __name__ == '__main__':
    main()
