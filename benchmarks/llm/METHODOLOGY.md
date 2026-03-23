# Transcript Cleanup Benchmark Methodology

## Overview

This document describes the methodology for benchmarking Qwen3-0.6B's transcript cleanup quality. The benchmark measures how well the model transforms raw speech-to-text output into clean, readable text.

## Dataset

**File:** `dataset.csv`
**Format:** CSV with columns: `id`, `category`, `raw`, `expected`, `notes`

### Categories

| Category | Count | Description |
|----------|-------|-------------|
| filler_removal | 15 | Filler words (uh, um) in various positions |
| crutch_words | 10 | "basically", "you know", hedging phrases |
| self_correction | 15 | "wait, actually", "no,", mid-speech corrections |
| false_start | 10 | Repeated/restarted sentence beginnings |
| grammar | 10 | Tense, agreement, punctuation issues |
| list_formatting | 10 | Numbered items spoken as prose |
| misheard_words | 10 | Speech recognition errors (oh auth → OAuth) |
| mixed | 15 | Multiple issues combined |
| short | 10 | Very brief utterances (2-10 words) |
| long_dictation | 5 | Multi-sentence passages (100+ words) |
| **Total** | **110** | |

### Adding Samples

New samples can be added to `generate_dataset.py` and regenerated:
```bash
python generate_dataset.py
```

Real-world transcription samples from SottoASR will be added over time as they become available. These will be placed in a separate `real_world` category to distinguish from synthetic samples.

## Metrics

### Primary Metrics

| Metric | What it measures | Ideal value | Why we use it |
|--------|-----------------|-------------|---------------|
| **ROUGE-L F1** | Longest common subsequence overlap with expected output | 1.0 | Captures both content preservation and word ordering. The primary quality metric. |
| **chrF** | Character n-gram F-score (n=1..6) | 1.0 | More robust than word-level metrics for short texts and minor wording differences. Good for catching spelling/formatting changes. |
| **Zero-Filler Rate** | % of samples with zero remaining fillers | 100% | Hard requirement: fillers must be removed. |

### Secondary Metrics

| Metric | What it measures | Ideal value | Why we use it |
|--------|-----------------|-------------|---------------|
| **Jaccard Similarity** | Word-set overlap (order-insensitive) | ~0.8-0.9 | Catches missing or added content. Lower than ROUGE-L because cleanup legitimately removes filler words. |
| **Length Ratio** | Output length / expected length | 0.9-1.1 | Detects excessive summarization (< 0.8) or hallucination/verbosity (> 1.3). |
| **Exact Match Rate** | % of samples matching expected exactly | Higher is better, but not primary | Useful for simple samples but too strict for complex ones where multiple valid outputs exist. |
| **Latency** | Time per sample on MPS | < 10s | Measures feasibility. ANE would be 4-7x faster. |

### Why Not These Metrics

- **BLEU:** Penalizes shorter outputs too heavily; our cleanup often produces shorter text.
- **BERTScore:** Requires loading another model; adds complexity without much benefit over ROUGE-L for this task.
- **Word Error Rate (WER):** Designed for ASR evaluation against reference transcripts, not for text-to-text cleanup.
- **Exact Match as primary:** Too strict — many valid cleanups differ from our single reference.

## Context Window Analysis

Based on SottoASR's 12-minute maximum recording limit:

| Speech Rate | Words/min | 12-min Total | Tokens | 512-ctx Chunks |
|------------|-----------|-------------|--------|----------------|
| Normal (130 wpm) | 130 | 1,560 | ~1,560 | ~10 |
| Fast (170 wpm) | 170 | 2,040 | ~2,040 | ~13 |
| Very Fast (200 wpm) | 200 | 2,400 | ~2,400 | ~15 |

- Qwen3's tokenizer averages ~1.0 tokens/word for English speech
- System prompt uses ~85 tokens (Standard mode) or ~112 tokens (Markdown mode)
- Chat template overhead: ~17 tokens
- Per chunk: ~200 words of input in Standard mode, ~180 in Markdown mode

**Implication:** A 12-minute recording requires 10-15 chunks on ANE (512 context). The full PyTorch model supports 32K+ context with no chunking needed.

**Prompt token overhead (measured):**
- Standard prompt (Cycle 3): **85 tokens**
- Markdown prompt: **112 tokens**
- Chat template overhead: **17 tokens**
- Total overhead: 102 tokens (Standard) or 129 tokens (Markdown)
- Available for input per chunk: ~200 words (Standard) or ~180 words (Markdown)

## ANE Testing

Direct ANE benchmarking requires `coremltools` with `libcoremlpython`, which is not compatible with Python 3.14 as of 2026-03-21. Quality benchmarks (ROUGE-L, chrF, etc.) are device-independent — the model produces identical outputs regardless of whether it runs on MPS or ANE.

For latency, ANE projections are based on published ANEMLL benchmarks:
- **ANE:** 47-62 tok/s at ~2W power draw
- **MPS (measured):** 7-15 tok/s at ~20W power draw
- **Projected speedup:** 4-7x faster on ANE

| Input Size | MPS Measured | ANE Projected |
|-----------|-------------|---------------|
| Short (50 words) | ~2s | ~0.3-0.5s |
| Medium (150 words) | ~4s | ~0.6-1.0s |
| Long (300+ words) | ~15-25s | ~3-5s |

## Running Benchmarks

```bash
# Full benchmark with default prompt
python run.py --cycle 1

# Specific category
python run.py --category self_correction

# Custom prompt
python run.py --prompt "Your system prompt here" --cycle 2

# Verbose (show all samples)
python run.py --verbose
```

Results are saved to `results/` as timestamped JSON files.

## Benchmark History

Results from each cycle are documented below.

### Results Summary (9 Cycles)

Cycles 1-6 ran on Qwen3-0.6B (PyTorch). Cycles 7-9 ran on Qwen3.5-2B-OptiQ-4bit (MLX).

| Cycle | Model | Prompt Style | ROUGE-L | chrF | Zero-Filler% | LenR | Exact% |
|-------|-------|-------------|---------|------|-------------|------|--------|
| 1 | 0.6B | conditional-v2 (original) | 0.774 | 0.769 | 91.8% | 1.11 | 26.4% |
| 2 | 0.6B | Simplified, no examples | 0.725 | 0.717 | 95.5% | 1.01 | 11.8% |
| **3** | **0.6B** | **"such as" examples + preserve** | **0.845** | **0.845** | 86.4% | 1.04 | 24.6% |
| 4 | 0.6B | Explicit filler list | 0.806 | 0.804 | 84.5% | 1.04 | 23.6% |
| 5 | 0.6B | Cycle 3 + more crutch words | 0.815 | 0.800 | 92.7% | 0.99 | 28.2% |
| 6 | 0.6B | Stronger filler emphasis | 0.816 | 0.803 | 89.1% | 0.97 | 21.8% |
| 7 | 2B | Explicit rules (v2) | 0.847 | 0.848 | 87.4% | 1.16 | 24.3% |
| 8 | 2B | Targeted crutch+selfcorr (v3) | 0.874 | 0.861 | 85.6% | 1.05 | 22.5% |
| 9 | 2B | Inline example for selfcorr (v4) | 0.866 | 0.845 | 88.3% | 0.99 | 18.9% |

### Model Comparison (Cycle 3 prompt, MLX OptiQ-4bit, 111 samples)

| Metric | 0.8B | 2B | 4B |
|--------|------|-----|-----|
| ROUGE-L | 0.833 | **0.880** | 0.850 |
| chrF | 0.854 | **0.868** | 0.847 |
| Length Ratio | 1.15 | **1.05** | 1.12 |
| Exact Match | 27.9% | 24.3% | **33.3%** |
| Zero-Filler | 82.9% | 85.6% | **91.0%** |
| Avg Latency (s) | **0.42** | 0.80 | 2.22 |
| Gen tok/s | **128** | 59 | 26 |

**2B is the best overall quality model.** The 4B model scores higher on filler removal but restructures/reformats long text instead of just cleaning it (long_dictation ROUGE-L: 0.739 vs 0.905 for 2B). The 0.8B model is fastest but summarizes complex inputs.

### Winner: Cycle 3 prompt on 2B model (ROUGE-L 0.880)

```
Fix this speech transcript. Remove all verbal fillers and hesitations such
as uh and um. Remove crutch phrases such as basically and you know. Fix
grammar and misheard words. Remove false starts where the speaker restarts
a sentence. When the speaker changes their mind, keep only the final version.
If the speaker lists items by number, format as a numbered list. Preserve all
meaningful content — do not summarize or shorten. Output only the cleaned text.
```

**Why Cycle 3 wins:**
- Highest ROUGE-L across all model sizes — best overall text quality
- "such as" examples teach the model what fillers look like without causing prompt echoing
- "Preserve all meaningful content" prevents over-summarization
- "do not summarize or shorten" is a strong negative instruction the model respects

**Trade-off:** Cycle 3 has a lower zero-filler rate (85.6% on 2B) than more aggressive prompts (Cycle 9: 88.3%). The quality improvement in content preservation outweighs the filler rate difference.

### Per-Category Performance (Cycle 3 prompt on 2B)

| Category | ROUGE-L | chrF | LenR | Exact% | Notes |
|----------|---------|------|------|--------|-------|
| misheard_words | 0.931 | 0.922 | 0.99 | 10% | Best category — strong domain knowledge |
| filler_removal | 0.925 | 0.901 | 0.91 | 47% | Core capability, very strong |
| long_dictation | 0.905 | 0.879 | 1.09 | 0% | Good content preservation (was 0.454 on 0.8B) |
| crutch_words | 0.895 | 0.899 | 1.05 | 30% | Good, some crutch words persist (so, I mean) |
| mixed | 0.873 | 0.893 | 1.08 | 27% | Good multi-issue handling |
| list_formatting | 0.863 | 0.779 | 0.90 | 0% | Improved over 0.8B (was 0.744) |
| false_start | 0.852 | 0.776 | 0.92 | 20% | Good false start removal |
| grammar | 0.806 | 0.863 | 0.98 | 30% | Solid grammar correction |
| self_correction | 0.801 | 0.833 | 1.44 | 27% | Weakest — keeps both versions |
| short | 0.975 | 0.799 | 1.01 | 20% | Excellent for brief utterances |

### Key Insights

1. **Model size matters more than prompt tuning.** Switching from 0.8B to 2B improved ROUGE-L from 0.833 to 0.880 — more than any prompt change across 9 cycles.
2. **The 4B model is NOT the best for cleanup.** Despite higher IFEval (89.8%), it restructures/reformats long text (ROUGE-L 0.739 on long_dictation vs 0.905 for 2B). It's too "creative" for a faithful cleanup task.
3. **Self-correction is a fundamental 2B limitation.** The model treats corrections ("actually", "no", "wait") as additive content rather than replacement signals. Prompt changes (Cycles 7-9) yielded only marginal improvement (0.801→0.814) with tradeoffs elsewhere.
4. **Residual fillers are mostly contextual.** The 24 remaining fillers are words like "so", "I mean", "okay" that serve double duty as connectors. Removing them aggressively hurts content quality.
5. **The 0.8B model summarizes long/complex inputs.** User-reported failure: a 100-word technical dictation was compressed to ~44% of its length. The 2B model preserved it at 103% (ROUGE-L 0.976).
6. **Prompt echoing risk** — Cycles 1 and 4 showed the model can repeat system prompt text. The "such as" pattern avoids this.
