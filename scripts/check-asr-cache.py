#!/usr/bin/env python3
"""Test the actual pinned-v3 loader's non-destructive cache failures offline.

Optional --model-dir and --audio clone the supplied model into private temporary
storage for one real synthetic transcription. Never mutates the supplied cache.
"""
import argparse
import subprocess
import tempfile
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--model-dir', type=Path)
parser.add_argument('--audio', type=Path)
args = parser.parse_args()
if bool(args.model_dir) != bool(args.audio):
    parser.error('--model-dir and --audio must be supplied together')
repo = Path(__file__).resolve().parents[1]
bridge = repo / 'src-tauri/vendor/fluidaudio-rs'
builds = [path for profile in ('debug', 'release')
          for path in (repo / 'src-tauri/target' / profile / 'build').glob('fluidaudio-rs-*/out/swift-build')
          if (path / 'release/libFluidAudioBridge.a').is_file()]
if not builds:
    parser.error('Run cargo build from src-tauri before this probe')
build = max(builds, key=lambda path: (path / 'release/libFluidAudioBridge.a').stat().st_mtime)
with tempfile.TemporaryDirectory(prefix='sotto-asr-cache-probe-') as scratch:
    root = Path(scratch)
    executable = root / 'asr-cache-probe'
    command = ['swiftc', '-O', '-parse-as-library']
    for include in [build / 'release/Modules',
                    build / 'checkouts/FluidAudio/Sources/FastClusterWrapper/include',
                    build / 'checkouts/FluidAudio/Sources/MachTaskSelfWrapper/include',
                    build / 'artifacts/fluidaudio/NemoTextProcessing/NemoTextProcessing.xcframework/macos-arm64_x86_64/Headers']:
        command.extend(['-I', str(include)])
    command.extend(['-L', str(build / 'release'), '-lFluidAudioBridge', '-ltext_processing_rs', '-lc++'])
    for framework in ['AVFoundation', 'CoreML', 'Accelerate', 'Metal', 'MetalPerformanceShaders']:
        command.extend(['-framework', framework])
    command.extend([str(bridge / 'swift/AsrCache.swift'), str(bridge / 'tests/AsrCacheProbe.swift'), '-o', str(executable)])
    subprocess.run(command, check=True)
    run = [str(executable)]
    if args.model_dir:
        clone = root / 'parakeet-tdt-0.6b-v3'
        subprocess.run(['cp', '-cR', str(args.model_dir.resolve()), str(clone)], check=True)
        run.extend([str(clone), str(args.audio.resolve())])
    subprocess.run(run, check=True, timeout=120)
