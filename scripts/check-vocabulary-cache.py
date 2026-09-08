#!/usr/bin/env python3
"""Run the actual Swift tokenizer-repair helper against offline fault fixtures.

Requires an existing cargo build; reuses its pinned FluidAudio static library.
Never reads or changes the installed model cache.
"""
import subprocess
import tempfile
from pathlib import Path

repo = Path(__file__).resolve().parents[1]
bridge = repo / "src-tauri/vendor/fluidaudio-rs"
builds = [path for profile in ("debug", "release")
          for path in (repo / "src-tauri/target" / profile / "build").glob("fluidaudio-rs-*/out/swift-build")
          if (path / "release/libFluidAudioBridge.a").is_file()]
if not builds:
    raise SystemExit("Run cargo build from src-tauri before this probe.")
build = max(builds, key=lambda path: (path / "release/libFluidAudioBridge.a").stat().st_mtime)
with tempfile.TemporaryDirectory(prefix="sotto-cache-probe-") as scratch:
    executable = Path(scratch) / "vocabulary-cache-probe"
    args = ["swiftc", "-O", "-parse-as-library"]
    includes = [build / "release/Modules",
                build / "checkouts/FluidAudio/Sources/FastClusterWrapper/include",
                build / "checkouts/FluidAudio/Sources/MachTaskSelfWrapper/include",
                build / "artifacts/fluidaudio/NemoTextProcessing/NemoTextProcessing.xcframework/macos-arm64_x86_64/Headers"]
    for include in includes:
        args.extend(["-I", str(include)])
    args.extend(["-L", str(build / "release"), "-lFluidAudioBridge", "-ltext_processing_rs"])
    for framework in ("AVFoundation", "CoreML", "Accelerate", "Metal", "MetalPerformanceShaders"):
        args.extend(["-framework", framework])
    args.extend(["-lc++", str(bridge / "swift/VocabularyCache.swift"),
                 str(bridge / "tests/VocabularyCacheProbe.swift"), "-o", str(executable)])
    subprocess.run(args, check=True)
    subprocess.run([str(executable)], check=True)
