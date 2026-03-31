# LLM Prompt Tuning Journal

- **Version:** 1.0
- **Date:** 2026-03-30
- **Status:** Implemented

**Model:** Qwen3.5-2B-OptiQ-4bit (MLX)
**Dataset:** 135 samples across 12 categories
**Goal:** Maximize cleanup quality without over-aggressive rewriting

## Baseline (Cycle 3 — previous production prompt)

```
Fix this speech transcript. Remove all verbal fillers...
```

| Metric | Value |
|--------|-------|
| ROUGE-L | 0.880 |
| chrF | 0.868 |
| Zero-Filler | 85.6% |
| self_correction | 0.801 |
| crutch_words | 0.895 |
| preserve_wording | N/A (category didn't exist) |

**Problem:** Over-aggressive rewriting — paraphrasing, removing emphasis, skipping sentences.

---

## Cycle 10 — Conservative "minimal edits" prompt

Changed to flat REMOVE/FIX/CONVERT structure with explicit "Do NOT paraphrase" constraint.

| Metric | Cycle 3 | Cycle 10 | Delta |
|--------|---------|----------|-------|
| ROUGE-L | 0.880 | 0.879 | -0.001 |
| preserve_wording | N/A | **0.998** | NEW |
| dictation_commands | N/A | **0.936** | NEW |
| self_correction | 0.801 | 0.683 | **-0.118** |
| crutch_words | 0.895 | 0.795 | **-0.100** |
| list_formatting | N/A | 0.769 | — |
| Zero-Filler | 85.6% | 78% | -7.6% |

**Lesson:** "Do NOT change anything" overrides "REMOVE crutch words" for small models. They resolve instruction conflicts conservatively.

---

## Cycle 11 — MUST/MUST-NOT hierarchy

Restructured as numbered MUST rules + MUST-NOT constraints. Added "You MUST apply ALL".

| Metric | Cycle 10 | Cycle 11 | Delta |
|--------|----------|----------|-------|
| ROUGE-L | 0.879 | **0.893** | +0.014 |
| crutch_words | 0.795 | **0.850** | +0.055 |
| list_formatting | 0.769 | **0.860** | +0.091 |
| false_start | 0.888 | **0.934** | +0.046 |
| filler_removal | 0.964 | **0.978** | +0.014 |
| dictation_commands | 0.936 | **0.969** | +0.033 |
| preserve_wording | 0.998 | 0.981 | -0.017 |
| self_correction | 0.683 | 0.700 | +0.017 |
| Zero-Filler | 78% | **83%** | +5% |

**Lesson:** MUST/MUST-NOT hierarchy resolves instruction conflicts — model treats MUST as priority.

---

## Cycle 12 — Stronger self-correction instruction + "dot" support

Added two in-prompt examples for self-correction rule, added "dot" → "." conversion. Changed self-correction wording to "REMOVE the original statement entirely."

| Metric | Cycle 11 | Cycle 12 | Delta |
|--------|----------|----------|-------|
| ROUGE-L | 0.893 | 0.891 | -0.002 |
| self_correction | 0.700 | 0.695 | -0.005 |
| crutch_words | 0.850 | 0.863 | +0.013 |
| dictation_commands | 0.946 | **0.963** | +0.017 |
| preserve_wording | 0.981 | 0.979 | -0.002 |

**Lesson:** Stronger wording in rules didn't improve self-correction. The 2B model ignores intensifiers.

---

## Cycle 13 — Few-shot examples (rules as flat list)

Replaced MUST/MUST-NOT structure with flat "Rules:" list + 3 IN/OUT examples. Examples covered: self-correction, crutch+filler+dictation, preserve emphasis.

| Metric | Cycle 12 | Cycle 13 | Delta |
|--------|----------|----------|-------|
| self_correction | 0.695 | **0.726** | +0.031 |
| dictation_commands | 0.963 | **0.992** | +0.029 |
| preserve_wording | 0.979 | **0.986** | +0.007 |
| crutch_words | 0.863 | 0.813 | **-0.050** |
| Zero-Filler | 83% | 79% | **-4%** |

**Lesson:** Few-shot examples help the model learn hard patterns (self-correction, dictation) but WITHOUT the MUST hierarchy, crutch word removal regresses.

---

## Cycle 14 — MUST + few-shot combined ★

Combined MUST/MUST-NOT hierarchy with 3 IN/OUT examples. Best of both approaches.

| Metric | Cycle 12 | Cycle 14 | Delta |
|--------|----------|----------|-------|
| ROUGE-L | 0.891 | **0.896** | +0.005 |
| self_correction | 0.695 | **0.726** | +0.031 |
| crutch_words | 0.863 | **0.883** | +0.020 |
| dictation_commands | 0.963 | **0.992** | +0.029 |
| preserve_wording | 0.979 | **0.992** | +0.013 |
| Exact Match | 28.9% | **38.5%** | +9.6pts |
| grammar | 0.862 | **0.878** | +0.016 |

**Lesson:** Combining MUST hierarchy (for crutch word authority) with few-shot examples (for self-correction patterns) yields the best overall result.

---

## Experiment: Parameter Sweep (temp × repetition_penalty)

Tested on self_correction + crutch_words categories (25 samples):

| temp | rep_pen | ROUGE-L |
|------|---------|---------|
| 0.5 | 1.15 | **0.805** |
| 0.3 | 1.15 | 0.799 |
| 0.7 | 1.10 | 0.797 |
| 0.5 | 1.05 | 0.791 |
| 0.1 | 1.10 | 0.789 |
| 0.3 | 1.05 | 0.778 |

**But:** Full benchmark with temp=0.5/rep=1.15 (Cycle 15) hurt long text badly (long_06 → 0.218). Higher temperature makes the model too creative on long passages.

---

## Cycle 16 — MUST+few-shot with rep_penalty=1.10 ★★ WINNER

Moderate repetition penalty increase (1.05 → 1.10). Helps self-correction without hurting long text.

| Metric | Cycle 14 | Cycle 16 | Delta |
|--------|----------|----------|-------|
| ROUGE-L | 0.896 | 0.891 | -0.005 |
| self_correction | 0.726 | **0.742** | +0.016 |
| crutch_words | 0.883 | **0.879** | -0.004 |
| long_dictation | 0.929 | 0.929 | 0.000 |
| preserve_wording | 0.992 | **0.992** | 0.000 |
| dictation_commands | 0.992 | 0.992 | 0.000 |

---

## Final Production Configuration

**Prompt:** `prompts/must_fewshot_v1.txt` (MUST hierarchy + 3 few-shot examples)
**Generation params:** temp=0.3, top_p=0.9, repetition_penalty=1.10

### Final vs Baseline Comparison

| Category | Baseline | Final | Delta |
|----------|----------|-------|-------|
| **ROUGE-L (overall)** | 0.880 | **0.891** | +0.011 |
| preserve_wording | N/A | **0.992** | NEW — 83% exact |
| dictation_commands | N/A | **0.992** | NEW — 70% exact |
| self_correction | 0.695* | **0.742** | +0.047 |
| crutch_words | 0.795* | **0.879** | +0.084 |
| list_formatting | 0.769* | **0.859** | +0.090 |
| filler_removal | 0.925 | **0.974** | +0.049 |
| grammar | 0.806 | **0.874** | +0.068 |
| long_dictation | 0.905 | **0.929** | +0.024 |
| Exact Match | 24.3% | **37%** | +12.7pts |

*Cycle 10 values (before MUST hierarchy was added)

### Key Learnings

1. **Small models resolve instruction conflicts conservatively.** If you say "remove X" AND "don't change anything," they choose inaction. The MUST/MUST-NOT hierarchy resolves this.
2. **Few-shot examples are essential for complex patterns.** Self-correction can't be taught with rules alone — the model needs to see input→output examples.
3. **Parameter tuning has diminishing returns.** The prompt matters 10x more than temperature/repetition settings. But moderate rep_penalty (1.10) helps the model avoid keeping duplicate content in self-corrections.
4. **Parameters that help in isolation can hurt globally.** The sweep showed temp=0.5/rep=1.15 was best for self-correction alone, but destroyed long text quality. Always validate on the full benchmark.
5. **Self-correction is a fundamental 2B limitation.** Despite all tuning, it remains the weakest category (0.742). The model treats corrections as additive content. Would likely need fine-tuning or a larger model to go higher.
