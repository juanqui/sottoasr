#!/usr/bin/env python3
"""Test the bridge's speech-model loader for non-destructive cache failures, offline.

Optional --model-dir and --audio clone the supplied model folder (parakeet-ultra,
or the previous parakeet-tdt-0.6b-v3) into private temporary storage for one real
synthetic transcription. Never mutates the supplied cache.
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
# SwiftPM's native build system writes modules to release/Modules (release is
# a symlink to arm64-apple-macosx/release). The Swift Build system (Swift 6.4
# here) points release at out/Products/Release and writes them there directly.
# Only what release resolves to is current: a stale arm64-apple-macosx tree can
# sit beside it.
modules = [path for path in (build / 'release/Modules', build / 'release')
           if (path / 'FluidAudio.swiftmodule').exists()]
if not modules:
    parser.error(f'No FluidAudio module in {build / "release"}; rebuild with cargo build')
with tempfile.TemporaryDirectory(prefix='sotto-asr-cache-probe-') as scratch:
    root = Path(scratch)
    executable = root / 'asr-cache-probe'
    command = ['swiftc', '-O', '-parse-as-library']
    for include in [*modules,
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
        # The folder name selects the model version, so the clone keeps it.
        clone = root / args.model_dir.resolve().name
        subprocess.run(['cp', '-cR', str(args.model_dir.resolve()), str(clone)], check=True)
        run.extend([str(clone), str(args.audio.resolve())])
    subprocess.run(run, check=True, timeout=120)
