#!/usr/bin/env python3
"""Synthetic macOS ASR tail regression; no microphone or private recordings.

Run from any directory: python3 scripts/check-asr-tail.py
Requires macOS Samantha voice, Xcode, Rust, and the FluidAudio model cache.
The SDK downloads missing model artifacts through the same path as the app.
"""

import array
import json
import re
import subprocess
import sys
import tempfile
import wave
from pathlib import Path

BODY = (
    "This is a recording test for our local speech recognition application. "
    "Every sentence should appear in the final transcript, even when the audio "
    "crosses a processing boundary. We need reliable dictation so that every "
    "instruction remains intact."
)
TAIL = (
    "The final instruction is to keep the blue notebook beside the window "
    "and remember the silver telescope."
)
RATE = 16_000


def words(text):
    return re.findall(r"[a-z0-9]+", text.lower())


def speak(directory, name, text, rate):
    source = directory / f"{name}.txt"
    source.write_text(text)
    destination = directory / f"{name}.wav"
    subprocess.run([
        "say", "-v", "Samantha", "-r", str(rate), "-f", str(source),
        "-o", str(destination), "--data-format=LEI16@16000",
    ], check=True)
    with wave.open(str(destination), "rb") as wav:
        samples = array.array("h", wav.readframes(wav.getnframes()))
    if sys.byteorder != "little":
        samples.byteswap()
    return samples


def main():
    root = Path(__file__).resolve().parents[1]
    # Only this newly created synthetic directory is removed at exit.
    with tempfile.TemporaryDirectory(prefix="sotto-asr-tail-") as temporary:
        directory = Path(temporary)
        body = speak(directory, "body", " ".join([BODY] * 3), 155)
        tail = speak(directory, "tail", TAIL, 150)
        cases = [
            ("normal", 1, 1, 1, .75),
            ("quiet", .03, .03, 1, .75),
            ("quiet_tail", 1, .03, 1, .75),
            ("quiet_no_pad", .03, .03, 1, 0),
        ]
        cases += [
            (f"gain_{gain}_pause_{pause}", gain, gain, pause, .75)
            for pause in [0, 2, 4, 6, 8, 10] for gain in [.02, .006]
        ]
        paths, durations = [], {}
        for name, body_gain, tail_gain, pause, padding in cases:
            data = array.array("h", (
                [int(value * body_gain) for value in body] + [0] * int(RATE * pause)
                + [int(value * tail_gain) for value in tail] + [0] * int(RATE * padding)
            ))
            path = directory / f"{name}.wav"
            durations[str(path)] = len(data) / RATE
            if sys.byteorder != "little":
                data.byteswap()
            with wave.open(str(path), "wb") as wav:
                wav.setparams((1, 2, RATE, 0, "NONE", "not compressed"))
                wav.writeframes(data.tobytes())
            paths.append(str(path))
        run = subprocess.run([
            "cargo", "run", "--quiet", "--example", "asr_fixture", "--", *paths,
        ], cwd=root / "src-tauri", text=True, capture_output=True)
        if run.stderr:
            print(run.stderr, file=sys.stderr, end="")
        if run.returncode:
            print(run.stdout, end="")
            return run.returncode
        results = [json.loads(line) for line in run.stdout.splitlines() if line.startswith('{"duration_secs"')]
        if {item["path"] for item in results} != set(paths):
            raise RuntimeError("The runner did not return exactly the expected audio fixtures")
        failures = []
        for result in results:
            text_words = words(result["text"])
            tail_ok = text_words[-len(words(TAIL)):] == words(TAIL)
            body_ok = text_words.count("instruction") == 4
            duration_ok = abs(result["duration_secs"] - durations[result["path"]]) < 1 / RATE
            passed = tail_ok and body_ok and duration_ok
            print(json.dumps({
                "fixture": Path(result["path"]).name, "passed": passed,
                "tail_preserved": tail_ok, "body_repeats_preserved": body_ok,
                "duration_correct": duration_ok, "processing_secs": result["processing_secs"],
            }))
            if not passed:
                failures.append(Path(result["path"]).name)
        print(f"{len(results) - len(failures)}/{len(results)} fixtures passed")
        return int(bool(failures))


if __name__ == "__main__":
    raise SystemExit(main())
