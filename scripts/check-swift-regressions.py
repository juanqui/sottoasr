#!/usr/bin/env python3
"""Run numerical and alignment regressions against the SDK produced by Cargo."""
import argparse
import subprocess
import sys
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target-dir', type=Path, default=Path('src-tauri/target'))
    args = parser.parse_args()
    builds = sorted(args.target_dir.glob('debug/build/fluidaudio-rs-*/out/swift-build'),
                    key=lambda path: path.stat().st_mtime, reverse=True)
    if not builds:
        raise SystemExit('Build the default CoreML backend before running Swift regressions')
    source = 'Sources/FluidAudio/ASR/Parakeet/SlidingWindow/CustomVocabulary/WordSpotting/CtcKeywordSpotter+Inference.swift'
    builds = [path for path in builds if (path / 'checkouts/FluidAudio' / source).is_file()]
    if not builds:
        raise SystemExit('The Cargo build has no resolved FluidAudio source')
    # Cargo's current build is the newest output; older toolchain outputs can coexist.
    build = builds[0]
    for script in ('check-alignment.py', 'check-ctc-tensors.py'):
        subprocess.run([sys.executable, str(Path(__file__).with_name(script)),
                        '--sdk-build', str(build)], check=True)


if __name__ == '__main__':
    main()
