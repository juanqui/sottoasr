"""Mechanically export revealed v6 oracle cases; no model or new gold discovery."""
from collections import Counter
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import platform
import sys
import unicodedata
import unittest

ROOT = Path(__file__).resolve().parents[3]
FROZEN = ROOT / 'benchmarks/llm/model-study-2026-09-08/direct-cleanup-diagnostic/validation-prototype-v6'
DESTINATION = ROOT / 'src-tauri/tests/fixtures/cleanup-validation-v6.json'
ORACLE_SHA256 = '4a8bc2a49feb6b944e765ec238f7bdc381a65576907d1b8ddddf9a4b172b177b'


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    assert platform.python_version() == '3.14.5', 'Use the measured Python 3.14.5 oracle runtime'
    assert unicodedata.unidata_version == '16.0.0'
    source_path = FROZEN / 'source_validation.py'
    assert sha(source_path) == ORACLE_SHA256
    spec = importlib.util.spec_from_file_location('parity_v6_oracle', source_path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    original = module.validate
    cases = {}
    provenance = Counter()
    excluded = Counter()

    def capture(source, proposal, *, dictionary=(), completed=True, origin):
        if not completed:
            excluded['incomplete'] += 1
            return
        if not isinstance(source, str) or not isinstance(proposal, str):
            excluded['not_text'] += 1
            return
        try:
            source.encode('utf-8')
            proposal.encode('utf-8')
        except UnicodeEncodeError:
            excluded['malformed_unicode'] += 1
            return
        terms = list(dictionary)
        result = original(source, proposal, dictionary=terms, completed=True)
        key = json.dumps([source, proposal, terms], ensure_ascii=False, separators=(',', ':'))
        case = {'source': source, 'proposal': proposal, 'protected_terms': terms,
                'accepted': result.accepted, 'output': result.output}
        if key in cases:
            assert cases[key] == case
        else:
            cases[key] = case
        provenance[origin] += 1

    def observed(source, proposal, **kwargs):
        result = original(source, proposal, **kwargs)
        capture(source, proposal, **kwargs, origin='authored_tests')
        return result

    module.validate = observed
    log = io.StringIO()
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(module.ValidatorTests)
    result = unittest.TextTestRunner(stream=log, verbosity=2).run(suite)
    assert result.wasSuccessful(), log.getvalue()
    module.validate = original
    profiles = json.loads((FROZEN / 'inputs.json').read_text())['profiles']
    source_files = {'source_validation.py': ORACLE_SHA256, 'inputs.json': sha(FROZEN / 'inputs.json')}
    for profile in profiles:
        path = FROZEN / (profile['name'] + '.json')
        payload = json.loads(path.read_text())
        assert payload['validator_sha256'] == ORACLE_SHA256
        source_files[path.name] = sha(path)
        for row in payload['cases']:
            complete = row['completed'] and row['finish_reason'] == 'stop' and row['error'] is None
            capture(row['source'], row['raw']['output'], dictionary=payload['dictionary'],
                    completed=complete, origin='revealed_profiles')
    metadata = {'oracle_sha256': ORACLE_SHA256, 'python': platform.python_version(),
                'unicode_database': unicodedata.unidata_version,
                'oracle_path': str(FROZEN.relative_to(ROOT) / 'source_validation.py'),
                'profile_count': len(profiles), 'authored_test_groups': result.testsRun,
                'input_invocations': dict(provenance), 'excluded': dict(excluded),
                'unique_cases': len(cases), 'source_files_sha256': source_files,
                'notes': 'Only revealed development proposals and frozen authored tests. No qualification2 data. completed=false, non-text, and malformed Unicode excluded for Rust &str API; work-limit cases retained.'}
    DESTINATION.parent.mkdir(parents=True, exist_ok=True)
    lines = ['{', '  "metadata": ' + json.dumps(metadata, ensure_ascii=False, separators=(',', ':')) + ',', '  "cases": [']
    lines.extend('    '+json.dumps(case, ensure_ascii=False, separators=(',', ':'))+(',' if i+1<len(cases) else '')
                 for i, case in enumerate(cases.values()))
    lines.extend(['  ]', '}'])
    DESTINATION.write_text('\n'.join(lines)+'\n')
    print(json.dumps({'path': str(DESTINATION), 'sha256': sha(DESTINATION), 'bytes': DESTINATION.stat().st_size,
                      'unique_cases': len(cases), 'input_invocations': dict(provenance), 'excluded': dict(excluded)}, indent=2))


if __name__ == '__main__':
    main()
