#!/usr/bin/env python3
"""Explicit local release smoke, using an actual .app resource and Rust guard.

No downloads, settings/history writes, clipboard access, or automatic retries.
This is a revealed development smoke, not independent model qualification.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import selectors
import subprocess
import sys
import time

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
MAX_LINE = 256 * 1024
MODEL = "openbmb/MiniCPM5-2B-MLX"
REVISION = "32f8dd5df1188512a20413f1297083238306634c"
PROMPT_HASH = "2edd80834efc831c1f7d37f93da35c209622525b39dcc766c01159f6ad87de7f"
PROMPT_FILE = HERE.parent / "model-study-2026-09-08/direct-cleanup-diagnostic/development/d5-restart/prompt-d7.json"


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def lexical(text):
    # Reporting only. Protected source payloads are checked separately in bytes.
    return re.findall(r"[^\W_]+(?:['’][^\W_]+)*", text.casefold())


class Child:
    def __init__(self, command, stderr, environment):
        self.process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=stderr, env=environment, bufsize=0)
        os.set_blocking(self.process.stdin.fileno(), False)
        os.set_blocking(self.process.stdout.fileno(), False)
        self.buffer = bytearray()

    def request(self, value, timeout):
        started = time.perf_counter()
        deadline = started + timeout
        line = (json.dumps(value, ensure_ascii=False, separators=(",", ":")) + "\n").encode()
        if len(line) > MAX_LINE:
            raise ValueError("smoke request exceeds protocol bound")
        with selectors.DefaultSelector() as selector:
            selector.register(self.process.stdin, selectors.EVENT_WRITE)
            position = 0
            while position < len(line):
                remaining = deadline - time.perf_counter()
                if remaining <= 0 or not selector.select(remaining):
                    raise TimeoutError("sidecar request write deadline")
                try:
                    position += os.write(self.process.stdin.fileno(), line[position:])
                except BlockingIOError:
                    continue
            selector.unregister(self.process.stdin)
            selector.register(self.process.stdout, selectors.EVENT_READ)
            while b"\n" not in self.buffer:
                remaining = deadline - time.perf_counter()
                if remaining <= 0 or not selector.select(remaining):
                    raise TimeoutError("sidecar response deadline")
                try:
                    data = os.read(self.process.stdout.fileno(), min(65536, MAX_LINE + 1 - len(self.buffer)))
                except BlockingIOError:
                    continue
                if not data:
                    raise RuntimeError("sidecar response ended early")
                self.buffer.extend(data)
                if len(self.buffer) > MAX_LINE:
                    raise RuntimeError("sidecar response exceeds protocol bound")
        raw_line, _, remaining = self.buffer.partition(b"\n")
        self.buffer = bytearray(remaining)
        return json.loads(raw_line.decode("utf-8")), time.perf_counter() - started

    def close(self):
        if self.process.poll() is None:
            try:
                self.request({"action": "quit"}, 1)
                self.process.wait(timeout=1)
            except Exception:
                self.process.kill()
                self.process.wait(timeout=3)
        self.process.stdin.close()
        self.process.stdout.close()


def pure_checks(sidecar):
    """Importing this resource must not load MLX or contact the model hub."""
    spec = importlib.util.spec_from_file_location("bundled_cleanup_smoke", sidecar)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    if "mlx.core" in sys.modules:
        raise AssertionError("sidecar import unexpectedly loads inference")
    assert module.MODEL_ID == MODEL and module.MODEL_REVISION == REVISION
    assert module.PROMPT_SHA256 == PROMPT_HASH == digest(PROMPT_FILE)
    assert module.PROMPT == json.loads(PROMPT_FILE.read_text())
    assert module.parse_generation("", "stop") == ""
    assert module.parse_generation("Complete tail 859.", "stop") == "Complete tail 859."
    checks = []
    for output, finish in [("partial", "length"), ("partial", None), ("partial", "error"),
                           ("é" * 32000, "stop"), ("\ud800", "stop")]:
        try:
            module.parse_generation(output, finish)
        except module.CleanupError as error:
            checks.append(error.code)
        else:
            raise AssertionError("incomplete/invalid proposal accepted")
    return {"model_id": MODEL, "revision": REVISION, "prompt_sha256": PROMPT_HASH,
            "empty_completed_output_supported": True, "rejected_cases": checks,
            "runtime_pins": module.RUNTIME_PINS}


def adapter_request(adapter, request, environment):
    result = subprocess.run([str(adapter)], input=json.dumps(request) + "\n", text=True,
                            capture_output=True, timeout=5, check=True, env=environment)
    return json.loads(result.stdout)


def protected_ok(case, output):
    # Author-provided spans are exact source bytes; preserve payload count/order.
    offset = 0
    for span in case.get("protected_spans", []):
        payload = span["text"]
        assert case["raw"].encode()[span["start_byte"]:span["end_byte"]] == payload.encode()
        position = output.find(payload, offset)
        if position < 0:
            return False
        offset = position + len(payload)
    return True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bundle", type=Path, required=True)
    parser.add_argument("--python", type=Path, required=True)
    parser.add_argument("--adapter", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--execute", action="store_true", help="explicitly run local inference")
    args = parser.parse_args()
    sidecar = args.bundle.resolve() / "Contents/Resources/sidecar/llm_cleanup.py"
    if not sidecar.is_file():
        raise SystemExit(f"Bundled resource missing: {sidecar}")
    report = {"kind": "revealed_release_smoke_not_qualification", "bundle": str(args.bundle.resolve()),
              "sidecar_sha256": digest(sidecar), "python": str(args.python.resolve()),
              "pure_checks": pure_checks(sidecar), "cases_sha256": digest(HERE / "cases.json"),
              "validator_source_sha256": digest(REPO / "src-tauri/src/llm/validation.rs"),
              "cases": [], "limitations": "Does not exercise live capture, clipboard, focus or paste. Source validator is structural, not a semantic guarantee. Prior qualification failed."}
    if not args.execute:
        report["inference"] = "not_requested"
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
        print(json.dumps({"pure_checks": "passed", "inference": "not_requested"}))
        return
    environment = os.environ.copy()
    environment.update({"HF_HUB_OFFLINE": "1", "TRANSFORMERS_OFFLINE": "1", "HF_HUB_DISABLE_TELEMETRY": "1",
                        "PYTHONDONTWRITEBYTECODE": "1", "TOKENIZERS_PARALLELISM": "false"})
    # Do not forward cloud credentials, and never send a download action.
    environment.pop("HF_TOKEN", None)
    environment.pop("HUGGING_FACE_HUB_TOKEN", None)
    versions = subprocess.run([str(args.python), "-c", "import importlib.metadata,json,sys;print(json.dumps({'python':sys.version,'packages':{n:importlib.metadata.version(n) for n in ['mlx','mlx-lm','transformers','huggingface-hub']}}))"],
                              capture_output=True, text=True, check=True, timeout=5, env=environment)
    report["runtime"] = json.loads(versions.stdout)
    assert report["runtime"]["packages"] == report["pure_checks"]["runtime_pins"]
    args.out.parent.mkdir(parents=True, exist_ok=True)
    stderr_path = args.out.with_suffix(".stderr.txt")
    child = None
    failure = None
    try:
        with stderr_path.open("wb") as stderr:
            child = Child([str(args.python), str(sidecar)], stderr, environment)
            status, report["status_latency_s"] = child.request({"action": "status"}, 5)
            report["status"] = status
            assert status.get("ok") and status.get("downloaded")
            assert status.get("model_id") == MODEL and status.get("local_revision") == REVISION
            assert status.get("prompt_sha256") == PROMPT_HASH
            assert status.get("loaded") is False and status.get("warmed") is False
            loaded, report["cold_load_and_warm_latency_s"] = child.request({"action": "load"}, 15)
            report["load"] = loaded
            assert loaded.get("ok") and loaded.get("model_id") == MODEL and loaded.get("revision") == REVISION
            assert loaded.get("prompt_sha256") == PROMPT_HASH
            assert loaded.get("warmed") is True and loaded.get("did_warm") is True
            warm_status, report["warm_status_latency_s"] = child.request({"action": "status"}, 5)
            report["warm_status"] = warm_status
            assert warm_status.get("ok") and warm_status.get("loaded") is True and warm_status.get("warmed") is True
            assert warm_status.get("model_id") == MODEL and warm_status.get("local_revision") == REVISION
            assert warm_status.get("prompt_sha256") == PROMPT_HASH
            repeated, report["repeat_load_latency_s"] = child.request({"action": "load"}, 15)
            report["repeat_load"] = repeated
            assert repeated.get("ok") and repeated.get("model_id") == MODEL and repeated.get("revision") == REVISION
            assert repeated.get("prompt_sha256") == PROMPT_HASH
            assert repeated.get("warmed") is True and repeated.get("did_warm") is False
            # Reuse is asserted by did_warm, with a generous local no-op latency bound.
            assert report["repeat_load_latency_s"] < 1.0, "resident load took at least one second"
            for case in json.loads((HERE / "cases.json").read_text())["cases"]:
                raw = case["raw"]
                admission = adapter_request(args.adapter, {"action": "admit", "source": raw}, environment)
                row = {"id": case["id"], "raw": raw, "expected": case["expected"], **admission}
                if admission["language_skip"]:
                    row.update({"response": None, "latency_s": 0, "delivered": raw, "accepted": False})
                else:
                    response, elapsed = child.request({"action": "cleanup", "text": raw}, 15)
                    row.update({"response": response, "latency_s": elapsed})
                    report.setdefault("first_user_request_latency_s", elapsed)
                    completed = (response.get("ok") is True and response.get("finish_reason") == "stop"
                                 and isinstance(response.get("text"), str) and response.get("elapsed_ms", 10001) <= 10000)
                    row["completed"] = completed
                    if completed:
                        validated = adapter_request(args.adapter, {"source": raw, "proposal": response["text"],
                            "protected_terms": case.get("protected_terms", [])}, environment)
                        row.update({"validation": validated, "delivered": validated["output"], "accepted": validated["accepted"]})
                    else:
                        row.update({"delivered": raw, "accepted": False})
                choices = case.get("acceptable_outputs", [case["expected"]])
                row["exact_target"] = row["delivered"] in choices
                row["lexical_target"] = any(lexical(row["delivered"]) == lexical(choice) for choice in choices)
                row["full_source_fallback"] = row["delivered"] == raw
                row["protected_payloads_preserved"] = protected_ok(case, row["delivered"])
                row["smoke_pass"] = row["protected_payloads_preserved"] and (row["lexical_target"] or row["full_source_fallback"])
                if case["id"] == "reported_sentence":
                    row["smoke_pass"] = row["exact_target"]
                report["cases"].append(row)
                print(json.dumps({"id": case["id"], "smoke_pass": row["smoke_pass"], "latency_s": row["latency_s"]}), flush=True)
    except Exception as error:
        failure = type(error).__name__ + ": " + str(error)
        report["error"] = failure
    finally:
        if child is not None:
            child.close()
            report["child_exit_code"] = child.process.returncode
        report["summary"] = {"completed_cases": len(report["cases"]), "smoke_passes": sum(c["smoke_pass"] for c in report["cases"]),
            "exact_targets": sum(c["exact_target"] for c in report["cases"]), "lexical_targets": sum(c["lexical_target"] for c in report["cases"]),
            "source_fallbacks": sum(c["full_source_fallback"] for c in report["cases"]), "language_skips": sum(c["language_skip"] for c in report["cases"])}
        args.out.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    if failure or report["summary"]["completed_cases"] != 15 or report["summary"]["smoke_passes"] != 15:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
