#!/usr/bin/env python3
"""Download Qwen3-0.6B model for local experimentation.

Downloads the model and tokenizer from HuggingFace to a local cache directory.
This script validates the download and runs a quick smoke test.
"""

import os
import sys
from pathlib import Path

MODEL_ID = "Qwen/Qwen3-0.6B"
CACHE_DIR = Path(__file__).parent / "model_cache"


def download_model():
    """Download model and tokenizer, return them for verification."""
    from transformers import AutoModelForCausalLM, AutoTokenizer

    print(f"Downloading {MODEL_ID} to {CACHE_DIR}...")
    print("This may take a few minutes on first run (~1.2 GB).\n")

    os.makedirs(CACHE_DIR, exist_ok=True)

    print("Downloading tokenizer...")
    tokenizer = AutoTokenizer.from_pretrained(
        MODEL_ID,
        cache_dir=CACHE_DIR,
        trust_remote_code=True,
    )
    print(f"  Tokenizer ready. Vocab size: {tokenizer.vocab_size}")

    print("\nDownloading model weights...")
    model = AutoModelForCausalLM.from_pretrained(
        MODEL_ID,
        cache_dir=CACHE_DIR,
        torch_dtype="auto",
        device_map="auto",
        trust_remote_code=True,
    )
    print(f"  Model ready. Parameters: {model.num_parameters():,}")
    print(f"  Device: {model.device}")
    print(f"  Dtype: {model.dtype}")

    return model, tokenizer


def smoke_test(model, tokenizer):
    """Run a quick generation to verify the model works."""
    print("\n--- Smoke Test ---")
    prompt = "Hello, how are you?"
    messages = [{"role": "user", "content": prompt}]

    text = tokenizer.apply_chat_template(
        messages,
        tokenize=False,
        add_generation_prompt=True,
        enable_thinking=False,
    )
    inputs = tokenizer(text, return_tensors="pt").to(model.device)

    outputs = model.generate(
        **inputs,
        max_new_tokens=50,
        temperature=0.7,
        top_p=0.8,
        top_k=20,
        do_sample=True,
    )
    response = tokenizer.decode(outputs[0][inputs["input_ids"].shape[1]:], skip_special_tokens=True)
    print(f"Prompt: {prompt}")
    print(f"Response: {response}")
    print("\nModel is working correctly!")


if __name__ == "__main__":
    model, tokenizer = download_model()
    smoke_test(model, tokenizer)
