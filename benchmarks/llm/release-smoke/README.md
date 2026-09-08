# Bundled MiniCPM local-release smoke

- **Version:** 1.0
- **Date:** 2026-09-08
- **Status:** In Review

This verifies the actual `.app` Python resource and prepared app runtime against 15 previously revealed development cases. It is a smoke test for the user-requested local test release. The earlier [D3 qualification failed](../model-study-2026-09-08/direct-cleanup-diagnostic/qualification/RESULTS.md); these cases do not establish a new independent quality qualification or a semantic preservation guarantee.

The script never downloads models, changes settings/history, touches the clipboard, or sends paste events. Run it only after explicit model preparation has created the verified pinned snapshot and after other inference/build processes have exited. It terminates and reaps its own sidecar.

Build the tiny adapter once. It imports the **current production Rust validator**, plus the same `whatlang` reliability check used by `run_cleanup`:

```sh
cargo build --manifest-path benchmarks/llm/release-smoke/adapter/Cargo.toml --release 2>&1 | tee /tmp/sotto-cleanup-smoke-adapter-build.txt
```

Then pass the built bundle and prepared application runtime explicitly:

```sh
python3 benchmarks/llm/release-smoke/run_bundled_sidecar.py \
  --bundle src-tauri/target/release/bundle/macos/SottoASR.app \
  --python "$HOME/Library/Application Support/com.sottoasr.app/llm-venv/bin/python3" \
  --adapter benchmarks/llm/release-smoke/adapter/target/release/sotto-cleanup-smoke-adapter \
  --out /tmp/sotto-bundled-cleanup-smoke.json \
  --execute 2>&1 | tee /tmp/sotto-bundled-cleanup-smoke.txt
```

Without `--execute`, only import-safe prompt/model identity and generation-completion checks run. These verify that length termination, missing stop reasons, malformed Unicode, and oversized proposals are rejected. Empty completed text is valid; pipeline tests separately verify that filler-only empty delivery retains the raw history and does not overwrite the clipboard.

The executed smoke checks the pinned revision, frozen D7 prompt object, runtime package versions, status/load response identity, bounded actual JSON requests, complete generation, production validation, and reliable non-English abstention. A fresh child must report `loaded:false,warmed:false`; its first load must return `warmed:true,did_warm:true`, and subsequent status must confirm readiness. A repeated load must return `did_warm:false` within one second, demonstrating reuse without another synthetic warmup. `cold_load_and_warm_latency_s`, `repeat_load_latency_s`, and `first_user_request_latency_s` keep preparation separate from dictation timing. The combined cold load/warm measurement does not pretend to isolate weight loading from compilation.

The smoke stores raw responses and delivered text separately. The reported sentence must match its natural requested cleanup exactly. Other cases pass when their delivered words match the target or the complete source is retained; missed cleanup is counted separately. Exact embedded protected payload checks accompany lexical comparison, which alone cannot detect punctuation corruption. The suite includes names, literal field words, quoted/code text, a negation, numbers, self-correction, foreign text, and a long ending.

This does **not** exercise the native hotkey, microphone, focus restoration, live clipboard, or actual paste. Root's integration tests and local app check cover those layers. No release qualification2 file is used.

`export_v6_parity.py` separately regenerates `src-tauri/tests/fixtures/cleanup-validation-v6.json` from immutable v6 authored tests and 21 previously revealed raw-model profiles. The fixture records oracle outputs, with incomplete/non-text requests outside the Rust API excluded; no model inference or policy tuning is involved.
