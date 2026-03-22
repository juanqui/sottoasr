# Qwen3-0.6B Viability Assessment for SottoASR Transcript Cleanup

**Date:** 2026-03-21
**Model:** Qwen/Qwen3-0.6B (751M params, bfloat16)
**Test environment:** MPS (Metal GPU) on Apple Silicon
**ANE projections:** Based on ANEMLL benchmarks of 47-62 tok/s

## Executive Summary

**Qwen3-0.6B IS viable for transcript cleanup.** It excels at filler removal
and grammar correction, handles list formatting well with the right prompt,
and produces good Markdown output. The main weakness is limited domain knowledge
for correcting specialized misheard words (e.g., "talks" → "tokens",
"core M L" → "CoreML").

## Benchmark Results

### Memory
| Metric | Value |
|--------|-------|
| Model footprint | ~593 MB (bfloat16) |
| Parameters | 751,632,384 |

### Latency (MPS vs ANE Projected)

| Input Size | In Tokens | Out Tokens | MPS Time | MPS tok/s | ANE Projected |
|-----------|-----------|------------|----------|-----------|---------------|
| Tiny (15 words) | 58 | 13 | 1.3s | 9.8 | ~0.2-0.3s |
| Short (50 words) | 101 | 43 | 6.1s | 7.0 | ~0.7-1.0s |
| Medium (150 words) | 191 | 114 | 12.1s | 9.4 | ~1.8-2.4s |
| Long (300+ words) | 381 | 205 | 22.9s | 8.9 | ~3.3-4.4s |

ANE projections assume 50 tok/s (conservative mid-range of 47-62 tok/s).

### Context Window
The full PyTorch model handles 128, 256, 512, and 1024+ tokens without issues.
The ANEMLL-converted version is limited to 512 tokens, requiring chunking for
inputs >~150 words after accounting for the system prompt.

## Prompt Experiment Results

### Standard Mode (6 prompts × 8 samples)

#### Best Prompt: "Conditional" (Prompt A)

```
Fix this transcript: remove fillers (uh, um, like, you know), fix grammar,
fix misheard words. If the speaker explicitly numbers items (saying one,
two, three), format those as a numbered list. Otherwise keep as prose.
Output only the fixed text.
```

**Updated to conditional-v2 (2026-03-21):**

```
Fix this transcript: remove fillers (uh, um, like, you know, basically, right,
yeah), fix grammar, fix misheard words. Remove false starts. When the speaker
corrects themselves (wait, no, actually), keep only their final intent. If the
speaker explicitly numbers items (one, two, three), format those as a numbered
list. Otherwise keep as prose. Output only the fixed text.
```

**Why this wins:**
- Removes 100% of filler words AND crutch words across all test samples
- Handles self-corrections: "deploy to A, wait, actually B" → "deploy to B"
- Removes false starts: "The API should, the API needs to" → "The API needs to"
- Correctly formats numbered items as lists without over-applying
- Uses ~65 system tokens, leaving sufficient budget for input/output

#### Prompt Comparison Summary

| Prompt | Fillers | Self-Corrections | Lists | Prose | Misheard | Speed |
|--------|:-:|:-:|:-:|:-:|:-:|:-:|
| minimal | ✅ | ❌ | ❌ | ✅ | Partial | Fast |
| structured | ✅ | ❌ | Partial | ✅ | Partial | Medium |
| few_shot | ✅ | ❌ | ✅ (slow) | ✅ | Partial | Slow |
| transcriptionist | ✅ | ❌ | ❌ | ⚠️ | Partial | Medium |
| concise | ✅ | ❌ | ❌ | ✅ | Partial | Fast |
| decomposed | ✅ | ❌ | Partial | ✅ | Partial | Medium |
| conditional (A) | ✅ | ❌ | ✅ | ✅ | Partial | Medium |
| **conditional-v2** | ✅ | ✅ | ✅ | ✅ | Partial | Medium |

**conditional-v2** adds self-correction handling ("wait, actually", "no", false starts)
and expanded crutch word removal (basically, right, yeah) to the original conditional prompt.
Tested against 14 samples (8 original + 6 new self-correction/crutch word samples).

### Markdown Mode (3 prompts × 2 samples)

#### Best Prompt: "Structured"

```
You are a transcript-to-markdown converter. Take the raw speech transcript
and convert it into well-structured Markdown.

Rules:
1. Remove filler words (uh, um, like, you know)
2. Fix grammar and misheard words
3. Organize content with headings (## for main topics)
4. Use bullet lists for items and details
5. Use numbered lists for sequential items or action items
6. Use bold for emphasis on key terms
7. Keep all information — do not summarize

Output ONLY the markdown, no commentary.
```

**Why this wins:**
- Produces proper heading hierarchy (##, ###)
- Uses bold for key terms
- Converts spoken numbers to digits (forty-seven → 47)
- Preserves all content (length ratio ~1.0)
- Separates action items into their own section

#### Markdown Prompt Comparison

| Prompt | Structure Quality | Content Preservation | Speed |
|--------|:-:|:-:|:-:|
| **markdown_structured** | ✅ Headings + bullets + bold | ✅ ~1.0 ratio | 10-12 tok/s |
| markdown_minimal | ⚠️ Flat bullets, no headings | ✅ ~0.9 ratio | 11-13 tok/s |
| markdown_few_shot | ⚠️ Sparse headings | ❌ ~0.5-0.6 ratio (summarizes) | 13-14 tok/s |

## Quality Analysis by Task

### 1. Filler Removal — EXCELLENT ✅
All prompts achieve 100% filler removal across all samples. Even single-word
fillers mid-sentence ("the uh dentist") are cleanly removed.

### 2. Grammar Correction — VERY GOOD ✅
- "gonna" → "going to" ✅
- Missing punctuation added ✅
- Sentence fragments fixed ✅
- Subject-verb agreement fixed ✅
- "its" → "isn't" (contraction fixes) ✅

### 3. List Formatting — GOOD (with right prompt) ✅
- Numbered items (one, two, three) → 1. 2. 3. ✅ (with conditional prompt)
- Grocery list → bullet or numbered list ✅
- Technical steps → numbered list ✅
- Does NOT format non-lists as lists ✅ (with conditional prompt)

### 4. Misheard Word Correction — PARTIAL ⚠️
| Input | Model Output | Expected | Verdict |
|-------|-------------|----------|---------|
| "oh auth two point oh" | "OAuth 2.0" | "OAuth 2.0" | ✅ |
| "open API" | "OpenAPI" | "OpenAPI" | ✅ |
| "basic off" | "basic authentication" | "basic auth" | ✅ (improved) |
| "core M L" | "core ML" or "Apple ML" | "CoreML" | ⚠️ Close |
| "talks per second" | "talks per second" | "tokens per second" | ❌ |
| "Quentin 3" | "Quen30.6B" (kept) | "Qwen3-0.6B" | ❌ |
| "front end" / "back end" | "frontend" / "backend" | Correct | ✅ |

The model has good general knowledge (OAuth, OpenAPI) but lacks specialized
ML/AI terminology for compound words and domain jargon.

### 5. Markdown Formatting — GOOD ✅
- Proper heading hierarchy ✅
- Bullet and numbered lists ✅
- Bold emphasis on key terms ✅
- Action items separated ✅
- Content preservation with structured prompt ✅

### 6. Long Text Handling — ACCEPTABLE ⚠️
- Tends to compress/summarize text >200 words (length ratio ~0.75)
- Still removes fillers and fixes grammar
- May lose some nuance in paraphrasing
- Chunking will be needed for ANE's 512-token limit

## Recommendations

### For Implementation

1. **Use the "conditional" prompt for Standard mode** — best balance of quality and speed
2. **Use the "structured" prompt for Markdown mode** — best structure and content preservation
3. **Skip cleanup for inputs <5 words** — too little context for meaningful improvement
4. **Implement chunking at sentence boundaries** for inputs >~150 words (ANE 512-token limit)
5. **Add output validation** — reject output if length < 30% or > 180% of input
6. **Fallback gracefully** — if cleanup fails or produces garbage, use raw text

### Known Limitations to Document for Users

1. Domain-specific jargon may not be corrected (specialized terms, product names)
2. Very long dictations may be slightly compressed
3. Additional ~1-3s latency per transcription (ANE)
4. ~600 MB model download required

### Future Improvements

1. **Custom vocabulary** — Let users add domain terms the model should recognize
2. **Larger model option** — Qwen3-1.7B on GPU for users who want better quality at higher power
3. **Prompt caching** — Pre-tokenize system prompt to save tokens on each call
4. **Streaming** — Show cleaned text as it generates (better perceived latency)
