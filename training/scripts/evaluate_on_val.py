#!/usr/bin/env python3
"""
Evaluate a model on the held-out validation set (6,921 samples).
This is the proper benchmark — NOT the 135-sample hand-crafted CSV.

Usage:
    python evaluate_on_val.py --model ~/sotto-finetune/output_v18/best
    python evaluate_on_val.py --model ~/sotto-finetune/output_v18/best --samples 1000
"""
import argparse
import json
import re
import random
import time
import torch
from pathlib import Path
from transformers import AutoModelForCausalLM, AutoTokenizer


def rouge_l_f1(hyp, ref):
    h = re.findall(r"\w+", hyp.lower())
    r = re.findall(r"\w+", ref.lower())
    if not h or not r:
        return 1.0 if not h and not r else 0.0
    m, n = len(r), len(h)
    dp = [[0] * (n + 1) for _ in range(m + 1)]
    for i in range(1, m + 1):
        for j in range(1, n + 1):
            dp[i][j] = dp[i - 1][j - 1] + 1 if r[i - 1] == h[j - 1] else max(dp[i - 1][j], dp[i][j - 1])
    lcs = dp[m][n]
    p = lcs / n if n else 0
    rc = lcs / m if m else 0
    return 2 * p * rc / (p + rc) if (p + rc) else 0.0


def exact_match(h, r):
    return " ".join(h.lower().split()) == " ".join(r.lower().split())


FILLERS = re.compile(
    r"\b(uh|um|uhm|er|like|you know|basically|right|yeah|okay|so|i mean)\b", re.I
)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True, help="Path to model")
    parser.add_argument("--val", default="data_lfm_v17/val.jsonl", help="Path to val JSONL")
    parser.add_argument("--samples", type=int, default=1000, help="Number of samples (0=all)")
    parser.add_argument("--seed", type=int, default=42)
    args = parser.parse_args()

    # Load val set
    val = []
    with open(args.val) as f:
        for line in f:
            if line.strip():
                val.append(json.loads(line))

    random.seed(args.seed)
    samples = random.sample(val, min(args.samples, len(val))) if args.samples > 0 else val

    # Load model
    print(f"Loading {args.model}...")
    tokenizer = AutoTokenizer.from_pretrained(args.model, trust_remote_code=True)
    model = AutoModelForCausalLM.from_pretrained(
        args.model, dtype=torch.bfloat16, trust_remote_code=True,
        attn_implementation="eager", device_map="auto",
    )
    model.eval()
    if tokenizer.pad_token is None:
        tokenizer.pad_token = tokenizer.eos_token

    print(f"Evaluating on {len(samples)} val samples...")
    scores = []
    exact_count = 0
    filler_free = 0
    start = time.time()

    for i, s in enumerate(samples):
        prompt = f"### Input:\n{s['input']}\n\n### Output:\n"
        inputs = tokenizer(prompt, return_tensors="pt", truncation=True, max_length=512).to(model.device)
        with torch.no_grad():
            out = model.generate(**inputs, max_new_tokens=512, do_sample=False)
        output = tokenizer.decode(out[0][inputs["input_ids"].shape[1]:], skip_special_tokens=True).strip()
        if "###" in output:
            output = output[: output.index("###")].strip()

        rl = rouge_l_f1(output, s["output"])
        em = exact_match(output, s["output"])
        fc = len(FILLERS.findall(output))

        scores.append(rl)
        if em:
            exact_count += 1
        if fc == 0:
            filler_free += 1

        if (i + 1) % 200 == 0:
            elapsed = time.time() - start
            print(
                f"  [{i+1}/{len(samples)}] ROUGE-L: {sum(scores)/len(scores):.4f} | "
                f"Exact: {exact_count/(i+1):.1%} | {elapsed:.0f}s"
            )

    n = len(scores)
    elapsed = time.time() - start
    print(f"\n{'=' * 60}")
    print(f"VAL SET BENCHMARK ({n} samples)")
    print(f"{'=' * 60}")
    print(f"  ROUGE-L:      {sum(scores)/n:.4f}")
    print(f"  Exact Match:  {exact_count/n:.1%}")
    print(f"  Zero-Filler:  {filler_free/n:.1%}")
    print(f"  Avg Latency:  {elapsed/n:.3f}s")
    print(f"{'=' * 60}")


if __name__ == "__main__":
    main()
