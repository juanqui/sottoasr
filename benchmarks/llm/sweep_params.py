#!/usr/bin/env python3
"""Quick parameter sweep on specific categories to find optimal temp/repetition_penalty."""

import csv
import re
import time
from pathlib import Path

DATASET = Path(__file__).parent / "dataset.csv"
PROMPTS_DIR = Path(__file__).parent / "prompts"

PROMPT = (PROMPTS_DIR / "must_fewshot_v1.txt").read_text().strip()


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


def sweep(categories, temps, rep_penalties):
    from mlx_lm import load, stream_generate
    from mlx_lm.sample_utils import make_sampler, make_logits_processors

    model_id = "mlx-community/Qwen3.5-2B-OptiQ-4bit"
    print(f"Loading {model_id}...")
    model, tokenizer = load(model_id)

    with open(DATASET) as f:
        all_samples = list(csv.DictReader(f))
    samples = [s for s in all_samples if s["category"] in categories]
    print(f"Testing {len(samples)} samples from {categories}\n")

    results = []

    for temp in temps:
        for rp in rep_penalties:
            sampler = make_sampler(temp=temp, top_p=0.9)
            lps = make_logits_processors(repetition_penalty=rp)

            scores = []
            for s in samples:
                messages = [
                    {"role": "system", "content": PROMPT},
                    {"role": "user", "content": s["raw"]},
                ]
                prompt = tokenizer.apply_chat_template(
                    messages, tokenize=False, add_generation_prompt=True,
                    enable_thinking=False,
                )
                segments = []
                for resp in stream_generate(
                    model, tokenizer, prompt=prompt, max_tokens=4096,
                    sampler=sampler, logits_processors=lps,
                ):
                    segments.append(resp.text)
                output = "".join(segments).strip()
                output = re.sub(r"<think>.*?</think>", "", output, flags=re.DOTALL).strip()
                output = re.sub(r"<think>.*", "", output, flags=re.DOTALL).strip()
                scores.append(rouge_l_f1(output, s["expected"]))

            avg = sum(scores) / len(scores)
            results.append((temp, rp, avg))
            print(f"  temp={temp:.1f}  rep_pen={rp:.2f}  →  ROUGE-L={avg:.4f}")

    print(f"\n{'='*60}")
    print("SWEEP RESULTS (sorted by ROUGE-L)")
    print(f"{'='*60}")
    for temp, rp, avg in sorted(results, key=lambda x: -x[2]):
        print(f"  temp={temp:.1f}  rep_pen={rp:.2f}  →  ROUGE-L={avg:.4f}")


if __name__ == "__main__":
    sweep(
        categories=["self_correction", "crutch_words"],
        temps=[0.1, 0.3, 0.5, 0.7],
        rep_penalties=[1.0, 1.05, 1.1, 1.15],
    )
