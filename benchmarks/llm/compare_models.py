#!/usr/bin/env python3
"""Compare all 3 Qwen3.5 OptiQ models on the transcript cleanup benchmark.

Runs the same benchmark suite against 0.8B, 2B, and 4B models, then prints
a side-by-side comparison table.

Usage:
    python compare_models.py                    # All models, full dataset
    python compare_models.py --models 0.8b 4b   # Specific models
    python compare_models.py --category long_dictation
"""

import argparse
import csv
import json
import re
import time
from collections import Counter, defaultdict
from pathlib import Path
from dataclasses import dataclass, asdict

MODELS = {
    "0.8b": "mlx-community/Qwen3.5-0.8B-OptiQ-4bit",
    "2b": "mlx-community/Qwen3.5-2B-OptiQ-4bit",
    "4b": "mlx-community/Qwen3.5-4B-OptiQ-4bit",
}

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


def run_single_model(model_id, model_label, system_prompt, samples):
    """Run benchmark for a single model. Returns list of Result."""
    from mlx_lm import load, stream_generate

    print(f"\n{'='*80}")
    print(f"  MODEL: {model_label} ({model_id})")
    print(f"{'='*80}")
    print(f"  Loading model...")
    model, tokenizer = load(model_id)
    print(f"  Loaded! Running {len(samples)} samples...\n")

    results = []
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
        segments = []
        last_resp = None
        for resp in stream_generate(model, tokenizer, prompt=prompt, max_tokens=2048):
            segments.append(resp.text)
            last_resp = resp
        elapsed = time.perf_counter() - start
        output = "".join(segments).strip()

        prompt_tps = last_resp.prompt_tps if last_resp else 0
        gen_tps = last_resp.generation_tps if last_resp else 0

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
        print(f"  [{i+1:3d}/{len(samples)}] {r.id:25s} | {status:10s} | chrF={r.chrf:.2f} | LR={r.len_ratio:.2f} | {r.elapsed_s:.2f}s | {r.gen_tps:.0f}t/s{flag}")

    # Free memory before loading next model
    del model, tokenizer
    try:
        import mlx.core as mx
        mx.metal.clear_cache()
    except Exception:
        pass

    return results


def print_comparison(all_results: dict[str, list[Result]]):
    """Print side-by-side comparison of all models."""
    avg = lambda v: sum(v) / len(v) if v else 0

    print(f"\n\n{'#'*80}")
    print(f"  COMPARISON: ALL MODELS")
    print(f"{'#'*80}\n")

    # Overall metrics table
    header = f"{'Metric':25s}"
    for label in all_results:
        header += f" | {label:>12s}"
    print(header)
    print("-" * (28 + 15 * len(all_results)))

    metrics = [
        ("ROUGE-L F1", lambda rs: avg([r.rouge_l for r in rs])),
        ("chrF", lambda rs: avg([r.chrf for r in rs])),
        ("Jaccard", lambda rs: avg([r.jaccard for r in rs])),
        ("Avg Length Ratio", lambda rs: avg([r.len_ratio for r in rs])),
        ("Exact Match Rate", lambda rs: sum(1 for r in rs if r.exact) / len(rs) if rs else 0),
        ("Zero-Filler Rate", lambda rs: sum(1 for r in rs if r.filler_count == 0) / len(rs) if rs else 0),
        ("Total Fillers", lambda rs: float(sum(r.filler_count for r in rs))),
        ("Avg Latency (s)", lambda rs: avg([r.elapsed_s for r in rs])),
        ("Avg Gen tok/s", lambda rs: avg([r.gen_tps for r in rs])),
    ]

    for name, fn in metrics:
        row = f"{name:25s}"
        values = []
        for label, rs in all_results.items():
            v = fn(rs)
            values.append(v)
            if name in ("Total Fillers",):
                row += f" | {v:>12.0f}"
            elif name.endswith("Rate"):
                row += f" | {v:>11.1%} "
            else:
                row += f" | {v:>12.4f}"
        # Mark the best value
        row += "  "
        print(row)

    # Per-category comparison
    all_cats = set()
    for rs in all_results.values():
        for r in rs:
            all_cats.add(r.category)

    print(f"\n--- ROUGE-L by Category ---")
    header = f"{'Category':25s}"
    for label in all_results:
        header += f" | {label:>8s}"
    print(header)
    print("-" * (28 + 11 * len(all_results)))

    for cat in sorted(all_cats):
        row = f"{cat:25s}"
        for label, rs in all_results.items():
            cr = [r for r in rs if r.category == cat]
            row += f" | {avg([r.rouge_l for r in cr]):>8.3f}" if cr else f" | {'N/A':>8s}"
        print(row)

    # Per-category length ratio comparison
    print(f"\n--- Length Ratio by Category ---")
    header = f"{'Category':25s}"
    for label in all_results:
        header += f" | {label:>8s}"
    print(header)
    print("-" * (28 + 11 * len(all_results)))

    for cat in sorted(all_cats):
        row = f"{cat:25s}"
        for label, rs in all_results.items():
            cr = [r for r in rs if r.category == cat]
            row += f" | {avg([r.len_ratio for r in cr]):>8.2f}" if cr else f" | {'N/A':>8s}"
        print(row)

    # Show specific long_dictation sample outputs
    print(f"\n--- long_06 (user's failing example) ---")
    for label, rs in all_results.items():
        matching = [r for r in rs if r.id == "long_06"]
        if matching:
            r = matching[0]
            print(f"\n  [{label}] ROUGE-L={r.rouge_l:.3f} | chrF={r.chrf:.3f} | LR={r.len_ratio:.2f} | fillers={r.filler_count}")
            # Show first 200 chars of output
            preview = r.output[:300].replace('\n', ' ')
            print(f"    Output: {preview}{'...' if len(r.output) > 300 else ''}")


def main():
    parser = argparse.ArgumentParser(description="Compare models on transcript cleanup benchmark")
    parser.add_argument("--models", nargs="+", default=["0.8b", "2b", "4b"],
                        choices=list(MODELS.keys()), help="Models to compare")
    parser.add_argument("--category", type=str, help="Filter to a specific category")
    parser.add_argument("--prompt", type=str, help="Custom system prompt")
    args = parser.parse_args()

    system_prompt = args.prompt or DEFAULT_PROMPT

    # Load dataset
    with open(DATASET) as f:
        samples = list(csv.DictReader(f))
    if args.category:
        samples = [s for s in samples if s["category"] == args.category]

    print(f"Benchmark: {len(samples)} samples × {len(args.models)} models")
    print(f"Prompt: {system_prompt[:80]}...")

    RESULTS_DIR.mkdir(exist_ok=True)
    all_results = {}

    for size in args.models:
        model_id = MODELS[size]
        label = f"Qwen3.5-{size.upper()}"
        results = run_single_model(model_id, label, system_prompt, samples)
        all_results[label] = results

        # Save individual results
        rfile = RESULTS_DIR / f"compare_{size}_{int(time.time())}.json"
        with open(rfile, "w") as f:
            json.dump({
                "model": model_id, "label": label,
                "prompt": system_prompt,
                "results": [asdict(r) for r in results],
            }, f, indent=2)

    print_comparison(all_results)

    # Save comparison summary
    summary = {}
    avg = lambda v: sum(v) / len(v) if v else 0
    for label, rs in all_results.items():
        n = len(rs)
        summary[label] = {
            "rouge_l": avg([r.rouge_l for r in rs]),
            "chrf": avg([r.chrf for r in rs]),
            "jaccard": avg([r.jaccard for r in rs]),
            "len_ratio": avg([r.len_ratio for r in rs]),
            "exact_rate": sum(1 for r in rs if r.exact) / n if n else 0,
            "zero_filler_rate": sum(1 for r in rs if r.filler_count == 0) / n if n else 0,
            "avg_latency": avg([r.elapsed_s for r in rs]),
            "avg_gen_tps": avg([r.gen_tps for r in rs]),
        }

    sfile = RESULTS_DIR / f"comparison_{int(time.time())}.json"
    with open(sfile, "w") as f:
        json.dump(summary, f, indent=2)
    print(f"\nComparison summary → {sfile}")


if __name__ == "__main__":
    main()
