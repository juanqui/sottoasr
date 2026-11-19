"""Supply-chain rules for the GitHub workflows: pinned actions, no workflow- or job-wide secrets."""
import re
import unittest
from pathlib import Path

WORKFLOW_DIR = Path(__file__).resolve().parent.parent / '.github' / 'workflows'
WORKFLOWS = sorted(WORKFLOW_DIR.glob('*.yml')) + sorted(WORKFLOW_DIR.glob('*.yaml'))
KEY_LINE = re.compile(r'^(?P<indent> *)(?P<dash>(?:- +)*)(?P<key>[A-Za-z0-9_-]+):(?:[ \t]+(?P<value>.*))?$')
BLOCK_SCALAR = re.compile(r'^[|>][-+0-9]*$')
PINNED = re.compile(r'@[0-9a-f]{40}$')
SECRET = re.compile(r'\bsecrets\s*[.\[]')
USES_MENTION = re.compile(r'\buses\s*:')

SAMPLE = '''\
jobs:
  build:
    env:
      TOKEN: ${{ secrets.TOKEN }}
    steps:
      - name: Sign
        env:
          KEY: ${{ secrets.KEY }}
        run: |
          echo "uses: actions/checkout@v5"
      - uses: actions/checkout@v5 # v5
      - { uses: actions/setup-node@v5 }
'''


def strip_comment(value):
    return re.split(r'(?:^|\s)#', value, maxsplit=1)[0].strip()


def parse(text):
    """Yield (line number, key path, line, value) for each line that belongs to a key.

    Covers the block-style YAML the workflows use: nested mappings, sequences of
    mappings and block scalars. Block-scalar lines carry their key's path and a
    value of None.
    """
    stack = []  # (column, key) from the outermost key to the current one
    block_column = None
    for number, line in enumerate(text.splitlines(), 1):
        stripped = line.strip()
        indent = len(line) - len(line.lstrip(' '))
        if block_column is not None:
            if not stripped or indent > block_column:
                yield number, tuple(key for _, key in stack), line, None
                continue
            block_column = None
        if not stripped or stripped.startswith('#'):
            continue
        match = KEY_LINE.match(line)
        if not match:
            continue
        column = len(match['indent']) + len(match['dash'])
        while stack and stack[-1][0] >= column:
            stack.pop()
        stack.append((column, match['key']))
        value = strip_comment(match['value'] or '')
        yield number, tuple(key for _, key in stack), line, value
        if BLOCK_SCALAR.match(value):
            block_column = column


def uses_refs(text):
    return [(number, value.strip('\'"')) for number, path, _, value in parse(text)
            if value is not None and path[-1] == 'uses']


def unparsed_uses(text):
    """Lines outside block scalars that mention `uses:` but were not parsed as a `uses` value."""
    entries = list(parse(text))
    block_lines = {number for number, _, _, value in entries if value is None}
    parsed = {number for number, path, _, value in entries if value is not None and path[-1] == 'uses'}
    known = block_lines | parsed
    return [number for number, line in enumerate(text.splitlines(), 1)
            if USES_MENTION.search(strip_comment(line)) and number not in known]


def job_wide_env_lines(text):
    """Lines inside a workflow-level `env` or a job-level `env`, which every step can read."""
    return [(number, line) for number, path, line, _ in parse(text)
            if path[:1] == ('env',) or (len(path) >= 3 and path[0] == 'jobs' and path[2] == 'env')]


def exposed_secrets(text):
    return [number for number, line in job_wide_env_lines(text) if SECRET.search(line)]


class WorkflowPolicyTests(unittest.TestCase):
    def test_parser_separates_job_env_from_step_env_and_scripts(self):
        self.assertEqual(exposed_secrets(SAMPLE), [4])
        self.assertEqual(uses_refs(SAMPLE), [(11, 'actions/checkout@v5')])
        self.assertEqual(unparsed_uses(SAMPLE), [12])

    def test_every_action_is_pinned_to_a_commit_sha(self):
        self.assertGreaterEqual(len(WORKFLOWS), 2)
        for workflow in WORKFLOWS:
            with self.subTest(workflow=workflow.name):
                text = workflow.read_text()
                refs = uses_refs(text)
                self.assertTrue(refs, 'no uses: found')
                self.assertEqual(unparsed_uses(text), [], 'write uses: as a block mapping key so it can be checked')
                unpinned = [f'{workflow.name}:{number}: {ref}' for number, ref in refs
                            if not PINNED.search(ref)]
                self.assertEqual(unpinned, [], 'pin these actions to a 40-character commit SHA')

    def test_no_secret_is_declared_for_a_whole_workflow_or_job(self):
        for workflow in WORKFLOWS:
            with self.subTest(workflow=workflow.name):
                exposed = [f'{workflow.name}:{number}' for number in exposed_secrets(workflow.read_text())]
                self.assertEqual(exposed, [], 'pass secrets only to the steps that use them')


if __name__ == '__main__':
    unittest.main()
