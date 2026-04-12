#!/usr/bin/env python3
"""Benchmark the SottoASR production MLX model using the EXACT production
sidecar format and generation parameters.

This script mirrors src-tauri/sidecar/llm_cleanup.py:
  - Prompt format: "### Input:\n{text}\n\n### Output:\n"
  - Greedy decoding (temp=0.0)
  - max_tokens = max(256, int(words * 1.5))
  - Post-process: strip and truncate at "###" marker
  - Skip inputs < 5 words (returns input unchanged)
  - Same MLX memory limits as sidecar

Usage:
    python run_production.py --run-dir results/20260411-run1 \
        --model juanquivilla/sotto-cleanup-lfm25-350m-mlx-5bit
"""

import argparse
import csv
import json
import re
import time
from collections import defaultdict
from dataclasses import dataclass, asdict
from pathlib import Path


DATASET = Path(__file__).parent / "dataset.csv"

FILLERS = [
    "uh", "um", "uhm", "er", "like", "you know", "basically", "right", "yeah",
    "okay", "so", "i mean", "honestly", "literally", "anyway",
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
            dp[i][j] = (
                dp[i - 1][j - 1] + 1
                if r[i - 1] == h[j - 1]
                else max(dp[i - 1][j], dp[i][j - 1])
            )
    lcs = dp[m][n]
    p = lcs / n if n else 0
    rc = lcs / m if m else 0
    return 2 * p * rc / (p + rc) if (p + rc) else 0.0


def chrf_score(hyp, ref, n=6, beta=2.0):
    from collections import Counter

    def cng(t, o):
        return Counter(t[i : i + o] for i in range(len(t) - o + 1))

    tp = tr = 0.0
    c = 0
    h, r = hyp.lower(), ref.lower()
    for o in range(1, n + 1):
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
    raw: str
    expected: str
    output: str
    identity_passthrough: bool
    skipped_short: bool
    rouge_l: float
    chrf: float
    jaccard: float
    len_ratio: float
    filler_count_in: int
    filler_count_out: int
    exact: bool
    elapsed_s: float
    prompt_tokens: int
    gen_tokens: int
    prompt_tps: float
    gen_tps: float
    has_double_newline: bool
    raw_word_count: int
    output_word_count: int


def run(model_id: str, run_dir: Path):
    import gc

    import mlx.core as mx
    from mlx_lm import load, stream_generate
    from mlx_lm.sample_utils import make_sampler

    run_dir.mkdir(parents=True, exist_ok=True)

    # Match production sidecar memory limits
    mx.set_memory_limit(1024 * 1024 * 1024)
    mx.set_cache_limit(128 * 1024 * 1024)

    print(f"[load] {model_id}")
    t0 = time.perf_counter()
    model, tokenizer = load(model_id)
    print(f"[load] done in {time.perf_counter() - t0:.2f}s")

    sampler = make_sampler(temp=0.0)  # Greedy — matches production

    # Warmup (same as sidecar)
    print("[warmup] running ...")
    warmup_gen = stream_generate(
        model,
        tokenizer,
        prompt="### Input:\nhello\n\n### Output:\n",
        max_tokens=8,
        sampler=sampler,
    )
    for _ in warmup_gen:
        pass
    mx.clear_cache()
    gc.collect()
    print("[warmup] done")

    with open(DATASET) as f:
        samples = list(csv.DictReader(f))
    print(f"[bench] {len(samples)} samples")

    results = []
    total_prompt_tokens = 0
    total_gen_tokens = 0
    total_time = 0.0

    for i, sample in enumerate(samples):
        raw = sample["raw"]
        expected = sample["expected"]
        word_count = len(raw.split())

        # Production sidecar skips inputs < 5 words (returns as-is)
        if word_count < 5:
            output = raw
            elapsed = 0.0
            prompt_tokens = gen_tokens = 0
            prompt_tps = gen_tps = 0.0
            skipped_short = True
        else:
            prompt = f"### Input:\n{raw}\n\n### Output:\n"
            max_output_tokens = max(256, int(word_count * 1.5))

            mx.clear_cache()
            segments = []
            last_resp = None
            start = time.perf_counter()
            for resp in stream_generate(
                model,
                tokenizer,
                prompt=prompt,
                max_tokens=max_output_tokens,
                sampler=sampler,
            ):
                segments.append(resp.text)
                last_resp = resp
            elapsed = time.perf_counter() - start
            mx.clear_cache()

            full = "".join(segments).strip()
            if "###" in full:
                full = full[: full.index("###")].strip()
            output = full

            prompt_tokens = last_resp.prompt_tokens if last_resp else 0
            gen_tokens = last_resp.generation_tokens if last_resp else 0
            prompt_tps = last_resp.prompt_tps if last_resp else 0.0
            gen_tps = last_resp.generation_tps if last_resp else 0.0
            skipped_short = False

        total_prompt_tokens += prompt_tokens
        total_gen_tokens += gen_tokens
        total_time += elapsed

        len_ratio = (
            len(output) / len(expected)
            if expected
            else (1.0 if not output else float("inf"))
        )

        identity_passthrough = (
            " ".join(output.lower().split()) == " ".join(raw.lower().split())
        )

        r = Result(
            id=sample["id"],
            category=sample["category"],
            raw=raw,
            expected=expected,
            output=output,
            identity_passthrough=identity_passthrough,
            skipped_short=skipped_short,
            rouge_l=rouge_l_f1(output, expected),
            chrf=chrf_score(output, expected),
            jaccard=jaccard(output, expected),
            len_ratio=len_ratio,
            filler_count_in=count_fillers(raw),
            filler_count_out=count_fillers(output),
            exact=exact_match(output, expected),
            elapsed_s=elapsed,
            prompt_tokens=prompt_tokens,
            gen_tokens=gen_tokens,
            prompt_tps=prompt_tps,
            gen_tps=gen_tps,
            has_double_newline=("\n\n" in output),
            raw_word_count=word_count,
            output_word_count=len(output.split()),
        )
        results.append(r)

        flag = ""
        if identity_passthrough and not skipped_short:
            flag += " IDENTITY"
        if skipped_short:
            flag += " SKIP<5"
        if not output:
            flag += " EMPTY"
        if r.filler_count_out > 0:
            flag += f" F={r.filler_count_out}"
        status = "EXACT" if r.exact else f"R={r.rouge_l:.2f}"
        print(
            f"  [{i+1:3d}/{len(samples)}] {r.id:20s} | {r.category:18s} | "
            f"{status:10s} | chrF={r.chrf:.2f} | {elapsed:5.2f}s | "
            f"{gen_tps:5.1f}t/s{flag}"
        )

    # Aggregate
    n = len(results)
    avg = lambda v: sum(v) / len(v) if v else 0.0
    considered = [r for r in results if not r.skipped_short]  # for scores
    exact_count = sum(1 for r in results if r.exact)
    zero_filler = sum(1 for r in results if r.filler_count_out == 0)
    identity_non_short = sum(
        1 for r in results if r.identity_passthrough and not r.skipped_short
    )
    empty = sum(1 for r in results if not r.output.strip())
    has_double_nl = sum(1 for r in results if r.has_double_newline)

    summary = {
        "model": model_id,
        "run_dir": str(run_dir),
        "n_samples": n,
        "n_processed": len(considered),
        "n_skipped_short": n - len(considered),
        "rouge_l_all": avg([r.rouge_l for r in results]),
        "rouge_l_processed": avg([r.rouge_l for r in considered]),
        "chrf_all": avg([r.chrf for r in results]),
        "jaccard_all": avg([r.jaccard for r in results]),
        "len_ratio_all": avg(
            [r.len_ratio for r in results if r.len_ratio != float("inf")]
        ),
        "exact_match_rate_all": exact_count / n if n else 0.0,
        "exact_match_rate_processed": (
            sum(1 for r in considered if r.exact) / len(considered)
            if considered
            else 0.0
        ),
        "zero_filler_rate_all": zero_filler / n if n else 0.0,
        "total_fillers_out": sum(r.filler_count_out for r in results),
        "total_fillers_in": sum(r.filler_count_in for r in results),
        "identity_passthrough_count": identity_non_short,
        "empty_output_count": empty,
        "has_double_newline_count": has_double_nl,
        "avg_latency_s": avg([r.elapsed_s for r in considered]),
        "avg_gen_tps": (total_gen_tokens / total_time) if total_time else 0.0,
        "total_wall_time_s": total_time,
        "per_category": {},
    }

    cats = defaultdict(list)
    for r in results:
        cats[r.category].append(r)
    for cat, cr in cats.items():
        cn = len(cr)
        summary["per_category"][cat] = {
            "n": cn,
            "rouge_l": avg([r.rouge_l for r in cr]),
            "chrf": avg([r.chrf for r in cr]),
            "exact_match_rate": sum(1 for r in cr if r.exact) / cn,
            "zero_filler_rate": sum(1 for r in cr if r.filler_count_out == 0) / cn,
            "total_fillers_out": sum(r.filler_count_out for r in cr),
            "identity_passthrough": sum(
                1 for r in cr if r.identity_passthrough and not r.skipped_short
            ),
        }

    # Save
    (run_dir / "results.json").write_text(
        json.dumps(
            {"summary": summary, "results": [asdict(r) for r in results]},
            indent=2,
        )
    )

    # Print summary
    print(f"\n{'='*80}")
    print(f"RESULTS — {model_id}")
    print(f"Run dir: {run_dir}")
    print(f"{'='*80}")
    print(f"  Samples:           {n} ({len(considered)} processed, "
          f"{n - len(considered)} skipped <5 words)")
    print(f"  ROUGE-L (all):     {summary['rouge_l_all']:.4f}")
    print(f"  chrF:              {summary['chrf_all']:.4f}")
    print(f"  Exact Match (all): {summary['exact_match_rate_all']:.1%}")
    print(
        f"  Zero-Filler (all): {summary['zero_filler_rate_all']:.1%} "
        f"(total fillers out: {summary['total_fillers_out']} / in: "
        f"{summary['total_fillers_in']})"
    )
    print(f"  Identity pass:     {identity_non_short} / {len(considered)}")
    print(f"  Empty outputs:     {empty}")
    print(f"  Outputs w/ \\n\\n:    {has_double_nl}")
    print(f"  Avg latency:       {summary['avg_latency_s']:.3f}s")
    print(f"  Avg gen tok/s:     {summary['avg_gen_tps']:.1f}")

    print(f"\n{'Category':20s} | {'N':>3s} | {'ROUGE-L':>7s} | {'chrF':>6s} | "
          f"{'Exact':>6s} | {'0Fill':>6s} | {'Fills':>5s} | {'Ident':>5s}")
    print("-" * 88)
    for cat in sorted(summary["per_category"]):
        m = summary["per_category"][cat]
        print(
            f"{cat:20s} | {m['n']:>3d} | {m['rouge_l']:>7.3f} | {m['chrf']:>6.3f} "
            f"| {m['exact_match_rate']:>6.1%} | {m['zero_filler_rate']:>6.1%} "
            f"| {m['total_fillers_out']:>5d} | {m['identity_passthrough']:>5d}"
        )

    worst = sorted(considered, key=lambda r: r.rouge_l)[:10]
    print(f"\nWorst 10 (by ROUGE-L):")
    for r in worst:
        print(
            f"  {r.id:20s} | R={r.rouge_l:.3f} | fillers={r.filler_count_out} "
            f"| ident={r.identity_passthrough}"
        )

    print(f"\nFull per-example results: {run_dir / 'results.json'}")


if __name__ == "__main__":
    p = argparse.ArgumentParser()
    p.add_argument(
        "--model",
        default="juanquivilla/sotto-cleanup-lfm25-350m-mlx-5bit",
    )
    p.add_argument("--run-dir", required=True, type=Path)
    args = p.parse_args()
    run(args.model, args.run_dir)
