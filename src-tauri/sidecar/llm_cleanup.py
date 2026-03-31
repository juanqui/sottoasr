#!/usr/bin/env python3
"""SottoASR LLM Cleanup Sidecar — runs Qwen3.5-0.8B via MLX for transcript cleanup.

Protocol: reads JSON requests from stdin (one per line), writes JSON responses to stdout.

Request format:
  {"action": "cleanup", "text": "raw transcript", "mode": "standard"|"markdown"}
  {"action": "status"}
  {"action": "download"}
  {"action": "quit"}

Response format:
  {"ok": true, "text": "cleaned transcript", "elapsed_ms": 123, "tokens": 45}
  {"ok": true, "status": "ready"|"not_downloaded"|"downloading"|"error", ...}
  {"ok": false, "error": "message"}
"""

import argparse
import json
import re
import sys
import time
import os

DEFAULT_MODEL_ID = "mlx-community/Qwen3.5-2B-OptiQ-4bit"
MODEL_ID = DEFAULT_MODEL_ID  # overridden by --model CLI arg

STANDARD_PROMPT = (
    "Clean this speech-to-text transcript.\n\n"
    "You MUST apply ALL of these changes:\n"
    "1. Remove fillers (uh, um, uhm, er) and crutch words (basically, you know, "
    "I mean, honestly, literally, anyway, and filler uses of "
    "like/so/okay/yeah/right)\n"
    "2. Remove stuttered repetitions (\"the the\" → \"the\") and false starts\n"
    "3. Fix punctuation, capitalization, grammar, and misheard terms\n"
    "4. Convert spoken punctuation: \"period\" → \".\", \"dot\" → \".\", "
    "\"comma\" → \",\", \"slash\" → \"/\", \"question mark\" → \"?\", "
    "\"exclamation point\" → \"!\"\n"
    "5. Self-corrections (\"wait\", \"actually\", \"no\", \"scratch that\"): "
    "DELETE the original, keep ONLY the correction\n"
    "6. Format numbered items (first/second/third, one/two/three) as a "
    "numbered list\n\n"
    "You MUST NOT paraphrase or reword. Preserve emphasis (really, very, "
    "definitely) and phrases like \"go ahead and\", \"a lot of\". "
    "Do not summarize.\n\n"
    "Examples:\n"
    "IN: \"I uh think we should use Redis, wait no, Memcached would be better\"\n"
    "OUT: \"I think we should use Memcached.\"\n\n"
    "IN: \"So basically the uh database is uh timing out period\"\n"
    "OUT: \"The database is timing out.\"\n\n"
    "IN: \"Let's go ahead and really focus on this\"\n"
    "OUT: \"Let's go ahead and really focus on this.\"\n\n"
    "Output only the cleaned text."
)

MARKDOWN_PROMPT = (
    "You are a transcript-to-markdown converter. Take the raw speech transcript "
    "and convert it into well-structured Markdown.\n\n"
    "Rules:\n"
    "1. Remove filler words (uh, um, like, you know)\n"
    "2. Fix grammar and misheard words\n"
    "3. Organize content with headings (## for main topics)\n"
    "4. Use bullet lists for items and details\n"
    "5. Use numbered lists for sequential items or action items\n"
    "6. Use bold for emphasis on key terms\n"
    "7. Keep all information — do not summarize\n\n"
    "Output ONLY the markdown, no commentary."
)

_model = None
_tokenizer = None


def log(msg):
    """Log to stderr (stdout is reserved for JSON protocol)."""
    print(f"[llm_cleanup] {msg}", file=sys.stderr, flush=True)


def load_model():
    global _model, _tokenizer
    if _model is not None:
        return True

    try:
        from mlx_lm import load
        log(f"Loading {MODEL_ID}...")
        _model, _tokenizer = load(MODEL_ID)
        log("Model loaded successfully")
        return True
    except Exception as e:
        log(f"Failed to load model: {e}")
        return False


def strip_thinking_tags(text):
    """Strip <think>...</think> blocks from model output.

    Qwen3/3.5 models may emit thinking blocks even when enable_thinking=False
    is passed to apply_chat_template (e.g. if the MLX tokenizer doesn't fully
    support that flag). This ensures only the actual response is returned.
    """
    cleaned = re.sub(r"<think>.*?</think>", "", text, flags=re.DOTALL).strip()
    # Also handle unclosed <think> (model stopped mid-thought)
    cleaned = re.sub(r"<think>.*", "", cleaned, flags=re.DOTALL).strip()
    if cleaned != text.strip():
        log(f"Stripped thinking tags ({len(text)} → {len(cleaned)} chars)")
    return cleaned


def cleanup_text(text, mode="standard"):
    """Clean up transcript text using the loaded model."""
    from mlx_lm import stream_generate
    from mlx_lm.sample_utils import make_sampler, make_logits_processors

    if _model is None or _tokenizer is None:
        return None, "Model not loaded"

    system = STANDARD_PROMPT if mode == "standard" else MARKDOWN_PROMPT

    messages = [
        {"role": "system", "content": system},
        {"role": "user", "content": text},
    ]

    prompt = _tokenizer.apply_chat_template(
        messages,
        tokenize=False,
        add_generation_prompt=True,
        enable_thinking=False,
    )

    sampler = make_sampler(temp=0.3, top_p=0.9)
    logits_processors = make_logits_processors(repetition_penalty=1.10)

    start = time.perf_counter()
    segments = []
    last_resp = None
    for resp in stream_generate(
        _model,
        _tokenizer,
        prompt=prompt,
        max_tokens=4096,
        sampler=sampler,
        logits_processors=logits_processors,
    ):
        segments.append(resp.text)
        last_resp = resp
    elapsed = time.perf_counter() - start

    output = "".join(segments).strip()
    gen_tokens = last_resp.generation_tokens if last_resp else 0

    # Strip any thinking blocks the model may have emitted
    output = strip_thinking_tags(output)

    return output, None, elapsed, gen_tokens


def check_model_downloaded():
    """Check if model files are cached locally."""
    try:
        from huggingface_hub import scan_cache_dir
        cache = scan_cache_dir()
        for repo in cache.repos:
            if MODEL_ID.replace("/", "--") in str(repo.repo_id).replace("/", "--"):
                return True
        return False
    except Exception:
        return False


def download_model():
    """Download the model (delegated to huggingface_hub)."""
    try:
        from huggingface_hub import snapshot_download
        log(f"Downloading {MODEL_ID}...")
        snapshot_download(MODEL_ID)
        log("Download complete")
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
        respond({
            "ok": True,
            "status": "ready" if loaded else ("not_downloaded" if not downloaded else "downloaded"),
            "downloaded": downloaded,
            "loaded": loaded,
            "model_name": MODEL_ID.split("/")[-1] if "/" in MODEL_ID else MODEL_ID,
            "model_id": MODEL_ID,
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
        mode = req.get("mode", "standard")

        if not text.strip():
            respond({"ok": True, "text": text, "elapsed_ms": 0, "tokens": 0})
            return

        # Skip very short inputs
        if len(text.split()) < 5:
            respond({"ok": True, "text": text, "elapsed_ms": 0, "tokens": 0})
            return

        # Auto-load model if not loaded
        if _model is None:
            if not load_model():
                respond({"ok": False, "error": "Model not available"})
                return

        result = cleanup_text(text, mode)
        if result[1] is not None:  # error
            respond({"ok": False, "error": result[1]})
        else:
            output, _, elapsed, tokens = result
            # Validate output length ratio
            ratio = len(output) / len(text) if text else 1.0
            if ratio < 0.3 or ratio > 2.5:
                log(f"Output ratio {ratio:.2f} outside bounds (input={len(text)}, output={len(output)}), using raw text")
                log(f"  First 200 chars of output: {output[:200]!r}")
                respond({
                    "ok": True,
                    "text": text,
                    "elapsed_ms": int(elapsed * 1000),
                    "tokens": tokens,
                    "fallback": True,
                    "fallback_reason": f"output ratio {ratio:.2f} outside bounds",
                })
            else:
                respond({
                    "ok": True,
                    "text": output,
                    "elapsed_ms": int(elapsed * 1000),
                    "tokens": tokens,
                })

    elif action == "quit":
        respond({"ok": True})
        sys.exit(0)

    else:
        respond({"ok": False, "error": f"Unknown action: {action}"})


def main():
    global MODEL_ID

    parser = argparse.ArgumentParser()
    parser.add_argument("--model", default=DEFAULT_MODEL_ID, help="HuggingFace model ID")
    args = parser.parse_args()
    MODEL_ID = args.model

    log(f"Sidecar started (model={MODEL_ID}), waiting for requests on stdin...")

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
