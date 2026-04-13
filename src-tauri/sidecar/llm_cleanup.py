#!/usr/bin/env python3
"""SottoASR LLM Cleanup Sidecar — runs fine-tuned LFM2.5-350M via MLX.

Protocol: reads JSON requests from stdin (one per line), writes JSON responses to stdout.

Request format:
  {"action": "cleanup", "text": "raw transcript"}
  {"action": "status"}
  {"action": "download"}
  {"action": "load"}
  {"action": "quit"}

Response format:
  {"ok": true, "text": "cleaned transcript", "elapsed_ms": 123, "tokens": 45}
  {"ok": true, "status": "ready"|"not_downloaded"|"downloaded", ...}
  {"ok": false, "error": "message"}
"""

import json
import sys
import time
import traceback

MODEL_ID = "juanquivilla/sotto-cleanup-lfm25-350m-mlx-5bit"

_model = None
_tokenizer = None
_sampler = None


def log(msg):
    """Log to stderr (stdout is reserved for JSON protocol)."""
    print(f"[llm_cleanup] {msg}", file=sys.stderr, flush=True)


def _format_exception(e):
    """Return exception type, message, and single-line traceback for protocol."""
    tb = traceback.format_exc()
    return f"{type(e).__name__}: {e}\n{tb}"


def load_model():
    """Load and warm up the model. Returns (True, None) on success or
    (False, error_string) on failure. The error string includes the exception
    type, message, and full traceback so callers see real diagnostics."""
    global _model, _tokenizer, _sampler
    if _model is not None:
        return True, None

    try:
        import gc
        import mlx.core as mx
        from mlx_lm import load, generate
        from mlx_lm.sample_utils import make_sampler

        # Cap Metal memory to prevent system hang on machines with limited RAM.
        # Without limits, MLX reserves up to 75% of system RAM as wired
        # (non-swappable) memory, which can exhaust unified memory and hang
        # the machine. See: ml-explore/mlx-lm#883, ml-explore/mlx-lm#1015
        mx.set_memory_limit(1024 * 1024 * 1024)   # 1GB soft limit
        mx.set_cache_limit(128 * 1024 * 1024)      # 128MB Metal buffer cache
        log("MLX memory limits set: 1GB memory, 128MB cache")

        log(f"Loading {MODEL_ID}...")
        _model, _tokenizer = load(MODEL_ID)
        _sampler = make_sampler(temp=0.0)  # Greedy — deterministic output
        log("Model loaded, running warmup inference...")
        # Warmup: trigger MLX lazy graph compilation so first real request is fast
        _warmup_output = generate(
            _model,
            _tokenizer,
            prompt="### Input:\nhello\n\n### Output:\n",
            max_tokens=8,
            sampler=_sampler,
            verbose=False,
        )
        # Release warmup temporaries from the Metal buffer cache
        mx.clear_cache()
        gc.collect()
        log("Model loaded and warmed up successfully")
        return True, None
    except Exception as e:
        err = _format_exception(e)
        log(f"Failed to load model: {err}")
        return False, err


def cleanup_chunk(text):
    """Clean a single chunk of transcript text (up to ~200 words)."""
    import mlx.core as mx
    from mlx_lm import generate  # Already imported/cached by load_model()

    # Clear stale Metal buffers before inference to prevent cache cascade OOM.
    # See: ml-explore/mlx-lm#1015
    mx.clear_cache()

    prompt = f"### Input:\n{text}\n\n### Output:\n"
    input_words = len(text.split())
    # Budget formula rationale (see docs/specs/2026-04-11-llm-cleanup-reliability.md §4.2):
    #   - LFM2.5 tokenizer averages ~1.24 tokens/word on SottoASR cleanup text
    #   - Cleaned output is typically the same word count as input, so output
    #     tokens ≈ 1.24 × input_words
    #   - The 2.5× multiplier gives 100% safety margin for expansions
    #     (corrections, dictation→punctuation, spelled-out acronyms, etc.)
    #   - Floor 4096 ensures short inputs never run out of budget
    #   - Ceiling 16384 prevents runaway generation on broken outputs while
    #     staying well inside the 32K trained seq_len. At 1.24 tok/word that
    #     allows up to ~13,200 clean words ≈ 15 min of speech at 150 WPM.
    max_output_tokens = min(16384, max(4096, int(input_words * 2.5)))

    output = generate(
        _model,
        _tokenizer,
        prompt=prompt,
        max_tokens=max_output_tokens,
        sampler=_sampler,
        verbose=False,
    )

    # Release inference temporaries
    mx.clear_cache()

    output = output.strip()
    if "###" in output:
        output = output[:output.index("###")].strip()

    return output


def cleanup_text(text):
    """Clean up transcript text using the fine-tuned model."""
    if _model is None or _tokenizer is None:
        return None, "Model not loaded"

    start = time.perf_counter()
    output = cleanup_chunk(text)
    elapsed = time.perf_counter() - start
    return output, None, elapsed


def get_local_revision():
    """Get the locally cached model revision (commit hash), or None."""
    try:
        from huggingface_hub import scan_cache_dir
        cache = scan_cache_dir()
        for repo in cache.repos:
            if repo.repo_id == MODEL_ID:
                # Sort by last_modified descending — frozenset has no guaranteed order
                revisions = sorted(repo.revisions, key=lambda r: r.last_modified, reverse=True)
                if revisions:
                    return revisions[0].commit_hash
        return None
    except Exception:
        return None


def get_remote_revision():
    """Get the latest revision from HuggingFace Hub, or None on error."""
    try:
        from huggingface_hub import repo_info
        info = repo_info(MODEL_ID, repo_type="model", timeout=10)
        return info.sha
    except Exception as e:
        log(f"Could not check remote revision: {e}")
        return None


def check_model_downloaded():
    """Check if model files are cached locally."""
    return get_local_revision() is not None


def check_update_available():
    """Check if a newer model version is available on HuggingFace."""
    local = get_local_revision()
    if not local:
        return False, None, None  # Not downloaded, can't update
    remote = get_remote_revision()
    if not remote:
        return False, local, None  # Can't check remote
    update_available = local != remote
    return update_available, local, remote


def download_model():
    """Download (or update) the model from HuggingFace."""
    try:
        from huggingface_hub import snapshot_download
        log(f"Downloading {MODEL_ID}...")
        path = snapshot_download(MODEL_ID)
        log(f"Download complete: {path}")
        return True, None
    except Exception as e:
        return False, str(e)


def respond(obj):
    """Write a JSON response to stdout."""
    print(json.dumps(obj), flush=True)


def handle_request(req):
    action = req.get("action", "")

    if action == "status":
        downloaded = check_model_downloaded()
        loaded = _model is not None
        local_rev = get_local_revision()
        respond({
            "ok": True,
            "status": "ready" if loaded else ("not_downloaded" if not downloaded else "downloaded"),
            "downloaded": downloaded,
            "loaded": loaded,
            "model_name": "SottoASR Cleanup",
            "model_id": MODEL_ID,
            "local_revision": local_rev,
        })

    elif action == "check_update":
        update_available, local_rev, remote_rev = check_update_available()
        respond({
            "ok": True,
            "update_available": update_available,
            "local_revision": local_rev,
            "remote_revision": remote_rev,
        })

    elif action == "download":
        success, error = download_model()
        if success:
            respond({"ok": True})
        else:
            respond({"ok": False, "error": error})

    elif action == "load":
        success, err = load_model()
        if success:
            respond({"ok": True})
        else:
            respond({"ok": False, "error": err or "Failed to load model"})

    elif action == "cleanup":
        text = req.get("text", "")

        if not text.strip():
            respond({"ok": True, "text": text, "elapsed_ms": 0, "tokens": 0})
            return

        # Skip very short inputs (< 5 words)
        if len(text.split()) < 5:
            respond({"ok": True, "text": text, "elapsed_ms": 0, "tokens": 0})
            return

        # Lazy-load model if not already loaded (Rust already verified download)
        if _model is None:
            success, err = load_model()
            if not success:
                respond({"ok": False, "error": err or "Failed to load model"})
                return

        try:
            result = cleanup_text(text)
        except Exception as e:
            err = _format_exception(e)
            log(f"cleanup_text raised: {err}")
            respond({"ok": False, "error": err})
            return

        if result[1] is not None:  # error
            respond({"ok": False, "error": result[1]})
        else:
            output, _, elapsed = result
            respond({
                "ok": True,
                "text": output,
                "elapsed_ms": int(elapsed * 1000),
                "tokens": 0,
            })

    elif action == "quit":
        respond({"ok": True})
        sys.exit(0)

    else:
        respond({"ok": False, "error": f"Unknown action: {action}"})


def main():
    log(f"SottoASR cleanup sidecar started (model={MODEL_ID})")

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
            handle_request(req)
        except json.JSONDecodeError as e:
            respond({"ok": False, "error": f"Invalid JSON: {e}"})
        except Exception as e:
            log(f"Error handling request: {e}")
            respond({"ok": False, "error": str(e)})


if __name__ == "__main__":
    main()
