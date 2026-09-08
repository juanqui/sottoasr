# Small cleanup model study

- **Version:** 1.0
- **Date:** 2026-09-08
- **Status:** Implemented

This folder preserves synthetic fixtures, pinned public model identities, measured per-case results, and the exact protocol sources used in the September model comparison. The decision and primary-source research live in [the research report](../../../docs/research/2026-09-08-small-cleanup-models.md). No model weights, personal transcripts, credentials, package environments, or generated logs are included.

No tested model qualified for automatic cleanup. The product keeps the small stock model only for experimental suggestions in History, requiring explicit review and Copy; ordinary dictation uses ASR plus saved dictionary corrections. The initial 24/25 result was superseded by the broader preservation failures recorded here.

## Evidence

`holdout-v2.json` contains 96 independently reviewed cases:40 requiring edits,56 requiring preservation. Only71 have eligible Rust candidates; report both all-case and eligible results. `development-bench-v2.json` contains eight separate development cases. `cleanup-fresh-72.json` contains the later independent validation set:32 edit/40 preserve,56 eligible. `cleanup-fresh-80.json` was independently authored and withheld until the semantic-role policy was frozen:40 edit/40 preserve,61 eligible. Fixture fingerprints are in `manifest.json`.

`results/*-fixed-v2.json` records all twelve models under the fixed production sparse protocol. `results/*-binary-fresh72.json` records the two frozen first-token classifiers under an external10-second whole-request deadline. `results/qwen4000-role-fresh80.json` records the frozen semantic-role policy, which also failed preservation and usefulness gates. Per-case results omit duplicated raw/gold/candidate fields; join by `id` to the corresponding fixture and enumerate candidates with the included Rust adapter. Other `*-dev.json` files are development evidence, not held-out scores. Gold labels were frozen before relevant model outputs were inspected. Failed validation data must not be reused as fresh validation after prompt changes.

Exact output, harmful deletion, and useful editing are separate measures. A harmful deletion occurs when the gold output is no longer a character subsequence of the result. Useful editing requires a changed result with no harmful deletion on a case requiring edits. This checks the authored synthetic gold; it does not prove universal semantic safety. Rust bypasses and deadline/format fallback preserve the raw text and must not be counted as successful model judgments.

## Reproduction

Requires an Apple Silicon Mac and Python3.11 or newer. The recorded environment used Python3.14.5, MLX0.32.2, mlx-lm0.31.3, transformers5.3.0, huggingface-hub1.7.2 and psutil7.2.2. Create an isolated environment outside the repository and install `requirements.txt`. Obtain the public artifact identified by `models.json` at its exact revision into an explicit experiment directory. All inference uses a supplied local path and offline mode; downloads are a separate preparation step.

From the repository root, build the tiny adapter without building the application:

```bash
cargo build --locked --manifest-path benchmarks/llm/model-study-2026-09-08/guard-adapter/Cargo.toml
```

With the isolated Python environment active, a baseline reproduction is:

```bash
HF_HUB_OFFLINE=1 HF_HUB_DISABLE_IMPLICIT_TOKEN=1 python benchmarks/llm/model-study-2026-09-08/bench_v2.py \
  --model-path /absolute/path/to/local/weights --label liquid350 \
  --output /tmp/liquid350-reproduction.json
```

A frozen binary validation reproduction is:

```bash
HF_HUB_OFFLINE=1 HF_HUB_DISABLE_IMPLICIT_TOKEN=1 python benchmarks/llm/model-study-2026-09-08/validate_binary.py \
  --dataset benchmarks/llm/model-study-2026-09-08/cleanup-fresh-72.json \
  --model /absolute/path/to/local/MiniCPM5-2B-MLX \
  --config benchmarks/llm/model-study-2026-09-08/binary-classifier-v1.json \
  --output /tmp/minicpm-binary-reproduction.json
```

Use the Qwen configuration and corresponding local weights for the4B control. The worker process uses the current Python executable. The reference sidecar and Rust guard are snapshots, so subsequent product changes do not silently change this benchmark. A read-only `source_context` accessor and its test were added after the binary runs; the original and archived guard hashes are both recorded. Candidate enumeration and application are unchanged.

For the semantic-role validation, use `validate_role.py`, `cleanup-fresh-80.json`, `role-qwen4-v1.json`, and the pinned Qwen4B weights. `role_dev.py` preserves the exact development comparison. `validate_reasoning.py` with either native-thinking configuration records the failed512-token reasoning controls, including actual timeout fallback; those are development experiments only. The included reference worker and classifier scripts use the same pinned policy with paths made portable. Their original source hashes are retained in the manifest.

The final Qwen0.8B and official Granite4.2-3B controls used that unchanged semantic-role policy on the eight development cases. Both failed development: Qwen made no useful edits; Granite made three harmful deletions. Neither was sent to fresh validation. The archived adapter was built with `--locked`, and both final controls ran through this portable `role_dev.py`:

```bash
HF_HUB_OFFLINE=1 HF_HUB_DISABLE_IMPLICIT_TOKEN=1 python benchmarks/llm/model-study-2026-09-08/role_dev.py \
  --model /absolute/path/to/local/weights --label granite4200 \
  --output /tmp/granite-role-development.json
```

The encoder diagnostic uses a separate environment with `requirements-encoder.txt`. Supply the pinned ONNX artifact from `encoder-catalog.json` to `encoder_dev.py --model /absolute/path/to/local/encoder --output /tmp/encoder-development.json`. It runs the CPU provider, not ANE, and its model license/provenance remains unresolved. `development-v2.json` preserves the original raw/gold authoring file; runnable development scripts use the equivalent cases with IDs in `development-bench-v2.json`.

## Measurement limits

Run one inference process at a time, with app/browser/build benchmarks paused. Cold load means a fresh process and normal OS file caches, not a purged disk. CPU time, RSS and allocated Metal bytes are distinct metrics; they are not watts and must not be added together. The fixed-protocol comparison inherited a2GiB MLX allocation guideline, which is not a hard RSS cap and may penalize a larger model. The binary validator measures an actual external10-second request deadline, including kill/reap fallback; the earlier fixed-protocol harness used a30-second development alarm. It is not evidence of the new production timeout behavior.

The later [bare hesitation follow-up](bare-hesitation-followup/README.md) preserves the bounded same-model prompt iteration, newly eligible unpunctuated tokens, independent16 validation and actual production diagnostic. It improves known-regression suggestion coverage while still failing automatic preservation; the reported sentence is only partially cleaned.
