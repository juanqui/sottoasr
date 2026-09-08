# Static-prefix cache experiment

- **Version:** 1.0
- **Date:** 2026-09-08
- **Status:** In Review

Prepared source only. No cache inference or performance measurement has run.
The D3/v5 qualification failed, so this optimization cannot establish model
quality or justify automatic cleanup promotion. The [parent study](../README.md)
and [API research](../../../../../docs/research/2026-09-08-mlx-cleanup-performance.md)
describe the separate quality and performance questions.

## 1. Protocol

The driver uses the fixed D3 inline-system examples and official MiniCPM5-2B
artifact, runtime, greedy sampler, seed 42, token allowance and 10-second request
deadline. Full native prompt tokens use the qualification runner's explicit BOS
handling and context admission. Static cache tokens come only from the shipped
system/examples/template prefix before a synthetic marker; the final separately
encoded token is omitted to avoid assuming a BPE boundary.

`make_prompt_cache` plus `generate_step(max_tokens=0)` prefills the baseline.
Every KV offset must equal the static prefix length. Each cached request gets
`copy.deepcopy(baseline)` and its full token sequence must start with the cached
tokens; a mismatch falls back uncached and cannot count toward cache speedup.
Full allocated baseline KV arrays, including unused capacity, and offsets are
fingerprinted before/after requests outside timing. No request cache is reused
or written to disk.

Eleven synthetic cases cover A/B/A isolation, boundary punctuation/whitespace,
Unicode and approximately 40/150 words. Two rounds alternate cached/uncached pair
order. Success requires exact generated token IDs including EOS, exact text and
completed finish status, unchanged baseline state, and equality after injected
post-first-token cancellation/timeout recovery. These are equivalence fixtures,
not new cleanup accuracy gold.

## 2. Measurement and execution

Cache construction is reported separately. Request latency includes rendering,
encoding, cache cloning, generation and request-cache release. MLX prefill/decode
durations, first-token latency, CPU time, RSS and Metal memory are separate fields.
Process peak RSS is cumulative; Metal allocations overlap physical unified
memory and must not be added to RSS. Full-state verification is excluded from
request timing but may affect cache/thermal conditions; run in an exclusive
hardware window. The driver measures no energy or watts.

Only pure helper checks have run:

```bash
python3 compare_prefix_cache.py --self-test
```

After an explicit experiment/hardware release, using the qualified isolated
runtime and a new output path:

```bash
/absolute/path/to/venv/bin/python compare_prefix_cache.py --execute \
  --model /absolute/path/to/verified/minicpm2000 \
  --prompt ../development/prompt-inline.json --output /tmp/cache-result.json
```

Small artifact hashes and runtime pins are always checked. `--verify-weights`
additionally rereads the pinned 1.4 GB weight; omit only when the exact existing
artifact has already been verified for the qualification run. Use an external
process deadline: Python signals can be deferred inside native work. The script
never downloads models, changes caches or reads application transcripts.

## 3. Provenance

[manifest.json](manifest.json) records source, prompt and native qualification
runner hashes. This directory contains the prepared source and protocol only;
there are no generated caches, model weights or claimed timing results.
