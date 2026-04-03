---
license: mit
language:
- en
base_model: juanquivilla/sotto-cleanup-lfm25-350m
tags:
- speech-to-text
- transcript-cleanup
- mlx
- quantized
- apple-silicon
- LFM
- LiquidAI
pipeline_tag: text-generation
datasets:
- juanquivilla/sotto-transcript-cleanup
---

# SottoASR Transcript Cleanup — LFM2.5-350M MLX 4-bit

[sottoasr.app](https://sottoasr.app) · [Full precision (bf16)](https://huggingface.co/juanquivilla/sotto-cleanup-lfm25-350m) · [MLX 5-bit (recommended)](https://huggingface.co/juanquivilla/sotto-cleanup-lfm25-350m-mlx-5bit) · [Training Dataset](https://huggingface.co/datasets/juanquivilla/sotto-transcript-cleanup)

## Overview

**4-bit MLX-quantized** version of the [SottoASR transcript cleanup model](https://huggingface.co/juanquivilla/sotto-cleanup-lfm25-350m), optimized for inference on **Apple Silicon** (M1/M2/M3/M4). This is the **smallest variant** — use the [5-bit version](https://huggingface.co/juanquivilla/sotto-cleanup-lfm25-350m-mlx-5bit) for better quality, or this one if disk space is a priority.

This model powers on-device transcript cleanup in [SottoASR](https://sottoasr.app) — a local, privacy-first speech-to-text application for macOS.

## Key Specs

| Property | Value |
|----------|-------|
| **Size** | **{{SIZE_MB}} MB** (3.5x smaller than bf16) |
| **ROUGE-L** | **~{{ROUGE_L}}** |
| **Quantization** | 4-bit affine, group_size=64 |
| **Framework** | MLX (Apple Silicon optimized) |

## What It Does

Takes raw, unpunctuated ASR output and produces clean, readable text:

| Input (raw ASR) | Output (cleaned) |
|-----------------|------------------|
| so uh basically we need to fix the deployment pipeline | We need to fix the deployment pipeline. |
| the deadline is friday no monday we have until monday | The deadline is Monday. |
| okay uh so basically yeah it's done | It's done. |

## Usage

```python
from mlx_lm import load, generate
from mlx_lm.sample_utils import make_sampler

model, tokenizer = load("juanquivilla/sotto-cleanup-lfm25-350m-mlx-4bit")
sampler = make_sampler(temp=0.0)

text = "uh yeah so the server crashed again"
prompt = f"### Input:\n{text}\n\n### Output:\n"

output = generate(model, tokenizer, prompt=prompt, max_tokens=256, sampler=sampler, verbose=False)
if "###" in output:
    output = output[:output.index("###")]
print(output.strip())
# → "The server crashed again."
```

## Quantization Recipe

```bash
mlx_lm.convert \
  --hf-path juanquivilla/sotto-cleanup-lfm25-350m \
  --mlx-path sotto-cleanup-lfm25-350m-mlx-4bit \
  -q --q-bits 4 --q-group-size 64 \
  --trust-remote-code
```

## All Variants

| Variant | Size | ROUGE-L | Use Case |
|---------|------|---------|----------|
| [Full precision (bf16)](https://huggingface.co/juanquivilla/sotto-cleanup-lfm25-350m) | 676 MB | {{PARENT_ROUGE_L}} | Training, GPU inference |
| [MLX 5-bit (recommended)](https://huggingface.co/juanquivilla/sotto-cleanup-lfm25-350m-mlx-5bit) | {{SIZE_5BIT_MB}} MB | ~{{ROUGE_L_5BIT}} | Recommended for Apple Silicon |
| **[MLX 4-bit (this)](https://huggingface.co/juanquivilla/sotto-cleanup-lfm25-350m-mlx-4bit)** | **{{SIZE_MB}} MB** | **~{{ROUGE_L}}** | **Smallest variant** |

## Links

- **Application:** [sottoasr.app](https://sottoasr.app)
- **Source:** [github.com/juanqui/sottoasr](https://github.com/juanqui/sottoasr)
- **Parent model:** [sotto-cleanup-lfm25-350m](https://huggingface.co/juanquivilla/sotto-cleanup-lfm25-350m)
- **Dataset:** [juanquivilla/sotto-transcript-cleanup](https://huggingface.co/datasets/juanquivilla/sotto-transcript-cleanup)

## License

MIT
