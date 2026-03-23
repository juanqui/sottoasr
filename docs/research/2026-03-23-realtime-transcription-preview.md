# Real-Time Transcription Preview — Feasibility Research

- **Version:** 1.0
- **Date:** 2026-03-23
- **Status:** Research Complete

## Table of Contents

1. [Summary](#1-summary)
2. [Problem Statement](#2-problem-statement)
3. [Current Architecture Constraints](#3-current-architecture-constraints)
4. [Streaming ASR Landscape](#4-streaming-asr-landscape)
5. [Candidate Approaches](#5-candidate-approaches)
6. [Approach Comparison](#6-approach-comparison)
7. [Recommendations](#7-recommendations)
8. [UI/UX Considerations](#8-uiux-considerations)
9. [Open Questions](#9-open-questions)

## 1. Summary

Real-time transcription preview during recording is **feasible and practical** on Apple Silicon. There are multiple viable approaches, each with different tradeoffs. The most promising path is to use FluidAudio's existing `StreamingEouAsrManager` (Parakeet EOU 120M) for real-time partial transcripts, which already runs on CoreML/ANE — the same infrastructure we use today. The primary obstacle is that `fluidaudio-rs` (our Rust FFI bridge) does not yet expose the streaming API.

## 2. Problem Statement

Currently, SottoASR shows only a waveform and timer during recording. The user has no idea what is being transcribed until recording stops. This creates:

- **Uncertainty:** Users don't know if the microphone is working or if their speech is being captured correctly.
- **No error correction opportunity:** Misheard words, wrong context, or mic issues are only discovered after the full transcription completes.
- **Reduced confidence:** Users may re-record unnecessarily.

**Goal:** Show a live preview of what the user is saying — as close to real-time as possible — in or near the existing recording overlay. This is strictly a *preview* mechanism. The final transcription can still use the high-accuracy batch pipeline.

## 3. Current Architecture Constraints

### What we have

| Component | Detail |
|-----------|--------|
| Audio capture | cpal @ device native rate (~48 kHz), mono, f32, pushed to mpsc channel |
| ASR engine | FluidAudio (Parakeet TDT v3, 0.6B) via `fluidaudio-rs` Rust FFI |
| Processing mode | **Batch only** — all audio collected, written to temp WAV, transcribed after recording stops |
| Rust FFI surface | `transcribe_file(path)` only — no streaming, no sample-based API |
| Compute target | CoreML on Apple Neural Engine (ANE) |
| Overlay | Tauri webview (NSPanel), shows waveform + timer |

### Key blockers for real-time preview

1. **`fluidaudio-rs` is batch-only.** The Rust bridge exposes `transcribe_file()` but not the Swift SDK's `StreamingEouAsrManager` or `StreamingAsrManager`.
2. **FluidAudio's batch model (Parakeet TDT v3) requires complete audio.** The TDT architecture uses non-causal attention — it looks at future context — making incremental processing architecturally impossible without the streaming variant.
3. **Audio is accumulated in a channel, not streamed.** Samples flow from the cpal callback to `audio_sender` and are only drained after recording stops.

### What FluidAudio Swift SDK already offers (but Rust doesn't expose)

The Swift SDK has **two streaming ASR managers** we cannot currently access from Rust:

**`StreamingAsrManager`** — Sliding-window batch ASR for real-time use
- Uses the same Parakeet TDT v3 model
- Processes audio in sliding windows with token deduplication
- Emits "confirmed" (stable) + "volatile" (may change) transcript segments
- ~120x RTF on M4 Pro

**`StreamingEouAsrManager`** — True streaming ASR with end-of-utterance detection
- Uses **Parakeet EOU** model (120M params, ~150 MB)
- Chunk sizes: 160ms (8.29% WER), 320ms (4.87% WER)
- End-of-utterance token detection (ID 1024) with configurable debounce
- Partial + EOU callbacks
- ~5x RTF at 160ms, ~12x RTF at 320ms on Apple Silicon
- **English only**
- Cache-aware streaming with LSTM decoder state

## 4. Streaming ASR Landscape

### Models purpose-built for streaming

| Model | Params | Latency | WER | Platform | License | Notes |
|-------|--------|---------|-----|----------|---------|-------|
| **Parakeet EOU 120M** | 120M | 160-320ms | 5-8% | CoreML/ANE | NVIDIA Open | Already in FluidAudio Swift SDK; English only |
| **Moonshine v2 Medium** | 245M | 258ms | 6.65% | ONNX (CPU) | MIT | Streaming encoder, sliding-window attention, cross-platform |
| **Moonshine v2 Small** | 123M | 148ms | — | ONNX (CPU) | MIT | Lighter variant |
| **Moonshine v2 Tiny** | 34M | 50ms | — | ONNX (CPU) | MIT | Ultra-low latency, higher WER |
| **Voxtral Realtime** | 4B | 200-400ms | — | MLX (GPU) | Apache 2.0 | Too large for background use (~4 GB) |

### Models NOT designed for streaming (workaround only)

| Model | Approach | Limitation |
|-------|----------|------------|
| Whisper / whisper.cpp | Overlapping chunks, re-stitch | Chunk boundary artifacts; encoder is non-causal so no KV cache reuse |
| Parakeet TDT (batch) | Re-transcribe growing audio file | Latency grows linearly with recording length |
| WhisperKit | Swift SDK with streaming mode | Separate ecosystem from our Tauri/Rust stack |

### Key architectural distinction

**Causal (streaming-native):** Parakeet EOU, Moonshine v2, Voxtral Realtime
- Encoder processes audio left-to-right, never needs future context
- Can emit tokens as audio arrives
- True incremental processing with state caching

**Non-causal (batch-native):** Whisper, Parakeet TDT, FluidAudio batch
- Encoder uses bidirectional attention — needs entire input before processing
- "Streaming" requires reprocessing from scratch each time
- Higher accuracy because it sees full context

## 5. Candidate Approaches

### Approach A: Extend `fluidaudio-rs` to expose `StreamingEouAsrManager`

**How it works:**
1. Add FFI bindings in fluidaudio-rs for `StreamingEouAsrManager`
2. Download Parakeet EOU model (~150 MB) alongside existing TDT model
3. During recording: feed audio chunks to streaming ASR, display partial transcripts
4. On recording stop: use batch Parakeet TDT for final high-accuracy transcription
5. Replace preview text with final text

**Pros:**
- Uses existing FluidAudio infrastructure (CoreML, ANE, model management)
- Parakeet EOU is purpose-built for this exact use case
- 160-320ms chunk size is ideal for responsive preview
- Partial + EOU callbacks give natural utterance boundaries
- Very small model (120M) — minimal memory alongside TDT

**Cons:**
- Requires contributing to or forking `fluidaudio-rs` (upstream dependency)
- English only for streaming preview (TDT v3 batch handles 25 languages)
- Two ASR models loaded simultaneously (~650 MB total)
- ANE contention possible (both models use ANE, though not simultaneously)

**Effort:** Medium — FFI binding work + audio pipeline refactoring
**Risk:** Medium — depends on upstream willingness to accept PR, or maintaining a fork

### Approach B: Dual-engine — Moonshine v2 (streaming) + FluidAudio (batch)

**How it works:**
1. Bundle Moonshine v2 Small/Medium ONNX model (~60-120 MB)
2. Run Moonshine on CPU in a background thread during recording
3. Feed audio chunks (50-258ms), display partial transcripts in overlay
4. On recording stop: use FluidAudio TDT for final batch transcription
5. Replace preview with final text

**Pros:**
- Moonshine runs on CPU; FluidAudio runs on ANE — no resource contention
- MIT license, no upstream dependency risk
- Cross-platform (ONNX Runtime works on macOS, Windows, Linux)
- Purpose-built streaming architecture with very low latency
- Well-tested: used in production by TypeWhisper, Rift, and others

**Cons:**
- New dependency (onnxruntime) — increases app bundle size
- Two completely separate ASR engines to maintain
- Moonshine is relatively new (v2 paper: Feb 2026)
- CPU usage during recording (Moonshine) + ANE usage after (FluidAudio)
- English-focused (multilingual support in progress)

**Effort:** Medium-High — integrate ONNX Runtime, Moonshine inference, audio pipeline
**Risk:** Low — Moonshine is stable, MIT-licensed, well-documented

### Approach C: Chunked re-transcription with FluidAudio batch

**How it works:**
1. During recording, every N seconds (e.g., 2s):
   - Write accumulated audio so far to a temp WAV file
   - Call `transcribe_file()` on it
   - Display result as preview
2. On recording stop: final transcription of complete audio

**Pros:**
- Uses existing FluidAudio batch API — zero new dependencies
- Simplest implementation (timer + temp file writes)
- Full Parakeet TDT accuracy even in preview

**Cons:**
- **Latency grows linearly:** 2s audio = fast; 60s audio = re-transcribing 60s every cycle
- At FluidAudio's ~120x RTF, 60s audio takes ~0.5s to transcribe — but 5 min takes ~2.5s
- Wasteful: processes the entire recording from scratch each time
- Disk I/O: frequent temp file writes
- ANE contention: batch transcription during recording could affect audio capture
- Not practical beyond ~30 seconds of recording

**Effort:** Low
**Risk:** Low (but poor UX for longer recordings)

### Approach D: Python sidecar with parakeet-mlx streaming

**How it works:**
1. Python sidecar (like the LLM sidecar) running `parakeet-mlx` with streaming API
2. Feed audio chunks via stdin (base64-encoded PCM)
3. Receive partial transcripts via stdout JSON
4. Display in overlay

**Pros:**
- `parakeet-mlx` already has a `transcribe_stream()` API
- Runs on Metal GPU via MLX
- Same model ecosystem as our LLM feature
- Pattern already proven (LLM sidecar)

**Cons:**
- Another Python sidecar process to manage
- MLX uses GPU — may contend with other GPU workloads
- parakeet-mlx streaming is "chunked with context" not true incremental
- Higher memory overhead (Python + MLX runtime)
- Extra ~400 MB model download

**Effort:** Medium
**Risk:** Medium — depends on parakeet-mlx streaming maturity

### Approach E: Replace FluidAudio entirely with Moonshine

**How it works:**
1. Replace FluidAudio with Moonshine v2 as the sole ASR engine
2. Use streaming mode during recording for real-time preview
3. Use the same engine's accumulated result as the final transcription
4. Single model, single engine

**Pros:**
- Simplest architecture — one engine does everything
- No resource contention
- MIT license, fully open
- Small models (34-245M params)
- Cross-platform (future Windows/Linux support)

**Cons:**
- Moonshine v2 Medium (6.65% WER) is less accurate than Parakeet TDT v3 (~5% WER on standard benchmarks)
- Losing FluidAudio's 25-language support
- ONNX Runtime on CPU — slower than ANE for batch processing
- Losing FluidAudio's speaker diarization capability (future feature)
- Major refactor — replace the entire ASR backend

**Effort:** High
**Risk:** Medium — trading accuracy for simplicity

## 6. Approach Comparison

| | A: FluidAudio Streaming | B: Moonshine + FluidAudio | C: Chunked Re-transcribe | D: parakeet-mlx Sidecar | E: Moonshine Only |
|---|---|---|---|---|---|
| **Preview latency** | 160-320ms | 50-258ms | 2-5s (growing) | ~500ms | 50-258ms |
| **Preview accuracy** | ~5-8% WER | ~6.7% WER | ~5% WER | ~5% WER | ~6.7% WER |
| **Final accuracy** | ~5% WER (TDT) | ~5% WER (TDT) | ~5% WER (TDT) | ~5% WER (TDT) | ~6.7% WER |
| **New dependencies** | None (extend FFI) | onnxruntime | None | Python + MLX | onnxruntime |
| **Model download** | +150 MB | +60-120 MB | None | +400 MB | Replace 500 MB |
| **Resource contention** | Possible (both ANE) | None (CPU + ANE) | Yes (ANE during recording) | Possible (GPU) | None |
| **Long recording OK?** | Yes | Yes | No (>30s degrades) | Yes | Yes |
| **Multilingual preview** | No (English only) | No (English-focused) | Yes (25 languages) | Yes (25 languages) | No |
| **Implementation effort** | Medium | Medium-High | Low | Medium | High |
| **Upstream risk** | Medium (fork/PR) | Low | None | Medium | Low |

## 7. Recommendations

### Primary: Approach A (extend fluidaudio-rs)

**This is the ideal path.** FluidAudio's `StreamingEouAsrManager` is purpose-built for exactly our use case — live preview with end-of-utterance detection, running on the same ANE infrastructure we already use. The 120M Parakeet EOU model adds only ~150 MB and provides 160-320ms latency with ~5% WER at 320ms chunks.

**Steps to validate:**
1. Check with FluidAudio team (Discord/GitHub) if they plan to add streaming to fluidaudio-rs
2. If not, fork fluidaudio-rs and add the FFI bindings ourselves
3. The Swift API is well-documented: `startStreaming(eouCallback:partialCallback:)` + `feedAudio(samples)`
4. Prototype: feed audio chunks during recording, display partial text in overlay

**The EOU (end-of-utterance) detection is particularly valuable** — it gives natural sentence boundaries, which makes the preview text much more readable than a continuous stream.

### Fallback: Approach B (Moonshine v2 + FluidAudio)

If extending fluidaudio-rs proves impractical, Moonshine v2 is the best alternative. It's designed from the ground up for streaming, runs on CPU (no ANE contention), has MIT license (no upstream risk), and is being actively developed.

**Key advantage over Approach A:** Moonshine runs on CPU while FluidAudio uses ANE — zero resource contention. This means we can run streaming preview AND batch transcription simultaneously if needed.

**Steps to validate:**
1. Test Moonshine v2 Small ONNX model on Apple Silicon CPU — measure latency and WER
2. Prototype: integrate onnxruntime-rs, feed audio chunks, display results
3. Compare preview quality vs Parakeet EOU

### Not recommended for MVP

- **Approach C** (chunked re-transcription): Works for short recordings but degrades badly. Acceptable as a stopgap but not as a long-term solution.
- **Approach D** (parakeet-mlx sidecar): Adds complexity with marginal benefit over A or B.
- **Approach E** (replace FluidAudio): Too disruptive; losing Parakeet TDT accuracy and FluidAudio's ecosystem isn't justified.

## 8. UI/UX Considerations

### Overlay design

The preview should be non-intrusive and clearly indicate it's a draft:

**Option 1: Expand the existing pill**
- Pill grows vertically to show 1-2 lines of preview text
- Text appears below the waveform
- Subtle styling (lighter color, smaller font) to indicate "preview"
- Fades/shrinks when text is confirmed (EOU detected)

**Option 2: Separate preview panel above the pill**
- Second floating panel above the recording pill
- Shows scrolling text as the user speaks
- Can be larger and show more context
- Independent of pill animation

**Option 3: Minimal subtitle strip**
- Thin strip below the pill showing only the most recent phrase
- Rolls over as new text arrives
- Least intrusive, most familiar (like live captions)

**Recommendation:** Start with Option 1 (expand the pill) — it's the most cohesive with the existing UI and avoids a second window.

### Preview text behavior

- **Partial text** (volatile): Show in lighter color or with a cursor/blinking indicator
- **Confirmed text** (EOU/stable): Show in full color, shift up
- **Corrections**: When partial text is revised by the model, update in-place (no flicker)
- **Final replacement**: When recording stops and batch transcription completes, smoothly replace preview with final text

### Settings

- Toggle: "Show live preview" (default: on)
- No model size selection needed (the streaming model is fixed at 120M)

## 9. Open Questions

1. **ANE scheduling:** Can Parakeet EOU (streaming) and Parakeet TDT (batch) share the ANE without contention? FluidAudio runs them sequentially (streaming during recording, batch after), but we need to verify the ANE context switch is clean.

2. **fluidaudio-rs upstream:** Will the FluidAudio team accept a PR adding streaming bindings? Or do we need to maintain a fork? Check their Discord/GitHub issues.

3. **Multilingual preview:** Parakeet EOU is English-only. For non-English users, should we fall back to no preview, or use Moonshine (which has limited multilingual support)?

4. **Memory budget:** Loading both TDT (0.6B) and EOU (120M) simultaneously. FluidAudio manages CoreML model compilation to ANE — need to verify both fit in ANE cache without thrashing.

5. **Audio pipeline refactoring:** Currently audio flows through an mpsc channel and is drained after recording. For streaming, we need a tee: audio chunks go to both the channel (for final batch) AND the streaming ASR. This is straightforward but touches the core recording path.

6. **LLM cleanup interaction:** If streaming preview is shown, should the LLM cleanup also show a preview? Or only apply to the final text? Recommend: only apply to final text.
