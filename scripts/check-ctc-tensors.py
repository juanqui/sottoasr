#!/usr/bin/env python3
"""Compare the patched CTC tensor routines with the pinned element-access implementation."""
import argparse
import subprocess
import tempfile
from pathlib import Path


def method(source, name):
    start = source.index('    private func ' + name + '(')
    cursor = source.index('{', start) + 1
    depth = 1
    while depth:
        if source[cursor] == '{':
            depth += 1
        elif source[cursor] == '}':
            depth -= 1
        cursor += 1
    return source[start:cursor].replace('private func ' + name, 'func ' + name, 1)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--sdk-build', type=Path, required=True)
    args = parser.parse_args()
    checkout = args.sdk_build.resolve() / 'checkouts/FluidAudio'
    relative = 'Sources/FluidAudio/ASR/Parakeet/SlidingWindow/CustomVocabulary/WordSpotting/CtcKeywordSpotter+Inference.swift'
    original = subprocess.run(['git', 'show', 'HEAD:' + relative], cwd=checkout,
                              check=True, capture_output=True, text=True).stdout
    patched = (checkout / relative).read_text()
    declarations = 'import CoreML\nimport Foundation\nimport Darwin\nenum ASRError: Error { case processingFailed(String) }\n'
    for name, source in [('Original', original), ('Patched', patched)]:
        declarations += 'struct ' + name + ' { let blankId = 2\n'
        declarations += method(source, 'makeLogProbs') + '\n' + method(source, 'logSoftmax') + '\n}\n'
    # Exercise the actual input fill block, including a deliberately poisoned tail.
    start = patched.index('        // Typed contiguous input access')
    end = patched.index('\n        if debugMode {', start)
    declarations += 'func fill(_ array: MLMultiArray, _ audioSamples: [Float]) {\n'
    declarations += 'let dataType = array.dataType, clampedCount = audioSamples.count, maxModelSamples = array.count\n'
    declarations += patched[start:end] + '\n}\n'
    fixture = Path(__file__).resolve().parents[1] / 'src-tauri/vendor/fluidaudio-rs/tests/CtcTensorProbe.swift'
    with tempfile.TemporaryDirectory(prefix='sotto-ctc-tensor-') as directory:
        directory = Path(directory)
        source, executable = directory / 'main.swift', directory / 'probe'
        source.write_text(declarations + fixture.read_text())
        subprocess.run(['swiftc', '-O', str(source), '-o', str(executable)], check=True)
        subprocess.run([str(executable)], check=True, timeout=60)


if __name__ == '__main__':
    main()
