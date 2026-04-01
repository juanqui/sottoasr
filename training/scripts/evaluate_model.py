#!/usr/bin/env python3
"""Evaluate a fine-tuned LFM2.5-350M model against the SottoASR benchmark.

Usage:
    python evaluate_model.py --model ~/sotto-finetune/output/merged
    python evaluate_model.py --model ~/sotto-finetune/output/merged --benchmark ~/sotto/benchmarks/llm/dataset.csv
"""

import argparse
import csv
import json
import re
import time
from collections import defaultdict
from pathlib import Path

# ── Metrics ──────────────────────────────────────────────────────────

def _tokenize(text):
    return re.findall(r"\w+", text.lower())


def rouge_l_f1(hyp, ref):
    h, r = _tokenize(hyp), _tokenize(ref)
    if not h or not r:
        return 1.0 if not h and not r else 0.0
    m, n = len(r), len(h)
    dp = [[0] * (n + 1) for _ in range(m + 1)]
    for i in range(1, m + 1):
        for j in range(1, n + 1):
            dp[i][j] = dp[i-1][j-1] + 1 if r[i-1] == h[j-1] else max(dp[i-1][j], dp[i][j-1])
    lcs = dp[m][n]
    p = lcs / n if n else 0
    rc = lcs / m if m else 0
    return 2 * p * rc / (p + rc) if (p + rc) else 0.0


def chrf_score(hyp, ref, n=6, beta=2.0):
    from collections import Counter
    def cng(t, o):
        return Counter(t[i:i+o] for i in range(len(t)-o+1))
    tp = tr = 0.0
    c = 0
    h, r = hyp.lower(), ref.lower()
    for o in range(1, n+1):
        hg, rg = cng(h, o), cng(r, o)
        cm = sum((hg & rg).values())
        ht, rt = sum(hg.values()), sum(rg.values())
        tp += cm / ht if ht else 0
        tr += cm / rt if rt else 0
        c += 1
    ap = tp / c if c else 0
    ar = tr / c if c else 0
    if ap + ar == 0:
        return 0.0
    b2 = beta ** 2
    return (1 + b2) * ap * ar / (b2 * ap + ar)


FILLERS = [
    "uh", "um", "uhm", "er", "like", "you know", "basically", "right", "yeah",
    "okay", "so", "i mean", "honestly", "literally", "anyway",
]


def count_fillers(text):
    tl = f" {text.lower()} "
    return sum(len(re.findall(rf"\b{re.escape(f)}\b", tl)) for f in FILLERS)


def exact_match(h, r):
    return " ".join(h.lower().split()) == " ".join(r.lower().split())


# ── Inference ────────────────────────────────────────────────────────

def load_model(model_path):
    """Load the fine-tuned model for inference."""
    import torch
    from transformers import AutoModelForCausalLM, AutoTokenizer

    print(f"Loading model from {model_path}...")
    tokenizer = AutoTokenizer.from_pretrained(model_path, trust_remote_code=True)
    model = AutoModelForCausalLM.from_pretrained(
        model_path,
        dtype=torch.bfloat16,
        device_map="auto",
        trust_remote_code=True,
    )
    model.eval()

    if tokenizer.pad_token is None:
        tokenizer.pad_token = tokenizer.eos_token

    print(f"  Loaded on {next(model.parameters()).device}")
    return model, tokenizer


def generate_cleanup(model, tokenizer, raw_text, max_new_tokens=512):
    """Run inference: raw transcript → cleaned text."""
    import torch

    prompt = f"### Input:\n{raw_text}\n\n### Output:\n"
    inputs = tokenizer(prompt, return_tensors="pt").to(model.device)

    with torch.no_grad():
        outputs = model.generate(
            **inputs,
            max_new_tokens=max_new_tokens,
            do_sample=False,  # greedy for deterministic eval
            temperature=1.0,
            eos_token_id=tokenizer.eos_token_id,
            pad_token_id=tokenizer.pad_token_id,
        )

    # Decode only the generated part (after the prompt)
    generated_ids = outputs[0][inputs["input_ids"].shape[1]:]
    output = tokenizer.decode(generated_ids, skip_special_tokens=True).strip()
    return output


# ── Benchmark Runner ─────────────────────────────────────────────────

def run_benchmark(model, tokenizer, dataset_path):
    """Run the full 135-sample benchmark."""
    with open(dataset_path) as f:
        samples = list(csv.DictReader(f))

    results = []
    cats = defaultdict(list)

    print(f"Running benchmark on {len(samples)} samples...")

    for i, sample in enumerate(samples):
        raw = sample["raw"]
        expected = sample["expected"]

        start = time.perf_counter()
        output = generate_cleanup(model, tokenizer, raw)
        elapsed = time.perf_counter() - start

        rl = rouge_l_f1(output, expected)
        cf = chrf_score(output, expected)
        fc = count_fillers(output)
        em = exact_match(output, expected)

        result = {
            "id": sample["id"],
            "category": sample["category"],
            "rouge_l": rl,
            "chrf": cf,
            "filler_count": fc,
            "exact_match": em,
            "elapsed_s": elapsed,
            "output": output,
        }
        results.append(result)
        cats[sample["category"]].append(result)

        status = "EXACT" if em else f"R={rl:.2f}"
        filler_flag = f" FILLERS={fc}" if fc > 0 else ""
        print(f"  [{i+1:3d}/{len(samples)}] {sample['id']:25s} | {status:10s} | chrF={cf:.2f} | {elapsed:.2f}s{filler_flag}")

    # ── Summary ──────────────────────────────────────────────────────
    n = len(results)
    avg = lambda v: sum(v) / len(v) if v else 0

    print(f"\n{'='*80}")
    print("FINE-TUNED MODEL BENCHMARK RESULTS")
    print(f"{'='*80}")
    print(f"  ROUGE-L:          {avg([r['rouge_l'] for r in results]):.4f}")
    print(f"  chrF:             {avg([r['chrf'] for r in results]):.4f}")
    print(f"  Exact Match:      {sum(1 for r in results if r['exact_match'])/n:.1%}")
    print(f"  Zero-Filler Rate: {sum(1 for r in results if r['filler_count']==0)/n:.1%}")
    print(f"  Avg Latency:      {avg([r['elapsed_s'] for r in results]):.3f}s")

    print(f"\n{'Category':25s} | {'N':>3s} | {'ROUGE-L':>7s} | {'chrF':>6s} | {'Exact':>5s} | {'Fillers':>7s}")
    print("-" * 70)
    for cat in sorted(cats):
        cr = cats[cat]
        cn = len(cr)
        print(f"{cat:25s} | {cn:>3d} | "
              f"{avg([r['rouge_l'] for r in cr]):>7.3f} | "
              f"{avg([r['chrf'] for r in cr]):>6.3f} | "
              f"{sum(1 for r in cr if r['exact_match'])/cn:>5.1%} | "
              f"{sum(r['filler_count'] for r in cr):>7d}")

    worst = sorted(results, key=lambda r: r["rouge_l"])[:5]
    print(f"\nWorst 5:")
    for r in worst:
        print(f"  {r['id']:25s} | R={r['rouge_l']:.3f} | fillers={r['filler_count']}")

    return results


# ── Main ─────────────────────────────────────────────────────────────

if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True, help="Path to fine-tuned model")
    parser.add_argument("--benchmark", default="~/sotto/benchmarks/llm/dataset.csv",
                        help="Path to benchmark CSV")
    parser.add_argument("--output", default=None, help="Save results JSON to this path")
    args = parser.parse_args()

    model, tokenizer = load_model(args.model)

    benchmark_path = Path(args.benchmark).expanduser()
    results = run_benchmark(model, tokenizer, benchmark_path)

    if args.output:
        with open(args.output, "w") as f:
            json.dump(results, f, indent=2)
        print(f"\nResults saved to {args.output}")
