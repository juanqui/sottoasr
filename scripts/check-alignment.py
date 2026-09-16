"""Exercise the exact patched Swift alignment and compare it with the pinned original."""
import argparse
import subprocess
import tempfile
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--sdk-build', type=Path, required=True)
parser.add_argument('--reproduce-original', action='store_true')
args = parser.parse_args()
checkout = args.sdk_build.resolve() / 'checkouts/FluidAudio'
source = 'Sources/FluidAudio/ASR/Parakeet/SlidingWindow/CustomVocabulary/Rescorer/VocabularyRescorer+Utilities.swift'
original = subprocess.run(['git', 'show', f'HEAD:{source}'], cwd=checkout, check=True, capture_output=True, text=True).stdout
patched = (checkout / source).read_text()

def alignment_body(text):
    start = text.index('    static func alignBaseWordsToUTF8Ranges(')
    end = text.index('    /// Build one candidate span', start)
    helpers = text.index('    private static func lexicalUTF8Range(')
    helpers_end = text.index('    /// Build set of normalized vocabulary terms', helpers)
    return text[start:end] + text[helpers:helpers_end]

fixture = Path(__file__).resolve().parents[1] / 'src-tauri/vendor/fluidaudio-rs/tests/AlignmentProbe.swift'
swift = 'import Foundation\nimport Darwin\nstruct Original {\n' + alignment_body(original) + '}\nstruct Patched {\n' + alignment_body(patched) + '}\n' + fixture.read_text()
with tempfile.TemporaryDirectory(prefix='sotto-alignment-') as directory:
    directory = Path(directory)
    code, binary = directory / 'main.swift', directory / 'alignment'
    code.write_text(swift)
    subprocess.run(['swiftc', '-O', str(code), '-o', str(binary)], check=True)
    if args.reproduce_original:
        result = subprocess.run([str(binary), '--original'], capture_output=True, timeout=60)
        if result.returncode not in (-10, -11):
            raise SystemExit(f'Expected stack-guard signal, got {result.returncode}')
        print(f'Original recursive alignment reproduced stack crash (signal {-result.returncode})')
    subprocess.run([str(binary)], check=True, timeout=120)
