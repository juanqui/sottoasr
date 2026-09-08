import contextlib
import io
import importlib.util
import json
import os
import subprocess
import unittest
from pathlib import Path
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    'check_release_target', Path(__file__).with_name('check-release-target.py')
)
target = importlib.util.module_from_spec(spec)
spec.loader.exec_module(target)


class ReleaseTargetTests(unittest.TestCase):
    def run_guard(self, refs='', release=None, error='gh: Not Found (HTTP 404)', ref_type='branch', ref_name='main'):
        responses = [subprocess.CompletedProcess([], 0, refs, '')]
        responses.append(subprocess.CompletedProcess([], 0 if release else 1,
                         json.dumps(release) if release else '', error))
        with patch.dict(os.environ, {'RELEASE_SHA':'abc123', 'GITHUB_REPOSITORY':'example/app',
                                    'GITHUB_REF_TYPE':ref_type, 'GITHUB_REF_NAME':ref_name}), \
             patch.object(target.subprocess, 'run', side_effect=responses), \
             contextlib.redirect_stdout(io.StringIO()):
            target.main()

    def test_new_release_and_same_commit_retries_are_allowed(self):
        self.run_guard()
        self.run_guard(refs='abc123\trefs/tags/v0.8.3\n')
        self.run_guard(refs='tagobject\trefs/tags/v0.8.3\nabc123\trefs/tags/v0.8.3^{}\n')
        self.run_guard(release={'target_commitish':'abc123'})

    def test_different_source_cannot_replace_published_or_draft_release(self):
        for options in [{'refs':'other\trefs/tags/v0.8.3\n'}, {'release':{'target_commitish':'other'}}]:
            with self.subTest(options=options), self.assertRaises(SystemExit):
                self.run_guard(**options)

    def test_metadata_failure_and_mismatched_version_tag_fail_closed(self):
        with self.assertRaises(SystemExit):
            self.run_guard(error='gh: Bad credentials (HTTP 401)')
        with self.assertRaises(SystemExit):
            self.run_guard(ref_type='tag', ref_name='v999.0.0')


if __name__ == '__main__':
    unittest.main()
