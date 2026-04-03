---
license: mit
language:
- en
base_model: LiquidAI/LFM2.5-350M-Base
tags:
- speech-to-text
- transcript-cleanup
- text-correction
- asr-post-processing
- LFM
- LiquidAI
pipeline_tag: text-generation
datasets:
- juanquivilla/sotto-transcript-cleanup
---

# SottoASR Transcript Cleanup — LFM2.5-350M (Full Precision)

[sottoasr.app](https://sottoasr.app) · [MLX 5-bit (recommended)](https://huggingface.co/juanquivilla/sotto-cleanup-lfm25-350m-mlx-5bit) · [MLX 4-bit (smaller)](https://huggingface.co/juanquivilla/sotto-cleanup-lfm25-350m-mlx-4bit) · [Training Dataset](https://huggingface.co/datasets/juanquivilla/sotto-transcript-cleanup)

## Overview

**Full-precision bf16** fine-tune of [LiquidAI/LFM2.5-350M-Base](https://huggingface.co/LiquidAI/LFM2.5-350M-Base) for on-device speech-to-text transcript cleanup. This is the **training artifact** — for on-device deployment, use the [5-bit MLX variant](https://huggingface.co/juanquivilla/sotto-cleanup-lfm25-350m-mlx-5bit) instead.

This model powers on-device transcript cleanup in [SottoASR](https://sottoasr.app) — a local, privacy-first speech-to-text application for macOS. It removes filler words, corrects grammar, formats punctuation, and handles false starts and self-corrections — all locally, with zero cloud dependency.

## Key Specs

| Property | Value |
|----------|-------|
| **Size** | **676 MB** |
| **ROUGE-L** | **{{ROUGE_L}}** |
| **Exact Match** | **{{EXACT_MATCH}}** |
| **Filler-Free** | **{{FILLER_FREE}}** |
| **Latency** | **{{LATENCY}}** average per transcript (RTX 4090) |
| **Architecture** | Hybrid: 10 conv + 6 GQA attention (354M params) |
| **Precision** | bf16 |
| **Context** | 32,768 tokens (trained with 4,096 packed) |

## Benchmark Results

Evaluated on 135-sample benchmark covering 12 transcript cleanup categories:

| Category | N | ROUGE-L | Exact Match |
|----------|---|---------|-------------|
{{CATEGORY_TABLE}}

### vs Prompted Qwen 2B Baseline

| Metric | This model (350M) | Prompted Qwen 2B | Improvement |
|--------|-------------------|-------------------|-------------|
| ROUGE-L | **{{ROUGE_L}}** | 0.891 | **+{{DELTA_ROUGE}}** |
| Exact Match | **{{EXACT_MATCH}}** | 37% | **+{{DELTA_EXACT}}** |
| Inference | **{{LATENCY}}** | 1.0s | **{{SPEEDUP}}x faster** |
| Parameters | 354M | 2B | **5.6x smaller** |

## What It Does

Takes raw, unpunctuated ASR output and produces clean, readable text:

| Input (raw ASR) | Output (cleaned) |
|-----------------|------------------|
| so uh basically we need to fix the deployment pipeline | We need to fix the deployment pipeline. |
| the deadline is friday no monday we have until monday | The deadline is Monday. |
| what we what i wanted to say is the tests pass | What I wanted to say is the tests pass. |
| uh yes | Yes. |

## Usage

### Prompt Format

```
### Input:
{raw transcript}

### Output:
{model generates cleaned text}
```

### Python Example

```python
from transformers import AutoModelForCausalLM, AutoTokenizer
import torch

model = AutoModelForCausalLM.from_pretrained(
    "juanquivilla/sotto-cleanup-lfm25-350m",
    dtype=torch.bfloat16, trust_remote_code=True,
)
tokenizer = AutoTokenizer.from_pretrained("juanquivilla/sotto-cleanup-lfm25-350m")

text = "so uh basically the thing is we need to uh fix the deployment pipeline"
prompt = f"### Input:\n{text}\n\n### Output:\n"

inputs = tokenizer(prompt, return_tensors="pt").to(model.device)
with torch.no_grad():
    out = model.generate(**inputs, max_new_tokens=256, do_sample=False)
output = tokenizer.decode(out[0][inputs["input_ids"].shape[1]:], skip_special_tokens=True)
# Strip at ### marker if model continues generating
if "###" in output:
    output = output[:output.index("###")]
print(output.strip())
# → "We need to fix the deployment pipeline."
```

## Training Details

| Parameter | Value |
|-----------|-------|
| Method | Full fine-tune (all 354M params, no LoRA) |
| Dataset | {{DATASET_SIZE}} samples ([sotto-transcript-cleanup](https://huggingface.co/datasets/juanquivilla/sotto-transcript-cleanup)) |
| Learning rate | {{LEARNING_RATE}} (cosine schedule) |
| Epochs | {{EPOCHS}} |
| Batch size | 1 × 8 gradient accumulation |
| Optimizer | AdamW (full precision) |
| Precision | bf16 + tf32 |
| Hardware | 1× RTX 4090, ~25 min |

### Training Data Categories

The dataset covers diverse transcript cleanup scenarios:
- **Filler removal** — uh, um, like, you know, basically
- **Crutch phrase stripping** — "okay so the thing is basically..."
- **Self-correction** — "X, no wait, Y" → Y
- **False starts** — "What we— what I meant was..."
- **Grammar & punctuation** — capitalization, periods, commas
- **Dictation commands** — "new paragraph", "period"
- **Short inputs** — heavy filler, minimal content
- **Long-form transcripts** — 500+ word dictation

## Training Progression

| Version | ROUGE-L | Key Innovation |
|---------|---------|----------------|
| v1: LoRA SFT 15K | 0.771 | Baseline |
| v3: LoRA SFT 100K | 0.863 | Data scale breakthrough |
| v4: + GRPO | 0.891 | Matched prompted 2B |
| v5: Full FT | 0.907 | LoRA was the bottleneck |
| v7: LR 2e-5 | 0.943 | Learning rate breakthrough |
| v11: + Targeted data | 0.950 | Pattern-specific training |
| **v15: LR 2.5e-5** | **{{ROUGE_L}}** | **Current production model** |

## All Variants

| Variant | Size | ROUGE-L | Use Case |
|---------|------|---------|----------|
| **[Full precision (this)](https://huggingface.co/juanquivilla/sotto-cleanup-lfm25-350m)** | 676 MB | {{ROUGE_L}} | Training, GPU inference |
| **[MLX 5-bit](https://huggingface.co/juanquivilla/sotto-cleanup-lfm25-350m-mlx-5bit)** | 237 MB | ~{{ROUGE_L_5BIT}} | **Recommended for Apple Silicon** |
| [MLX 4-bit](https://huggingface.co/juanquivilla/sotto-cleanup-lfm25-350m-mlx-4bit) | 195 MB | ~{{ROUGE_L_4BIT}} | Smallest, slight quality trade-off |

## Limitations

- Optimized for **English** conversational/meeting-style speech
- Domain-specific jargon (medical, legal) may not be corrected without additional fine-tuning
- Long dictation (>500 words) has lowest exact match rate
- Not designed for formal written text — trained on spoken language patterns

## License

MIT

## Links

- **Application:** [sottoasr.app](https://sottoasr.app)
- **Source:** [github.com/juanqui/sottoasr](https://github.com/juanqui/sottoasr)
- **Dataset:** [juanquivilla/sotto-transcript-cleanup](https://huggingface.co/datasets/juanquivilla/sotto-transcript-cleanup)
