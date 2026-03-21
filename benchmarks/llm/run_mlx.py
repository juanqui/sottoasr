#!/usr/bin/env python3
"""Benchmark MLX-based LLM transcript cleanup against the ground-truth dataset.

Uses mlx-lm with Qwen3.5-0.8B-OptiQ-4bit on Apple Silicon Metal GPU.

Usage:
    python run_mlx.py                          # Run full benchmark
    python run_mlx.py --category self_correction
    python run_mlx.py --model mlx-community/Qwen3.5-0.8B-MLX-4bit
"""

import argparse
import csv
import json
import re
import time
from collections import defaultdict
from pathlib import Path
from dataclasses import dataclass, asdict

DEFAULT_MODEL = "mlx-community/Qwen3.5-0.8B-OptiQ-4bit"
DATASET = Path(__file__).parent / "dataset.csv"
RESULTS_DIR = Path(__file__).parent / "results"
PROMPTS_DIR = Path(__file__).parent / "prompts"

DEFAULT_PROMPT = (PROMPTS_DIR / "standard.txt").read_text().strip()

FILLERS = [
    "uh", "um", "like", "you know", "basically", "right", "yeah", "okay",
    "so", "i mean", "honestly", "literally", "anyway",
]


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


def jaccard(a, b):
    wa, wb = set(_tokenize(a)), set(_tokenize(b))
    if not wa and not wb:
        return 1.0
    return len(wa & wb) / len(wa | wb) if (wa | wb) else 0.0


def count_fillers(text):
    tl = f" {text.lower()} "
    return sum(len(re.findall(rf"\b{re.escape(f)}\b", tl)) for f in FILLERS)


def exact_match(h, r):
    return " ".join(h.lower().split()) == " ".join(r.lower().split())


@dataclass
class Result:
    id: str
    category: str
    rouge_l: float
    chrf: float
    jaccard: float
    len_ratio: float
    filler_count: int
    exact: bool
    elapsed_s: float
    prompt_tps: float
    gen_tps: float
    output: str = ""


def run_benchmark(model_id, system_prompt, category_filter=None, cycle=0):
    from mlx_lm import load, stream_generate, generate as mlx_generate

    RESULTS_DIR.mkdir(exist_ok=True)

    print(f"Loading {model_id}...")
    model, tokenizer = load(model_id)
    print(f"Loaded! Running benchmark...\n")

    with open(DATASET) as f:
        samples = list(csv.DictReader(f))
    if category_filter:
        samples = [s for s in samples if s["category"] == category_filter]

    results = []
    total_prompt_tokens = 0
    total_gen_tokens = 0
    total_time = 0

    for i, sample in enumerate(samples):
        raw, expected = sample["raw"], sample["expected"]
        messages = [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": raw},
        ]
        prompt = tokenizer.apply_chat_template(
            messages, tokenize=False, add_generation_prompt=True, enable_thinking=False,
        )

        start = time.perf_counter()
        # Collect all stream chunks to get full text + final timing
        segments = []
        last_resp = None
        for resp in stream_generate(model, tokenizer, prompt=prompt, max_tokens=1024):
            segments.append(resp.text)
            last_resp = resp
        elapsed = time.perf_counter() - start
        full_text = "".join(segments)

        prompt_tokens = last_resp.prompt_tokens if last_resp else 0
        gen_tokens = last_resp.generation_tokens if last_resp else 0
        gen_tps = last_resp.generation_tps if last_resp else 0
        prompt_tps = last_resp.prompt_tps if last_resp else 0

        total_prompt_tokens += prompt_tokens
        total_gen_tokens += gen_tokens
        total_time += elapsed

        output = full_text.strip()
        lr = len(output) / len(expected) if expected else (1.0 if not output else float("inf"))

        r = Result(
            id=sample["id"], category=sample["category"],
            rouge_l=rouge_l_f1(output, expected),
            chrf=chrf_score(output, expected),
            jaccard=jaccard(output, expected),
            len_ratio=lr,
            filler_count=count_fillers(output),
            exact=exact_match(output, expected),
            elapsed_s=elapsed,
            prompt_tps=prompt_tps,
            gen_tps=gen_tps,
            output=output,
        )
        results.append(r)

        flag = " FILLERS" if r.filler_count > 0 else ""
        status = "EXACT" if r.exact else f"R={r.rouge_l:.2f}"
        print(f"  [{i+1:3d}/{len(samples)}] {r.id:25s} | {status:10s} | chrF={r.chrf:.2f} | {r.elapsed_s:.2f}s | {r.gen_tps:.0f}t/s{flag}")

    # Save
    tag = f"_mlx_cycle{cycle}" if cycle else "_mlx"
    rfile = RESULTS_DIR / f"benchmark{tag}_{int(time.time())}.json"
    with open(rfile, "w") as f:
        json.dump({"model": model_id, "prompt": system_prompt, "cycle": cycle,
                    "results": [asdict(r) for r in results]}, f, indent=2)

    # Summary
    n = len(results)
    avg = lambda v: sum(v) / len(v) if v else 0
    print(f"\n{'='*80}")
    print(f"MLX BENCHMARK — {model_id} — Cycle {cycle}")
    print(f"{'='*80}")
    print(f"  ROUGE-L:          {avg([r.rouge_l for r in results]):.4f}")
    print(f"  chrF:             {avg([r.chrf for r in results]):.4f}")
    print(f"  Jaccard:          {avg([r.jaccard for r in results]):.4f}")
    print(f"  Avg Len Ratio:    {avg([r.len_ratio for r in results]):.4f}")
    print(f"  Exact Match:      {sum(1 for r in results if r.exact)/n:.1%}")
    print(f"  Zero-Filler Rate: {sum(1 for r in results if r.filler_count == 0)/n:.1%}")
    print(f"  Total Fillers:    {sum(r.filler_count for r in results)}")
    print(f"  Avg Latency:      {avg([r.elapsed_s for r in results]):.3f}s")
    print(f"  Avg Gen tok/s:    {total_gen_tokens/total_time:.1f}")

    cats = defaultdict(list)
    for r in results:
        cats[r.category].append(r)
    print(f"\n{'Category':25s} | {'N':>3s} | {'ROUGE-L':>7s} | {'chrF':>6s} | {'LenR':>5s} | {'Exact':>5s} | {'Fillers':>7s}")
    print("-" * 80)
    for cat in sorted(cats):
        cr = cats[cat]
        cn = len(cr)
        print(f"{cat:25s} | {cn:>3d} | {avg([r.rouge_l for r in cr]):>7.3f} | {avg([r.chrf for r in cr]):>6.3f} | {avg([r.len_ratio for r in cr]):>5.2f} | {sum(1 for r in cr if r.exact)/cn:>5.1%} | {sum(r.filler_count for r in cr):>7d}")

    worst = sorted(results, key=lambda r: r.rouge_l)[:5]
    print(f"\nWorst 5:")
    for r in worst:
        print(f"  {r.id:25s} | R={r.rouge_l:.3f} | fillers={r.filler_count}")

    print(f"\nResults → {rfile}")


if __name__ == "__main__":
    p = argparse.ArgumentParser()
    p.add_argument("--model", default=DEFAULT_MODEL)
    p.add_argument("--category", type=str)
    p.add_argument("--prompt", type=str)
    p.add_argument("--cycle", type=int, default=0)
    args = p.parse_args()
    run_benchmark(args.model, args.prompt or DEFAULT_PROMPT, args.category, args.cycle)
