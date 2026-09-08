"""Recheck frozen v4 against the same 567 development proposals; no inference."""
from dataclasses import asdict
import hashlib
import importlib.util
import json
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
BASE = HERE.parent
PREVIOUS = BASE / 'validation-prototype-v3'


def load_module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def read(path):
    return json.loads(path.read_text())


freeze = read(HERE / 'freeze.json')
assert sha(HERE / 'source_validation.py') == freeze['validator_sha256']
assert sha(PREVIOUS / 'source_validation.py') == freeze['predecessor_sha256']
prior = load_module('frozen_v3_scoring', PREVIOUS / 'evaluate.py')
validator = load_module('frozen_v4_validator', HERE / 'source_validation.py')


def main():
    protections = {c['id']: c['protected_spans'] for c in
                   read(BASE / 'semantic60-protected.json')['cases']}
    cases = {c['id']: c for c in [*read(BASE / 'semantic60.json'), *read(BASE / 'cases.json')]}
    profiles = list(read(PREVIOUS / 'summary.json'))
    summary, changed_outputs, checks = {}, [], 0
    for profile in profiles:
        path = PREVIOUS / f'{profile}.json'
        source_result = read(path)
        details = []
        for old in source_result['cases']:
            case = cases[old['id']]
            result = validator.validate(case['raw'], old['raw']['output'], dictionary=(),
                                        completed=old['raw']['score']['completed'])
            prior.reconstruct(case['raw'], result)
            checks += 1
            detail = {key: value for key, value in old.items() if key != 'v2'}
            detail['v4'] = {
                'output': result.output, 'result': asdict(result),
                'score': prior.prior.score(result.output, case, protections.get(old['id'], [])),
            }
            if detail['v3']['output'] != result.output:
                changed_outputs.append({'profile': profile, 'id': old['id'],
                                        'v3': detail['v3']['output'], 'v4': result.output})
            details.append(detail)
        semantic = [r for r in details if r['id'].startswith('semantic_')]
        old9 = [r for r in details if not r['id'].startswith('semantic_')]
        summary[profile] = {
            'semantic60': {stage: prior.aggregate(semantic, stage) for stage in ['raw', 'v3', 'v4']},
            'old9': {stage: prior.aggregate(old9, stage) for stage in ['raw', 'v3', 'v4']} if old9 else None,
            'remaining_detected_loss_ids': [r['id'] for r in details if prior.detected_loss(r['v4']['score'])],
        }
        artifact = {'profile': profile, 'validator_sha256': freeze['validator_sha256'],
                    'input_path': str(path.relative_to(BASE)), 'input_sha256': sha(path),
                    'dictionary': [], 'gold_used_only_for_scoring': True,
                    'summary': summary[profile], 'cases': details}
        (HERE / f'{profile}.json').write_text(json.dumps(artifact, indent=2, ensure_ascii=False)+'\n')
    (HERE / 'summary.json').write_text(json.dumps(summary, indent=2)+'\n')
    review = {'reconstruction_checks': checks, 'profiles': len(profiles),
              'v3_to_v4_changed_outputs': changed_outputs,
              'remaining_detected_loss_ids': {p: s['remaining_detected_loss_ids'] for p, s in summary.items()},
              'validator_sha256': freeze['validator_sha256']}
    (HERE / 'review.json').write_text(json.dumps(review, indent=2)+'\n')
    print(json.dumps(review, indent=2))


if __name__ == '__main__':
    main()
