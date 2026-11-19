#!/usr/bin/env python3
"""Check or regenerate the bundled SHA-256 manifest of the pinned FluidAudio models.

The manifest lists exactly the files the bridge loads. Large files take their
SHA-256 from the HuggingFace tree API (Git LFS object IDs). Small files are
hashed from their bytes, read from --cache when given or downloaded at the
pinned revision otherwise. Either way their Git blob ID must match the tree, so
the hashes always describe the pinned commit, never a local edit.

Without --write the script exits non-zero when the committed manifest differs.
"""
import argparse
import hashlib
import json
import sys
import urllib.parse
import urllib.request
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
MANIFEST = REPO / 'src-tauri/vendor/fluidaudio-rs/model-manifest.json'
HUB = 'https://huggingface.co'

# Files the bridge loads: every file inside these CoreML bundles, plus these files.
# `previous_speech` is never downloaded again: an installation that has it keeps
# dictating with it until `speech` is downloaded and verified.
SPEECH_BUNDLES = ['Preprocessor.mlmodelc', 'Encoder.mlmodelc', 'Decoder.mlmodelc', 'JointDecisionv3.mlmodelc']
MODELS = {
    'speech': {
        'repository': 'FluidInference/parakeet-ultra-coreml',
        'directory': 'parakeet-ultra',
        'bundles': SPEECH_BUNDLES,
        'files': ['parakeet_vocab.json'],
    },
    'previous_speech': {
        'repository': 'FluidInference/parakeet-tdt-0.6b-v3-coreml',
        'directory': 'parakeet-tdt-0.6b-v3',
        'bundles': SPEECH_BUNDLES,
        'files': ['parakeet_vocab.json'],
    },
    'vocabulary': {
        'repository': 'FluidInference/parakeet-ctc-110m-coreml',
        'directory': 'parakeet-ctc-110m-coreml',
        'bundles': ['MelSpectrogram.mlmodelc', 'AudioEncoder.mlmodelc'],
        'files': ['vocab.json', 'tokenizer.json'],
    },
}


def get(url):
    request = urllib.request.Request(url, headers={'User-Agent': 'SottoASR'})
    with urllib.request.urlopen(request, timeout=60) as response:
        return response.read(), response.headers.get('Link', '')


def tree(repository, revision):
    """Every entry of the commit's tree, following the API's pagination cursor."""
    url, entries = f'{HUB}/api/models/{repository}/tree/{revision}?recursive=1', []
    while url:
        body, link = get(url)
        entries += json.loads(body)
        url = next((part.split(';')[0].strip(' <>') for part in link.split(',') if 'rel="next"' in part), None)
    return entries


def git_blob_id(data):
    return hashlib.sha1(b'blob %d\0' % len(data) + data).hexdigest()


def build(model, revision, cache):
    repository = model['repository']
    entries = {item['path']: item for item in tree(repository, revision) if item.get('type') == 'file'}
    wanted = sorted(path for path in entries
                    if path in model['files'] or any(path.startswith(bundle + '/') for bundle in model['bundles']))
    missing = [path for path in model['files'] if path not in entries]
    missing += [bundle for bundle in model['bundles'] if not any(path.startswith(bundle + '/') for path in wanted)]
    if missing:
        raise SystemExit(f'{repository}@{revision} lacks {", ".join(missing)}')
    files = []
    for path in wanted:
        entry = entries[path]
        local = cache / model['directory'] / path if cache else None
        if 'lfs' in entry:
            sha256 = entry['lfs']['oid']
            if local and hashlib.sha256(local.read_bytes()).hexdigest() != sha256:
                raise SystemExit(f'{local} differs from {repository}@{revision}')
        else:
            if local:
                data = local.read_bytes()
            else:
                data = get(f'{HUB}/{repository}/resolve/{revision}/{urllib.parse.quote(path)}')[0]
            if git_blob_id(data) != entry['oid']:
                raise SystemExit(f'{path} bytes differ from {repository}@{revision}')
            sha256 = hashlib.sha256(data).hexdigest()
        files.append({'path': path, 'size': entry['size'], 'sha256': sha256})
    return {'repository': repository, 'revision': revision, 'directory': model['directory'], 'files': files}


def render(manifest):
    lines = ['{']
    for index, name in enumerate(MODELS):
        model = manifest[name]
        lines.append(f'  "{name}": {{')
        for key in ('repository', 'revision', 'directory'):
            lines.append(f'    "{key}": {json.dumps(model[key])},')
        lines.append('    "files": [')
        lines.append(',\n'.join(f'      {json.dumps(item)}' for item in model['files']))
        lines.append('    ]')
        lines.append('  }' + (',' if index < len(MODELS) - 1 else ''))
    lines.append('}')
    return '\n'.join(lines) + '\n'


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--cache', type=Path,
                        help='FluidAudio Models folder to read small files from (read-only)')
    parser.add_argument('--revision', action='append', default=[], metavar='MODEL=COMMIT',
                        help='pin a model to another commit, for example speech=<40-hex commit>')
    parser.add_argument('--write', action='store_true', help='rewrite the manifest')
    args = parser.parse_args()
    current = json.loads(MANIFEST.read_text()) if MANIFEST.is_file() else {}
    revisions = {name: current.get(name, {}).get('revision') for name in MODELS}
    for value in args.revision:
        name, _, commit = value.partition('=')
        if name not in MODELS or len(commit) != 40:
            parser.error(f'expected MODEL=COMMIT with MODEL in {", ".join(MODELS)}')
        revisions[name] = commit
    if not all(revisions.values()):
        parser.error('no manifest yet; give --revision for every model')
    text = render({name: build(model, revisions[name], args.cache) for name, model in MODELS.items()})
    if args.write:
        MANIFEST.write_text(text)
        print(f'Wrote {MANIFEST.relative_to(REPO)}')
        return 0
    if MANIFEST.is_file() and MANIFEST.read_text() == text:
        print('Model manifest matches the pinned revisions')
        return 0
    print('Model manifest differs from the pinned revisions; rerun with --write and review', file=sys.stderr)
    return 1


if __name__ == '__main__':
    raise SystemExit(main())
