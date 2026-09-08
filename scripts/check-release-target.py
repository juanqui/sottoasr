#!/usr/bin/env python3
"""Refuse to replace versioned release artifacts with a different source commit."""
import json
import os
import subprocess
from pathlib import Path


def main():
    version = json.loads(Path('src-tauri/tauri.conf.json').read_text())['version']
    tag = 'v' + version
    expected = os.environ['RELEASE_SHA']
    if os.environ.get('GITHUB_REF_TYPE') == 'tag' and os.environ.get('GITHUB_REF_NAME') != tag:
        raise SystemExit('Release tag does not match the application version')
    refs = subprocess.run(['git', 'ls-remote', 'origin', f'refs/tags/{tag}', f'refs/tags/{tag}^{{}}'],
                          check=True, capture_output=True, text=True).stdout.splitlines()
    if refs:
        # Annotated tags have a peeled commit in ^{}; lightweight tags use the ref.
        commit = next((line.split()[0] for line in refs if line.endswith('^{}')), refs[0].split()[0])
        if commit != expected:
            raise SystemExit(f'{tag} already points to another commit; bump the version before releasing')
    else:
        # A draft can exist before GitHub materializes its tag. Protect it too.
        result = subprocess.run(['gh', 'api', f'repos/{os.environ["GITHUB_REPOSITORY"]}/releases/tags/{tag}'],
                                capture_output=True, text=True)
        if result.returncode == 0:
            if json.loads(result.stdout)['target_commitish'] != expected:
                raise SystemExit(f'{tag} draft targets another commit; bump the version before releasing')
        elif 'HTTP 404' not in result.stderr:
            raise SystemExit('Could not verify existing release metadata')
    print(f'Release target verified: {tag} at {expected}')


if __name__ == '__main__':
    main()
