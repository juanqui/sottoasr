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

Real-world transcription samples from Sotto will be added over time as they become available. These will be placed in a separate `real_world` category to distinguish from synthetic samples.

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

Based on Sotto's 12-minute maximum recording limit:

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

### Results Summary (6 Cycles)

| Cycle | Prompt Style | ROUGE-L | chrF | Zero-Filler% | LenR | Exact% |
|-------|-------------|---------|------|-------------|------|--------|
| 1 | conditional-v2 (original) | 0.774 | 0.769 | 91.8% | 1.11 | 26.4% |
| 2 | Simplified, no examples | 0.725 | 0.717 | 95.5% | 1.01 | 11.8% |
| **3** | **"such as" examples + preserve** | **0.845** | **0.845** | 86.4% | 1.04 | 24.6% |
| 4 | Explicit filler list | 0.806 | 0.804 | 84.5% | 1.04 | 23.6% |
| 5 | Cycle 3 + more crutch words | 0.815 | 0.800 | 92.7% | 0.99 | 28.2% |
| 6 | Stronger filler emphasis | 0.816 | 0.803 | 89.1% | 0.97 | 21.8% |

### Winner: Cycle 3

```
Fix this speech transcript. Remove all verbal fillers and hesitations such
as uh and um. Remove crutch phrases such as basically and you know. Fix
grammar and misheard words. Remove false starts where the speaker restarts
a sentence. When the speaker changes their mind, keep only the final version.
If the speaker lists items by number, format as a numbered list. Preserve all
meaningful content — do not summarize or shorten. Output only the cleaned text.
```

**Why Cycle 3 wins:**
- Highest ROUGE-L (0.845) and chrF (0.845) — best overall text quality
- "such as" examples teach the model what fillers look like without causing prompt echoing
- "Preserve all meaningful content" prevents over-summarization seen in Cycles 1-2
- "do not summarize or shorten" is a strong negative instruction the model respects
- Self-correction handling works well (keeps only final version)

**Trade-off:** Cycle 3 has a lower zero-filler rate (86.4%) than Cycle 5 (92.7%). Most remaining fillers are in long dictation (8/23) and short utterances (4/23). For production, the quality improvement (ROUGE-L +0.03 over Cycle 5) outweighs the filler rate difference.

### Per-Category Performance (Cycle 3)

| Category | ROUGE-L | chrF | Zero-Filler% | Exact% | Notes |
|----------|---------|------|-------------|--------|-------|
| short | 0.955 | 0.902 | 60% | 40% | Excellent for brief utterances |
| filler_removal | 0.925 | 0.901 | 87% | 47% | Strong core capability |
| crutch_words | 0.856 | 0.888 | 50% | 30% | Good, some crutch words persist |
| long_dictation | 0.854 | 0.862 | 0% | 0% | Good quality but fillers in long text |
| grammar | 0.818 | 0.826 | 90% | 20% | Solid grammar correction |
| false_start | 0.844 | 0.799 | 100% | 30% | Good false start removal |
| misheard_words | 0.803 | 0.870 | 100% | 30% | Decent, limited by model knowledge |
| mixed | 0.837 | 0.841 | 80% | 7% | Good multi-issue handling |
| self_correction | 0.796 | 0.827 | 100% | 27% | Works but sometimes keeps both versions |
| list_formatting | 0.756 | 0.723 | 100% | 0% | Weakest — model doesn't always format as lists |

### Key Insights

1. **Prompt echoing is a real risk** — Cycles 1 and 4 showed the model can repeat system prompt text as output when the prompt contains explicit word lists with parentheticals. The "such as" pattern avoids this.
2. **There's a quality-filler tradeoff** — More aggressive filler removal lowers ROUGE-L because the model over-trims meaningful content. The Cycle 3 prompt strikes the best balance.
3. **Self-correction handling is the hardest task** — ROUGE-L 0.796, LenR 1.32. The model sometimes keeps both the original and corrected versions, inflating output length.
4. **List formatting rarely produces exact matches** — The model formats lists differently from our expected output (different punctuation, numbering style) but the content is correct. chrF (0.723) is more representative than exact match (0%).
5. **Long dictation quality is good** (ROUGE-L 0.854) but has the most remaining fillers (8/23). This is because longer text has more opportunities for fillers to survive.
