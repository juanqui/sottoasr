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

MODEL_ID = "juanquivilla/sotto-cleanup-lfm25-350m-mlx-5bit"

_model = None
_tokenizer = None
_sampler = None


def log(msg):
    """Log to stderr (stdout is reserved for JSON protocol)."""
    print(f"[llm_cleanup] {msg}", file=sys.stderr, flush=True)


def load_model():
    global _model, _tokenizer, _sampler
    if _model is not None:
        return True

    try:
        from mlx_lm import load
        from mlx_lm.sample_utils import make_sampler
        log(f"Loading {MODEL_ID}...")
        _model, _tokenizer = load(MODEL_ID)
        _sampler = make_sampler(temp=0.0)  # Greedy — deterministic output
        log("Model loaded successfully")
        return True
    except Exception as e:
        log(f"Failed to load model: {e}")
        return False


def cleanup_chunk(text):
    """Clean a single chunk of transcript text (up to ~200 words)."""
    from mlx_lm import generate

    prompt = f"### Input:\n{text}\n\n### Output:\n"
    input_words = len(text.split())
    max_output_tokens = max(256, int(input_words * 1.5))

    output = generate(
        _model,
        _tokenizer,
        prompt=prompt,
        max_tokens=max_output_tokens,
        sampler=_sampler,
        verbose=False,
    )

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
            if MODEL_ID.replace("/", "--") in str(repo.repo_id).replace("/", "--"):
                # Get the latest revision from the cached repo
                revisions = list(repo.revisions)
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
        success = load_model()
        if success:
            respond({"ok": True})
        else:
            respond({"ok": False, "error": "Failed to load model"})

    elif action == "cleanup":
        text = req.get("text", "")

        if not text.strip():
            respond({"ok": True, "text": text, "elapsed_ms": 0, "tokens": 0})
            return

        # Skip very short inputs (< 5 words)
        if len(text.split()) < 5:
            respond({"ok": True, "text": text, "elapsed_ms": 0, "tokens": 0})
            return

        # Check if model is downloaded before attempting load
        if _model is None:
            if not check_model_downloaded():
                respond({"ok": False, "error": "model_not_downloaded"})
                return
            if not load_model():
                respond({"ok": False, "error": "Failed to load model"})
                return

        result = cleanup_text(text)
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
