#!/usr/bin/env python3
"""Interactive prompt testing for transcript cleanup.

Lets you quickly iterate on prompts by typing raw transcript text
and seeing the cleaned output in real-time.

Usage:
    python interactive.py                        # Use standard prompt
    python interactive.py --markdown             # Use markdown prompt
    python interactive.py --custom "Your prompt"  # Use custom prompt
"""

import argparse
import time
from pathlib import Path

MODEL_ID = "Qwen/Qwen3-0.6B"
CACHE_DIR = Path(__file__).parent / "model_cache"
PROMPTS_DIR = Path(__file__).parent / "prompts"


def load_model():
    from transformers import AutoModelForCausalLM, AutoTokenizer

    print(f"Loading {MODEL_ID}...")
    tokenizer = AutoTokenizer.from_pretrained(
        MODEL_ID, cache_dir=CACHE_DIR, trust_remote_code=True
    )
    model = AutoModelForCausalLM.from_pretrained(
        MODEL_ID,
        cache_dir=CACHE_DIR,
        torch_dtype="auto",
        device_map="auto",
        trust_remote_code=True,
    )
    print(f"Ready! ({model.device}, {model.dtype})\n")
    return model, tokenizer


def generate(model, tokenizer, system_prompt: str, user_text: str, max_tokens: int = 2048):
    messages = [
        {"role": "system", "content": system_prompt},
        {"role": "user", "content": user_text},
    ]
    chat_text = tokenizer.apply_chat_template(
        messages, tokenize=False, add_generation_prompt=True, enable_thinking=False
    )
    inputs = tokenizer(chat_text, return_tensors="pt").to(model.device)
    input_len = inputs["input_ids"].shape[1]

    start = time.perf_counter()
    outputs = model.generate(
        **inputs,
        max_new_tokens=max_tokens,
        temperature=0.3,
        top_p=0.9,
        top_k=20,
        do_sample=True,
        repetition_penalty=1.1,
    )
    elapsed = time.perf_counter() - start

    output_ids = outputs[0][input_len:]
    response = tokenizer.decode(output_ids, skip_special_tokens=True)
    return response.strip(), len(output_ids), elapsed


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--markdown", action="store_true", help="Use markdown prompt")
    parser.add_argument("--custom", type=str, help="Custom system prompt")
    args = parser.parse_args()

    if args.custom:
        system_prompt = args.custom
        prompt_name = "custom"
    elif args.markdown:
        system_prompt = (PROMPTS_DIR / "markdown.txt").read_text().strip()
        prompt_name = "markdown"
    else:
        system_prompt = (PROMPTS_DIR / "standard.txt").read_text().strip()
        prompt_name = "standard"

    model, tokenizer = load_model()

    print(f"Using prompt: {prompt_name}")
    print(f"System: {system_prompt[:100]}...")
    print(f"\nType a raw transcript and press Enter (or 'quit' to exit, 'prompt' to change):")
    print("-" * 60)

    while True:
        try:
            user_input = input("\n> ").strip()
        except (EOFError, KeyboardInterrupt):
            print("\nBye!")
            break

        if not user_input:
            continue
        if user_input.lower() == "quit":
            break
        if user_input.lower() == "prompt":
            new_prompt = input("New system prompt: ").strip()
            if new_prompt:
                system_prompt = new_prompt
                prompt_name = "custom"
                print("Updated system prompt.")
            continue

        result, tokens, elapsed = generate(model, tokenizer, system_prompt, user_input)
        tps = tokens / elapsed if elapsed > 0 else 0

        print(f"\n--- Cleaned ({tokens} tokens, {elapsed:.2f}s, {tps:.1f} tok/s) ---")
        print(result)
        print("---")


if __name__ == "__main__":
    main()
