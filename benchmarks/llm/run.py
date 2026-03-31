#!/usr/bin/env python3
"""Benchmark LLM transcript cleanup quality against the ground-truth dataset.

Metrics:
  1. ROUGE-L F1     — Longest common subsequence overlap (primary quality metric)
  2. chrF           — Character n-gram F-score (robust for short texts)
  3. Jaccard        — Word-set overlap (order-insensitive similarity)
  4. Length Ratio   — Output / Expected length (ideal: 0.9-1.1)
  5. Filler Residual— Count of remaining filler/crutch words in output
  6. Exact Match    — Binary: does output match expected exactly?

Usage:
    python run.py                          # Run full benchmark
    python run.py --category self_correction  # Single category
    python run.py --prompt "custom prompt"  # Custom system prompt
    python run.py --cycle 1                # Tag results with cycle number
"""

import argparse
import csv
import json
import re
import time
from collections import Counter, defaultdict
from pathlib import Path
from dataclasses import dataclass, field, asdict

MODEL_ID = "Qwen/Qwen3-0.6B"
CACHE_DIR = Path(__file__).parent / "model_cache"
DATASET = Path(__file__).parent / "dataset.csv"
RESULTS_DIR = Path(__file__).parent / "results"

PROMPTS_DIR = Path(__file__).parent / "prompts"
DEFAULT_PROMPT = (PROMPTS_DIR / "standard.txt").read_text().strip()

FILLERS = [
    "uh", "um", "uhm", "er", "like", "you know", "basically", "right", "yeah",
    "okay", "so", "i mean", "honestly", "literally", "anyway",
]


# ---------------------------------------------------------------------------
# Metrics
# ---------------------------------------------------------------------------

def _tokenize(text: str) -> list[str]:
    """Simple whitespace + punctuation tokenizer for metrics."""
    return re.findall(r"\w+", text.lower())


def rouge_l_f1(hypothesis: str, reference: str) -> float:
    """Compute ROUGE-L F1 score (longest common subsequence)."""
    hyp_tokens = _tokenize(hypothesis)
    ref_tokens = _tokenize(reference)
    if not hyp_tokens or not ref_tokens:
        return 1.0 if not hyp_tokens and not ref_tokens else 0.0

    m, n = len(ref_tokens), len(hyp_tokens)
    # LCS via DP
    dp = [[0] * (n + 1) for _ in range(m + 1)]
    for i in range(1, m + 1):
        for j in range(1, n + 1):
            if ref_tokens[i - 1] == hyp_tokens[j - 1]:
                dp[i][j] = dp[i - 1][j - 1] + 1
            else:
                dp[i][j] = max(dp[i - 1][j], dp[i][j - 1])
    lcs_len = dp[m][n]

    precision = lcs_len / n if n > 0 else 0
    recall = lcs_len / m if m > 0 else 0
    if precision + recall == 0:
        return 0.0
    return 2 * precision * recall / (precision + recall)


def chrf_score(hypothesis: str, reference: str, n: int = 6, beta: float = 2.0) -> float:
    """Compute chrF score (character n-gram F-score)."""
    def char_ngrams(text: str, order: int) -> Counter:
        ngrams = Counter()
        for i in range(len(text) - order + 1):
            ngrams[text[i:i + order]] += 1
        return ngrams

    total_precision = 0.0
    total_recall = 0.0
    count = 0

    hyp = hypothesis.lower()
    ref = reference.lower()

    for order in range(1, n + 1):
        hyp_ngrams = char_ngrams(hyp, order)
        ref_ngrams = char_ngrams(ref, order)

        common = sum((hyp_ngrams & ref_ngrams).values())
        hyp_total = sum(hyp_ngrams.values())
        ref_total = sum(ref_ngrams.values())

        precision = common / hyp_total if hyp_total > 0 else 0
        recall = common / ref_total if ref_total > 0 else 0

        total_precision += precision
        total_recall += recall
        count += 1

    avg_precision = total_precision / count if count > 0 else 0
    avg_recall = total_recall / count if count > 0 else 0

    if avg_precision + avg_recall == 0:
        return 0.0

    beta_sq = beta ** 2
    return (1 + beta_sq) * avg_precision * avg_recall / (beta_sq * avg_precision + avg_recall)


def jaccard_similarity(text_a: str, text_b: str) -> float:
    """Word-level Jaccard similarity."""
    words_a = set(_tokenize(text_a))
    words_b = set(_tokenize(text_b))
    if not words_a and not words_b:
        return 1.0
    intersection = words_a & words_b
    union = words_a | words_b
    return len(intersection) / len(union) if union else 0.0


def length_ratio(hypothesis: str, reference: str) -> float:
    """Length ratio (hypothesis / reference)."""
    if len(reference) == 0:
        return 1.0 if len(hypothesis) == 0 else float("inf")
    return len(hypothesis) / len(reference)


def count_fillers(text: str) -> int:
    """Count remaining filler/crutch words in text."""
    text_lower = f" {text.lower()} "
    count = 0
    for filler in FILLERS:
        # Match as whole word(s)
        pattern = rf"\b{re.escape(filler)}\b"
        count += len(re.findall(pattern, text_lower))
    return count


def exact_match(hypothesis: str, reference: str) -> bool:
    """Case-insensitive exact match after normalizing whitespace."""
    def normalize(t):
        return " ".join(t.lower().split())
    return normalize(hypothesis) == normalize(reference)


# ---------------------------------------------------------------------------
# Model
# ---------------------------------------------------------------------------

_model = None
_tokenizer = None


def load_model():
    global _model, _tokenizer
    if _model is not None:
        return _model, _tokenizer

    from transformers import AutoModelForCausalLM, AutoTokenizer

    print(f"Loading {MODEL_ID}...")
    _tokenizer = AutoTokenizer.from_pretrained(
        MODEL_ID, cache_dir=CACHE_DIR, trust_remote_code=True
    )
    _model = AutoModelForCausalLM.from_pretrained(
        MODEL_ID,
        cache_dir=CACHE_DIR,
        torch_dtype="auto",
        device_map="auto",
        trust_remote_code=True,
    )
    print(f"Loaded on {_model.device} ({_model.dtype})\n")
    return _model, _tokenizer


def generate(system_prompt: str, user_text: str, max_tokens: int = 1024) -> tuple[str, float]:
    """Generate and return (output_text, elapsed_seconds)."""
    model, tokenizer = load_model()

    messages = [
        {"role": "system", "content": system_prompt},
        {"role": "user", "content": user_text},
    ]
    text = tokenizer.apply_chat_template(
        messages, tokenize=False, add_generation_prompt=True, enable_thinking=False,
    )
    inputs = tokenizer(text, return_tensors="pt").to(model.device)
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
    response = tokenizer.decode(output_ids, skip_special_tokens=True).strip()
    return response, elapsed


# ---------------------------------------------------------------------------
# Benchmark Runner
# ---------------------------------------------------------------------------

@dataclass
class SampleResult:
    id: str
    category: str
    rouge_l: float
    chrf: float
    jaccard: float
    len_ratio: float
    filler_count: int
    exact_match: bool
    elapsed_s: float
    raw_len: int
    expected_len: int
    output_len: int
    output: str = ""


def run_benchmark(
    system_prompt: str,
    category_filter: str | None = None,
    cycle: int = 0,
    verbose: bool = False,
) -> list[SampleResult]:
    """Run benchmark against all samples, return per-sample results."""
    RESULTS_DIR.mkdir(exist_ok=True)

    # Load dataset
    with open(DATASET) as f:
        reader = csv.DictReader(f)
        samples = list(reader)

    if category_filter:
        samples = [s for s in samples if s["category"] == category_filter]

    print(f"Running benchmark: {len(samples)} samples")
    print(f"Prompt: {system_prompt[:80]}...")
    print("=" * 80)

    results: list[SampleResult] = []

    for i, sample in enumerate(samples):
        raw = sample["raw"]
        expected = sample["expected"]
        sid = sample["id"]
        cat = sample["category"]

        output, elapsed = generate(system_prompt, raw)

        r = SampleResult(
            id=sid,
            category=cat,
            rouge_l=rouge_l_f1(output, expected),
            chrf=chrf_score(output, expected),
            jaccard=jaccard_similarity(output, expected),
            len_ratio=length_ratio(output, expected),
            filler_count=count_fillers(output),
            exact_match=exact_match(output, expected),
            elapsed_s=elapsed,
            raw_len=len(raw),
            expected_len=len(expected),
            output_len=len(output),
            output=output,
        )
        results.append(r)

        status = "EXACT" if r.exact_match else f"R={r.rouge_l:.2f}"
        if verbose or not r.exact_match:
            filler_flag = f" FILLERS={r.filler_count}" if r.filler_count > 0 else ""
            print(f"  [{i+1:3d}/{len(samples)}] {sid:25s} | {status:10s} | chrF={r.chrf:.2f} | J={r.jaccard:.2f} | LR={r.len_ratio:.2f} | {r.elapsed_s:.1f}s{filler_flag}")

    # Save detailed results
    ts = int(time.time())
    tag = f"_cycle{cycle}" if cycle else ""
    results_file = RESULTS_DIR / f"benchmark{tag}_{ts}.json"
    with open(results_file, "w") as f:
        json.dump(
            {
                "prompt": system_prompt,
                "cycle": cycle,
                "timestamp": ts,
                "num_samples": len(results),
                "results": [asdict(r) for r in results],
            },
            f,
            indent=2,
        )

    # Print summary
    print_summary(results, system_prompt, cycle)
    print(f"\nDetailed results → {results_file}")

    return results


def print_summary(results: list[SampleResult], prompt: str = "", cycle: int = 0):
    """Print aggregate metrics."""
    n = len(results)
    if n == 0:
        print("No results.")
        return

    print(f"\n{'=' * 80}")
    if cycle:
        print(f"BENCHMARK RESULTS — Cycle {cycle}")
    else:
        print("BENCHMARK RESULTS")
    print(f"{'=' * 80}")

    # Overall metrics
    avg = lambda vals: sum(vals) / len(vals)
    metrics = {
        "ROUGE-L F1": avg([r.rouge_l for r in results]),
        "chrF": avg([r.chrf for r in results]),
        "Jaccard": avg([r.jaccard for r in results]),
        "Avg Length Ratio": avg([r.len_ratio for r in results]),
        "Exact Match Rate": sum(1 for r in results if r.exact_match) / n,
        "Zero-Filler Rate": sum(1 for r in results if r.filler_count == 0) / n,
        "Avg Latency (s)": avg([r.elapsed_s for r in results]),
        "Total Fillers Remaining": sum(r.filler_count for r in results),
    }
    print(f"\nOverall ({n} samples):")
    for name, val in metrics.items():
        if isinstance(val, float):
            print(f"  {name:25s}: {val:.4f}")
        else:
            print(f"  {name:25s}: {val}")

    # Per-category breakdown
    cats: dict[str, list[SampleResult]] = defaultdict(list)
    for r in results:
        cats[r.category].append(r)

    print(f"\n{'Category':25s} | {'N':>3s} | {'ROUGE-L':>7s} | {'chrF':>6s} | {'Jaccard':>7s} | {'LenR':>5s} | {'Exact':>5s} | {'Fillers':>7s}")
    print("-" * 90)
    for cat in sorted(cats.keys()):
        cr = cats[cat]
        cn = len(cr)
        print(
            f"{cat:25s} | {cn:>3d} | "
            f"{avg([r.rouge_l for r in cr]):>7.3f} | "
            f"{avg([r.chrf for r in cr]):>6.3f} | "
            f"{avg([r.jaccard for r in cr]):>7.3f} | "
            f"{avg([r.len_ratio for r in cr]):>5.2f} | "
            f"{sum(1 for r in cr if r.exact_match)/cn:>5.1%} | "
            f"{sum(r.filler_count for r in cr):>7d}"
        )

    # Worst samples (lowest ROUGE-L)
    worst = sorted(results, key=lambda r: r.rouge_l)[:5]
    print(f"\nWorst 5 samples by ROUGE-L:")
    for r in worst:
        print(f"  {r.id:25s} | ROUGE-L={r.rouge_l:.3f} | chrF={r.chrf:.3f} | fillers={r.filler_count}")


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(description="Benchmark LLM transcript cleanup")
    parser.add_argument("--category", type=str, help="Filter to a specific category")
    parser.add_argument("--prompt", type=str, help="Custom system prompt")
    parser.add_argument("--cycle", type=int, default=0, help="Tag results with cycle number")
    parser.add_argument("--verbose", action="store_true", help="Show all samples, not just non-exact")
    args = parser.parse_args()

    prompt = args.prompt or DEFAULT_PROMPT
    run_benchmark(prompt, args.category, args.cycle, args.verbose)


if __name__ == "__main__":
    main()
