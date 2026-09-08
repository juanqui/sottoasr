# MLX cleanup prefix caching and power measurement

- **Version:** 1.0
- **Date:** 2026-09-08
- **Status:** In Review

## Contents

1. Findings
2. Supported static-prefix cache flow
3. Correctness and performance experiment
4. Power measurement availability
5. Evidence and limits

## 1. Findings

MLX-LM 0.31.3 supports a reusable, model-specific prompt cache. A suitable Sotto
design keeps only the fixed system prompt and synthetic few-shot examples in a
baseline cache, deep-copies that baseline for each request, and discards the
request copy afterward. It must never insert a completed dictation's cache back
into the shared baseline. The generation API mutates the supplied cache in place.
The official cache implementation itself uses `copy.deepcopy` when returning
reusable cached state. [Cache implementation](https://github.com/ml-explore/mlx-lm/blob/v0.31.3/mlx_lm/models/cache.py#L15),
[generation API](https://github.com/ml-explore/mlx-lm/blob/v0.31.3/mlx_lm/generate.py#L307).

Do not use “generate, then trim back” as the isolation mechanism. Qwen3.5 and LFM2
combine attention caches with `ArraysCache` recurrent/convolution state; that
state does not generally support trimming to an earlier prefix. Use the model's
cache factory and clone the pristine static cache instead.
[Qwen3.5 cache factory](https://github.com/ml-explore/mlx-lm/blob/v0.31.3/mlx_lm/models/qwen3_5.py#L304),
[LFM2 cache factory](https://github.com/ml-explore/mlx-lm/blob/v0.31.3/mlx_lm/models/lfm2.py#L312).

No caching speedup or energy saving was measured in this preparation task.
The installed `powermetrics` exposes CPU, GPU and ANE power samplers, but direct
noninteractive sampling failed because it requires superuser execution. No
password prompt, privilege change, model load or inference was performed.

## 2. Supported static-prefix cache flow

Verified installed source:
`/tmp/experiments/sotto-cleanup-overnight/venv/lib/python3.14/site-packages/mlx_lm/`.
Package metadata identifies version **0.31.3**.

| API/source | Lifecycle detail |
|---|---|
| `models/cache.py:15`, `make_prompt_cache(model)` | Calls `model.make_cache()` when available; otherwise creates a `KVCache` per layer. Preserve model-specific cache types. |
| `cache_prompt.py:126`, `generate_step(..., max_tokens=0, prompt_cache=cache)` | This is the upstream cache-building flow. It evaluates the complete supplied token prefix without appending a generated token. |
| `generate.py:429` through `generate.py:465` | Prefill processes all but the last prompt token in chunks; `_step(prompt)` then consumes the remaining last token. With `max_tokens=0`, the loop does not call `_step(y)`. The final cached sequence therefore includes **all prefix tokens**, not all but one. |
| `generate.py:657`, `stream_generate(..., prompt=suffix_ids, prompt_cache=request_cache)` | Accepts already-tokenized IDs and forwards the cache to `generate_step`. It appends the supplied suffix to the existing prefix state. |
| `models/cache.py:1674`, `LRUPromptCache.fetch_nearest_cache` | Uses `copy.deepcopy` before returning cached state for reuse. This is the supported cloning pattern to test for the selected model/cache implementation. |

The upstream prefill helper is `generate_step`, **not** `stream_generate` with a
zero output budget. A zero-token `generate_step` iteration still computes the
next-token distribution, but does not add that prediction to the cache.
Its completion callback and attention-layer cache offsets can verify that the
number of consumed tokens equals the intended prefix length.
[Upstream cache-building utility](https://github.com/ml-explore/mlx-lm/blob/v0.31.3/mlx_lm/cache_prompt.py#L126).

Proposed sequence, not executed here:

```python
baseline = make_prompt_cache(model)
for _ in generate_step(mx.array(prefix_ids), model,
                       max_tokens=0, prompt_cache=baseline):
    pass
mx.eval([layer.state for layer in baseline])

# Each request: tokenize the complete native prompt once.
assert full_ids[:len(prefix_ids)] == prefix_ids
assert len(full_ids) > len(prefix_ids)
request_cache = copy.deepcopy(baseline)
responses = stream_generate(model, tokenizer,
    prompt=full_ids[len(prefix_ids):], prompt_cache=request_cache,
    max_tokens=qualified_output_budget, sampler=qualified_sampler)
# Consume the completed response, then discard request_cache in finally.
```

Build the candidate prefix exclusively from static application-owned prompt
content. A chat template can change delimiters depending on message position,
and BPE tokens can merge at a text boundary. Never assume that separately
tokenizing a prefix and suffix equals tokenizing their concatenation. Render and
tokenize the full native request, then compare exact prefix token IDs before
reuse. A mismatch takes the uncached path; any shorter baseline must be rebuilt
from static-only tokens rather than recovered from an earlier dictation.

Cache identity includes model revision/precision, tokenizer and chat template,
static prompt/examples, runtime/adapter version and cache configuration. Rebuild
on any identity change. Retain at least one uncached suffix token, since generation
rejects an empty prompt. Keep both baseline and request state in memory only;
the upstream disk-cache example is not the intended product flow.
[Official prompt-caching documentation](https://github.com/ml-explore/mlx-lm#long-prompts-and-generations).

## 3. Correctness and performance experiment

After the hardware window is available, compare cached and uncached execution
using the same qualified weights, prompt, sampler, output budget and synthetic
development inputs. Include leading whitespace, punctuation, accented characters,
emoji and inputs beginning with token-boundary-sensitive text. These checks are
about cache correctness, not an opportunity to tune against held-out gold.

Require equal generated token sequences, final text, completion reasons and
validated output. Repeated execution of A, then a distinctive unrelated B, then A
must match independent uncached A results. Check that baseline attention offsets,
cache types and evaluated static state remain unchanged after each request,
including cancellation or an exception. Verify the selected custom adapter's
cache clone behavior; upstream cache support is not proof for every adapter.

Prefix splitting can change numerical execution shape, so semantic plausibility
alone is insufficient evidence of equivalent output. If equality fails, identify
token-boundary, cache-offset, mutation or numerical causes before enabling the
optimization. Keep the accepted uncached path available.

Measure one-time baseline construction separately from request latency, including
clone cost, prefill time, full generation, CPU time, RSS and allocated Metal bytes.
Record full prompt tokens, cached prefix tokens and remaining tokens separately:
`stream_generate` reports `prompt_tokens` for the supplied suffix when caching,
so that field alone no longer describes the complete request. Use alternating
cached/uncached blocks on the same Mac with other inference/build work paused.

## 4. Power measurement availability

The installed `/usr/bin/powermetrics --help` lists `cpu_power`, `gpu_power` and
`ane_power`. It describes subsystem values as estimates appropriate for
within-device optimization, not cross-device power comparisons. Its per-process
“energy impact” number is not a direct joule measurement.

This exact noninteractive availability probe was attempted:

```sh
/usr/bin/powermetrics --samplers cpu_power,gpu_power,ane_power \
  --sample-rate 1000 --sample-count 1
```

It exited with code 1 and `powermetrics must be invoked as the superuser`.
The current unprivileged execution context therefore cannot collect those power
samples. A later root-agent probe used `sudo -n` with the same sampling arguments;
it exited immediately with `sudo: a password is required`. No credentials were
requested or supplied and no elevated sampler ran. No credential refresh or
system setting was changed. CPU time and latency remain available as
performance measurements; they must not be relabeled as watts or joules.

If an already authorized measurement context becomes available, first inspect a
bounded sample's actual fields and units. Then collect matched 30–60-second
idle and serial-workload blocks at a fixed sampling interval, with the same
ASR/model residency and no overlapping downloads, builds or user activity.
Use only the needed power/thermal samplers, not unrelated process or network
listings. Preserve raw samples and exact block timestamps.

Integrate the reported, non-overlapping subsystem power estimates over time to
estimate joules per completed request, and report idle-baseline treatment and
run-to-run variation. Do not double-count a combined total alongside its component
rails. These are whole-subsystem estimates rather than isolated application
measurements; no energy result is available from this task.

## 5. Evidence and limits

Installed-source SHA256 values:

| Source | SHA256 |
|---|---|
| `mlx_lm/models/cache.py` | `819ed95dcbf755652363cfdb15a639890447abb534a06dcefd52c7fff5055750` |
| `mlx_lm/generate.py` | `270778ad53eaca55a8533d82e6752660fe5d2605c4aa0879b48a50a91f69345f` |
| `mlx_lm/cache_prompt.py` | `b2f561f47e177367499be07aa92214a70d30220a84a126a5460ab51ebab25cd8` |

The installed source and current official documentation were read directly.
No model/validator result files or held-out labels were used for this research.
No inference, cache experiment, benchmark-harness edit, product edit or privilege
change was performed. The cloning, token-boundary and equality experiment above
remains required before claiming a safe optimization or measured improvement.
