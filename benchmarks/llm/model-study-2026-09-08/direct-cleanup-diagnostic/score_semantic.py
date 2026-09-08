"""Offline diagnostics; this scorer never filters or changes model generation."""
import argparse
import difflib
import json
import math
import pathlib
import re
import statistics

def words(text):
    return re.findall(r"\w+(?:['’]\w+)*", text.casefold())

def subseq(needle, haystack):
    iterator = iter(haystack)
    return all(any(c == h for h in iterator) for c in needle)

def protected_failures(output, spans):
    cursor = 0
    failed = []
    for span in sorted(spans, key=lambda s: s['start_byte']):
        payload = span['text']
        accepted = None
        for match in re.finditer(re.escape(payload), output[cursor:]):
            start, end = cursor + match.start(), cursor + match.end()
            if payload[0].isalnum() and start and (output[start - 1].isalnum() or output[start - 1] == '_'):
                continue
            if payload[-1].isalnum() and end < len(output) and (output[end].isalnum() or output[end] == '_'):
                continue
            if payload.isdecimal() and (re.match(r'[.,]\d', output[end:]) or re.search(r'\d[.,]$', output[:start])):
                continue
            accepted = end
            break
        if accepted is None:
            failed.append(span)
        else:
            cursor = accepted
    return failed

def evaluate(dataset, annotations, result):
    by_id = {c['id']: c for c in dataset}
    protected = {c['id']: c['protected_spans'] for c in annotations['cases']}
    details = []
    for row in result['rows']:
        if row.get('is_warmup') or row['id'] not in by_id:
            continue
        case = by_id[row['id']]
        output = row.get('final_output', row['raw_output']).strip()
        targets = [case['expected'], *case.get('acceptable_outputs', [])]
        completed = row['error'] is None and row['finish_reason'] == 'stop'
        added = not subseq(words(output), words(case['raw']))
        lost = not any(subseq(words(target), words(output)) for target in targets)
        fidelity = protected_failures(output, protected[row['id']]) if completed else []
        changed = words(output) != words(case['raw'])
        details.append({'id': row['id'], 'group': case['group'], 'completed': completed,
                        'accepted_exact': completed and output in targets,
                        'lexical_exact': completed and any(words(output) == words(target) for target in targets),
                        'added_word_diagnostic': completed and added,
                        'required_word_loss_diagnostic': completed and lost,
                        'protected_failures': fidelity,
                        'useful_lexical_edit_without_detected_loss': completed and changed and not added and not lost and not fidelity and case['group'].endswith('_cleanup'),
                        'request_s': row['request_elapsed_s'], 'cpu_s': row['cpu_s'],
                        'rss_after_bytes': row['rss_after_bytes'],
                        'peak_rss_process_bytes': row['peak_rss_process_bytes'],
                        'peak_metal_request_bytes': row['peak_metal_request_bytes'],
                        'prompt_plus_budget_tokens': (row['prompt_tokens'] or 0) + row['budget'],
                        'source': case['raw'], 'expected': case['expected'], 'accepted_alternatives': case.get('acceptable_outputs', []),
                        'output': output, 'finish_reason': row['finish_reason'], 'error': row['error'],
                        'word_changes': row['word_changes']})
    aggregates = {}
    groups = ['all', 'ordinary', 'adversarial', 'ordinary_cleanup', 'ordinary_preserve', 'adversarial_cleanup', 'adversarial_preserve']
    for group in groups:
        values = [r for r in details if group == 'all' or r['group'] == group or r['group'].startswith(group + '_')]
        if not values:
            continue
        times = sorted(r['request_s'] for r in values)
        aggregates[group] = {'cases': len(values), 'completed': sum(r['completed'] for r in values),
                             'accepted_exact': sum(r['accepted_exact'] for r in values),
                             'lexical_exact': sum(r['lexical_exact'] for r in values),
                             'added_word_diagnostic_cases': sum(r['added_word_diagnostic'] for r in values),
                             'required_word_loss_diagnostic_cases': sum(r['required_word_loss_diagnostic'] for r in values),
                             'protected_fidelity_failure_cases': sum(bool(r['protected_failures']) for r in values),
                             'useful_edits_without_detected_loss': sum(r['useful_lexical_edit_without_detected_loss'] for r in values),
                             'median_request_s': statistics.median(times), 'p95_request_s': times[max(0, math.ceil(.95 * len(times)) - 1)],
                             'maximum_request_s': max(times), 'total_cpu_s': sum(r['cpu_s'] for r in values),
                             'peak_rss_process_bytes': max(r['peak_rss_process_bytes'] for r in values),
                             'maximum_observed_rss_after_bytes': max(r['rss_after_bytes'] for r in values),
                             'peak_metal_request_bytes': max(r['peak_metal_request_bytes'] for r in values),
                             'maximum_prompt_plus_budget_tokens': max(r['prompt_plus_budget_tokens'] for r in values)}
    return {'label': result['label'], 'prompt_sha256': result['prompt_sha256'], 'aggregates': aggregates, 'cases': details}

if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--results', nargs='+', required=True)
    parser.add_argument('--directory', default=str(pathlib.Path(__file__).resolve().parent))
    args = parser.parse_args()
    base = pathlib.Path(args.directory)
    dataset = json.loads((base / 'semantic60.json').read_text())
    annotations = json.loads((base / 'semantic60-protected.json').read_text())
    for path in args.results:
        source = pathlib.Path(path)
        scored = evaluate(dataset, annotations, json.loads(source.read_text()))
        source.with_name(source.stem + '-scored.json').write_text(json.dumps(scored, indent=2, ensure_ascii=False) + '\n')
        print(json.dumps({'profile': source.stem, **scored['aggregates']}, ensure_ascii=False))
