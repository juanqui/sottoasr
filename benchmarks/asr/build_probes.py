"""Link ASR benchmark probes against a previously built, pinned Swift SDK."""
import argparse
import subprocess
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('output', type=Path)
parser.add_argument('--sdk-build', type=Path, required=True,
                    help='Existing fluidaudio-rs out/swift-build directory')
args = parser.parse_args()
root, build = args.output.resolve(), args.sdk_build.resolve()
archive = build / 'release/libFluidAudioBridge.a'
if not archive.is_file():
    parser.error(f'Pinned SDK archive is missing: {archive}; build Sotto from src-tauri first')
root.mkdir(parents=True, exist_ok=True)
common = ['swiftc', '-O', '-parse-as-library']
for include in [build / 'release/Modules',
                build / 'checkouts/FluidAudio/Sources/FastClusterWrapper/include',
                build / 'checkouts/FluidAudio/Sources/MachTaskSelfWrapper/include',
                build / 'artifacts/fluidaudio/NemoTextProcessing/NemoTextProcessing.xcframework/macos-arm64_x86_64/Headers']:
    common += ['-I', str(include)]
common += ['-L', str(build / 'release'), '-lFluidAudioBridge', '-ltext_processing_rs', '-lc++']
for framework in ['AVFoundation', 'CoreML', 'Accelerate', 'Metal', 'MetalPerformanceShaders']:
    common += ['-framework', framework]
for name in ['vocabulary_probe', 'unified_probe', 'compute_probe']:
    output = root / name
    if output.exists():
        parser.error(f'Refusing to overwrite an existing probe: {output}')
    subprocess.run(common + [str(Path(__file__).with_name(name + '.swift')), '-o', str(output)], check=True)
