# In-Process Cleanup Spike (G-08)

- **Version:** 1.0
- **Date:** 2026-11-19
- **Status:** In Review

## Table of Contents

1. [Question](#1-question)
2. [Method](#2-method)
3. [Results](#3-results)
4. [Limits](#4-limits)
5. [Recommendation](#5-recommendation)

## 1. Question

Transcript cleanup runs today in a Python sidecar (`src-tauri/sidecar/llm_cleanup.py`)
that the app starts as a child process and talks to over JSON lines. Finding
G-08 in the [November 2026 review](../audit/2026-11-19-application-review.md)
asks whether cleanup can run inside the app through Swift, using
[`mlx-swift-lm`](https://github.com/ml-explore/mlx-swift-lm), which registers
the checkpoint's `llama` model type.

The spike answers four questions with measurements:

1. Does `mlx-swift-lm` load the pinned MiniCPM5 snapshot and generate? Which
   tokenizer loader does it need, and does it render `chat_template.jinja` to
   the same prompt token IDs as the Python path?
2. Are the outputs byte-identical to the Python path with the sidecar's
   generation settings?
3. How do load time, one-window time, 16-window time and peak memory compare?
   Can the Swift library generate a batch?
4. Can a Cargo build script build it with `swift build` as a static library,
   what does it add to the bundle, and what would it replace?

**Short answer:** the Swift path works and matches Python closely, but it is
not ready to replace the sidecar today. Section 5 lists what blocks it.

## 2. Method

### 2.1 Machine and software

All numbers come from one machine: Apple M2 Pro, 32 GB, macOS 26.7.3 (25G342),
Xcode 27.0 (27A266a), Swift 6.4 (swiftlang-6.4.0.34.1). During every run,
other agents were building and testing on the same machine (load average 10
to 21, read with `uptime` before and after each run). The production SottoASR
app and its idle cleanup sidecar were also running. Timings are therefore
indicative, not lab-grade.

| Path | Versions |
| --- | --- |
| Python | Python 3.11.13 venv installed with `pip install --require-hashes --only-binary :all: --no-deps -r src-tauri/sidecar/requirements.lock`: `mlx` 0.32.2, `mlx-metal` 0.32.2, `mlx-lm` 0.31.3, `transformers` 5.3.0, `tokenizers` 0.22.2 |
| Swift | `mlx-swift-lm` 3.33.0 (`ca661dc6`, traits off), `mlx-swift` 0.33.0 (`7ede4fd6`, embeds MLX core 0.32.4), `swift-transformers` 1.4.0 (`402a8a11`), `swift-jinja` 3.2.1 |
| Model | `openbmb/MiniCPM5-2B-MLX` at revision `32f8dd5d`, read in place from the Hugging Face cache (read only) |

### 2.2 Inputs

Sixteen synthetic windows: the 15 raw inputs in
`benchmarks/llm/release-smoke/cases.json` plus one filler sentence ("Please um
keep the the spare filler sentence so that this batch uh has sixteen
windows."). No private text was used. Both prompt modes were run: `retype`
(the default, `CleanupMode::Retype` in `models.rs`) and `replace`.

### 2.3 Harnesses

All scratch files are under `/tmp/experiments/InProcessSpike/`.

- **Python** (`py_bench.py`) imports an unmodified copy of `llm_cleanup.py`
  (as committed, last changed in `246bfeb2`; sha256 `d79bec1d…`) and calls its
  own `load_model`, `build_prompt`, `prompt_ids_for`, `warm_model` and
  `run_batch_generation`. This is the production batch path without the
  JSON-lines transport. The 10 s batch deadline was raised so a slow batch
  would be measured to the end; no batch took longer than 6.8 s, so it had no
  effect. The script also runs a "single" path: mlx-lm `generate_step` on one
  prompt with no batch and no prefix cache.
- **Swift** (`swift/Sources/Spike/main.swift`) loads the snapshot with
  `loadModel(from:using:)`, renders the same message lists through
  `swift-transformers`, and generates greedily with `TokenIterator`. It runs
  three variants: library default (whole prompt in one forward pass), "split"
  (all but the last token, then one token, which is the mlx-lm call shape),
  and "head cache" (the sidecar's shared 407-token or 520-token prompt head is
  built once and copied per window, as the sidecar does).
- Generation settings on both sides: temperature 0 (greedy), the sidecar's
  budget `min(8192, max(128, 2 × tokens(text) + 32))`, stop on token 1 or
  130073, `Memory.memoryLimit` 4 GiB and `cacheLimit` 128 MiB.
- `compare.py` prints every table below from the four result files.

Each process ran under `/usr/bin/time -l`. Both harnesses were run twice
(runs 1 and 2, back to back). Every generated token was identical between
the two runs on every path.

## 3. Results

### 3.1 Loading and prompt rendering

`mlx-swift-lm` loads the snapshot and generates. It picks `LlamaModel` from
`config.json` (`"llama"` is registered in `LLMModelFactory.swift:54`).

`mlx-swift-lm` 3.x ships no tokenizer. It needs a `TokenizerLoader`. The only
integration it documents is `swift-transformers`: either the
`#huggingFaceTokenizerLoader()` macro in `MLXHuggingFace` (which pulls in the
swift-syntax macro plugin), or a hand-written bridge to
`Tokenizers.AutoTokenizer.from(modelFolder:)`. The spike used a 40-line bridge
copied from the macro expansion. `swift-transformers` reads
`chat_template.jinja` from the model folder (`Hub.swift:277`), maps the
checkpoint's `tokenizer_class: "TokenizersBackend"` to its BPE tokenizer
(`Tokenizer.swift:178`), renders with `swift-jinja`, and encodes without added
special tokens (`Tokenizer.swift:839-840`). The template itself emits `<s>`,
so this matches the sidecar's `add_special_tokens` rule.

| Check (16 windows × 2 modes) | Result |
| --- | --- |
| Swift prompt token IDs equal Python `prompt_ids_for` | 32 / 32 |
| Swift encoding of the Python-rendered string equals Python IDs | 32 / 32 |
| Swift decode of its IDs equals the Python-rendered string | 32 / 32 |
| Generation budget equal | 32 / 32 |
| Prompt length | 458–586 tokens (replace), 571–699 (retype) |
| Head cache length after prefill | 407 (replace), 520 (retype), same as the sidecar's heads |

### 3.2 Output parity

"Python single" is the same algorithm as Swift (one prompt, greedy). "Python
batch" is what production runs today. Counts are byte-identical decoded texts
out of 16; runs 1 and 2 gave the same counts.

| Comparison | retype | replace |
| --- | --- | --- |
| Python batch vs Python single | 16 | 14 |
| Swift default vs Python single | 16 | 15 |
| Swift default vs Python batch | 16 | 14 |
| Swift head cache vs Python batch | 16 | 14 |
| Swift split vs Python single | 16 | 14 |

The differences are all in `replace` mode:

- **Window 11** (the long `semantic_47` paragraph): Swift diverges from Python
  single at the second generated token, repeats `<D>|` and stops at the
  304-token budget. The sidecar would report that as `incomplete_generation`.
  The Python batch path also returned `failed / incomplete_generation` for this
  window. Python single stopped after 16 tokens with a malformed edit line.
- **Window 0** (`reported_sentence`): Swift equals Python single. Python batch
  produced a different edit list, so the batch itself changes this output.
- **Window 10**, split variant only: Swift wrote `…|||thirty` where Python
  wrote `…|||twenty, thirty`.

Reproducing the mlx-lm call shape ("split") did not improve parity. The
remaining differences come from numerics, not prompt rendering (both sides
receive identical token IDs). Two likely causes, not isolated here: MLX core
0.32.4 in Swift against 0.32.2 in the lock, and differences between the Swift
and Python Llama implementations.

### 3.3 Latency and memory

Latency is wall time inside each process, measured with `time.perf_counter`
(Python) and `ContinuousClock` (Swift), in seconds. Values are run 1 / run 2;
where three repeats were taken, the median is shown.

| Measure | Python sidecar code | Swift `mlx-swift-lm` |
| --- | --- | --- |
| Model load | `load_model` 5.16 / 2.07 (imports 1.46–1.70 plus `mlx_lm.load` 0.68–0.75, from `py_load_breakdown.py`) | 0.73 / 0.55 |
| Warm-up | `warm_model` (2 heads, 2 warm batches) 4.53 / 2.27 | first generation 3.91 / 1.08 |
| One window, retype | 0.34 / 0.33 (batch of 1, head cache) | 0.33 / 0.33 (head cache) |
| One window, replace | 0.59 / 0.59 | 0.66 / 0.68 |
| 16 windows, retype | **4.19 / 4.23** (one batch) | **5.32 / 5.39** (sequential, head cache); 17.3 / 16.3 (sequential, no cache) |
| 16 windows, replace | **6.55 / 6.56** (one batch) | **8.20 / 7.83** (sequential, head cache); 17.4 / 16.3 (no cache) |

The replace-mode batch time matches the production figure of about 6.6 s per
16-window batch. Python load in run 1 includes the first import in a fresh
venv. Neither side includes process start or IPC; the sidecar's spawn cost was
not measured.

Memory was read from `/usr/bin/time -l` ("peak memory footprint", which
includes Metal allocations) and from MLX's peak active memory. The Python
process ran 16-row batches; the Swift process ran one sequence at a time.

| Measure | Python | Swift |
| --- | --- | --- |
| Peak memory footprint | 3.09 GB / 3.08 GB | 2.60 GB / 2.60 GB |
| MLX peak active memory | 2.76 GB | 2.44 GB |
| Maximum resident set size | 1.16 GB / 1.80 GB | 0.95 GB / 1.55 GB |

Resident set size varied between runs with the page cache; footprint was
stable.

**Batching.** `mlx-swift-lm` 3.33.0 generates one prompt at a time.
`TokenIterator.next()` returns one `Int?` (`Evaluate.swift:1276`), each step
adds a batch axis of one (`Evaluate.swift:1265`), and the attention cache the
Llama model gets by default (`KVCacheSimple`) does not override the no-op
`prepare(lengths:)` (`KVCache.swift:308`), so ragged rows are not supported.
No batch generator type exists in the package (a search for
`BatchGenerator`, `BatchTokenIterator`, `batchGenerate` and `ContinuousBatch`
found nothing). Even with the shared-head cache, 16 sequential windows took
19–27 % longer than the sidecar's batch.

### 3.4 Build and integration

**Default `swift build` fails at the Metal shaders.** Swift 6.4's default
build system compiles the `.metal` sources of `mlx-swift` and stops after
11 s:

```text
error: error: cannot execute tool 'metal' due to missing Metal Toolchain; use: xcodebuild -downloadComponent MetalToolchain
```

`xcodebuild` itself cannot run on this machine without a first-launch step:

```text
A required plugin failed to load. Please ensure system content is up-to-date — try running 'xcodebuild -runFirstLaunch'.
```

Both fixes install system components outside the repository and were not
authorised, so a build with the Metal Toolchain was not tested.

**The deprecated native build system builds, without a Metal library.**
`swift build -c release --arch arm64 --build-system native` built everything
in 208 s from clean (dependency resolution took a further 62 s), with the
warning `'--build-system native' has been deprecated and will be removed in a
future release`. The build directory reached 1.8 GB. The result runs until the
first GPU call:

```text
MLX error: Failed to load the default metallib. library not found library not found library not found library not found library not found  at …/mlx-c/mlx/c/memory.cpp:89
```

The spike then used `mlx.metallib` from the PyPI wheel
`mlx_metal-0.32.4-py3-none-macosx_26_0_arm64.whl` (wheel sha256 `514df92d…`,
metallib sha256 `c6d7a3fa…`), the MLX core version that `mlx-swift` 0.33.0
embeds. Every Swift result above used that file.

**Static library linked into a non-Swift binary.** A `.library(type:
.static)` product with one `@_cdecl` function built to a 381.6 MB
`libCleanupBridge.a`. A C program linked against it with `clang` (standing in
for `rustc`'s linker). Three findings:

- It links with the same flags `vendor/fluidaudio-rs/build.rs` emits today
  (Foundation, Metal, Accelerate, `c++`, `swiftCore`) plus one search path,
  `$(dirname $(xcrun -f swift))/../lib/swift/macosx`. Without it the link
  fails: `Could not find or use auto-linked library 'swiftCompatibility56'`
  and `Undefined symbols … __swift_FORCE_LOAD_$_swiftCompatibility56`.
- Loading through `loadModel(from:using:)` failed with
  `noModelFactoryAvailable`. The registry finds factories with
  `NSClassFromString("MLXLLM.TrampolineModelFactory")`
  (`ModelFactory.swift:626`), and a static archive only links objects that
  something references. Calling `LLMModelFactory.shared.load(from:using:)`
  directly fixed it.
- In a minimal `.app` layout, MLX found the Metal library at
  `Contents/Resources/mlx-swift_Cmlx.bundle/Contents/Resources/default.metallib`
  (the SwiftPM bundle search in MLX `device.cpp`). The linked program produced
  the same text as Python for window 0 in 4.6 s, including load. That
  layout can be produced with Tauri's bundle resources. The test bundle was
  not signed or notarised.

A one-window run with the older 0.32.2 metallib from the venv also gave the
same text. That does not show that mismatched versions are safe for every
kernel.

**What the two build routes need.**

| Route | Developer machine | CI runner (`macos-latest`) |
| --- | --- | --- |
| Compile the Metal library in `build.rs` | `xcodebuild -downloadComponent MetalToolchain` once per Xcode version (here also `xcodebuild -runFirstLaunch`) | The same, as an explicit step. GitHub's [runner-images #13014](https://github.com/actions/runner-images/issues/13014) reports the toolchain missing or broken on some macOS 26 images; not tested here |
| Ship a prebuilt metallib | Nothing new | Nothing new |

The prebuilt route means: pin the `mlx-metal` wheel by URL and sha256 (the
version must follow the MLX core inside `mlx-swift`, which is 0.32.4 today
while the Python lock pins 0.32.2), extract `mlx/lib/mlx.metallib` and check
its sha256, and copy it into the bundle as above. Loading it from the app data
directory instead (download on demand) needs MLX's C++ `set_metallib_path`
(`metal.cpp:55`), which neither `mlx-c` nor `mlx-swift` exposes, so it would
need a small C++ shim.

**Bundle size.** Sizes are file bytes from `stat`; compressed sizes use
`gzip -6` as a stand-in for DMG compression.

| Item | Uncompressed | gzip -6 |
| --- | --- | --- |
| Current `SottoASR.app` (all files) | 34.9 MB | – |
| Added code (C host after `-dead_strip` and `strip -x`) | 28.0 MB | 8.3 MB |
| Added Metal library (0.32.4) | 214.2 MB | 71.6 MB |
| **Estimated bundle with in-process cleanup** | **about 277 MB** | **+80 MB download** |
| Removed for cleanup users: venv from the lock | 418.7 MB on disk | – |

The code estimate comes from the C host, not from the Rust binary. Every user
would download the Metal library, including users who never enable cleanup,
unless it is fetched on demand.

**Dependencies added.** SwiftPM resolved 12 packages: `mlx-swift-lm`,
`mlx-swift`, `swift-transformers`, `swift-jinja`, `swift-huggingface`,
`swift-crypto`, `swift-asn1`, `swift-collections`, `swift-numerics`, `yyjson`,
`EventSource` and `swift-syntax`. Resolution also downloaded a prebuilt
swift-syntax archive from `download.swift.org`. `swift-transformers` links its
Hub client; for local folders it documents the Hub argument as unused. Network
activity was not monitored.

**What it would replace.** The `LlmBackend` trait in `src-tauri/src/llm/engine.rs`
(`cleanup_batch(&mut self, texts, mode) -> Result<Vec<BatchItem>, String>`) is
the seam; an in-process backend would implement it. It would remove, from
`engine.rs`: `LlmEngine` and its JSON-lines transport (`request_timeout`,
`response_reader`, `nonblocking_stdin`, `resolve_sidecar_script`,
`parse_batch_response`), the venv and Python discovery (`venv_dir`,
`venv_python`, `VenvCache`, `venv_verdict`, `probe_venv_at`,
`find_compatible_python`, `setup_venv`), and the spawn and orphan handling
(`ensure_running`'s spawn path, `is_zombie_error`, `kill_orphan`). It would
also remove the whole of `llm_cleanup.py`, `requirements.lock` and the
standalone-Python download (G-06). The model download in `llm/download.rs`
runs through the sidecar and would need a Rust or Swift replacement. Prompt
construction, the shared-head cache, per-window budgets, the stop tokens, the
32,000-byte output cap, the 10 s deadline and the manifest hash check would
move to Swift or Rust. The planner and validator (`correction.rs`,
`validation.rs`) stay.

It would also remove the process boundary that protects dictation today:

- MLX's default error handler prints `MLX error: …` and calls `exit(-1)`
  (`mlx-c` `error.cpp:13-14`). The missing-metallib run above printed that
  message and exited. Every MLX call in the app would have to run inside
  `withError`.
- About 2.6 GB of peak footprint would move into the app process while the
  model is loaded.
- The sidecar's deadline kills a stuck child process group. In-process, a
  deadline can only be checked between tokens; a stuck GPU call cannot be
  stopped.

## 4. Limits

- One machine (M2 Pro), shared with other build jobs. The production figure
  of 6.6 s was taken elsewhere; the measured replace batch (6.55 s) agrees
  with it.
- 16 synthetic windows. The parity counts do not measure cleanup quality, and
  whether the production validator accepts each output was not run.
- Swift used the PyPI metallib, not one compiled from the `mlx-swift`
  sources, because the Metal Toolchain was not installed.
- The Swift harness is a test program, not a Rust integration. No Rust code
  was built; the static-library test linked a C program.
- Function names and line numbers for `engine.rs` were read on 2026-11-19
  while other slices were editing it; they may move.
- Only the scripts and result files remain under
  `/tmp/experiments/InProcessSpike/`, and `/tmp` is not durable, so this
  document records the measured values.
- Swift batching was established from the 3.33.0 source and API, not by
  trying to force a batch through the model.
- The test `.app` was unsigned; code signing, notarisation and Gatekeeper
  behaviour with a 214 MB resource were not checked.

## 5. Recommendation

**Not viable as a drop-in replacement today.** The model loads, prompts
render to the same token IDs (32/32), and outputs match the production batch
path in 30 of 32 windows (all 16 in the default retype mode). Load is faster
(0.55–0.73 s against 2.07–5.16 s). It would remove Python, the venv and the
standalone-Python download.

Three things block adoption:

1. **The Metal library.** `swift build` cannot produce it without the Metal
   Toolchain on every developer machine and CI runner, and the native build
   system that skips it is deprecated. The alternative is a pinned, hashed
   prebuilt metallib that adds 214 MB (72 MB compressed) to every download
   and must track the MLX version inside `mlx-swift`.
2. **No batched generation.** Sequential windows with a shared-head cache are
   19–27 % slower than the sidecar's 16-window batch; without the cache they
   are 2.5 to 4 times slower.
3. **Lost isolation.** An MLX error exits the app, a stuck GPU call cannot be
   killed, and 2.6 GB of footprint moves into the dictation process.

Keep the sidecar. Reopen this when `mlx-swift-lm` offers batched generation
and the project chooses how to ship the Metal library. If Python removal
becomes the priority first, a small Swift helper process linked against
`mlx-swift` would keep the process boundary; that option was not measured
here.
