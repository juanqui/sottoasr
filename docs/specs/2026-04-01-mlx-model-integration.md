# Replace Qwen Sidecar with Fine-Tuned MLX Model

- **Version:** 1.0
- **Date:** 2026-04-01
- **Status:** Approved (5 reviews completed)

## Table of Contents

1. [Summary](#1-summary)
2. [Problem Statement](#2-problem-statement)
3. [Design Overview](#3-design-overview)
4. [Detailed Design](#4-detailed-design)
5. [File Changes](#5-file-changes)
6. [Migration Plan](#6-migration-plan)
7. [Testing Strategy](#7-testing-strategy)
8. [Implementation Tasks](#8-implementation-tasks)

---

## 1. Summary

Replace the current Qwen3.5 + Python sidecar + mlx-lm cleanup system with our fine-tuned LFM2.5-350M MLX model. Remove model size selection (0.8B/2B/4B choice), remove markdown mode, and simplify to a single toggle: LLM cleanup on/off. The model downloads from HuggingFace on first enable (~233MB for 5-bit MLX).

## 2. Problem Statement

The current system has several issues:
1. **Complex Python sidecar** — spawns a separate Python process, requires a venv with mlx-lm + huggingface_hub
2. **Multiple model choices** — confusing UX (0.8B/2B/4B) that users shouldn't need to think about
3. **Prompt engineering fragility** — the cleanup quality depends on a carefully tuned prompt
4. **Large downloads** — the 2B model is 1.4GB; the new fine-tuned model is 233MB
5. **Slow inference** — the prompted approach takes ~1 second; our model does ~130ms

Our fine-tuned model (ROUGE-L 0.926, 99.3% filler-free) eliminates all of these issues.

## 3. Design Overview

### Current Architecture
```
User speaks → ASR → raw text → Python sidecar (mlx-lm + Qwen3.5) → cleaned text → paste
                                    ↑
                              venv + pip install
                              model download (1.4GB)
                              prompt engineering
                              JSON stdin/stdout protocol
```

### New Architecture
```
User speaks → ASR → raw text → MLX model (native Rust/Swift) → cleaned text → paste
                                    ↑
                              Single model file download (233MB)
                              No Python, no venv, no sidecar
                              Direct mlx-lm inference
```

**However**, mlx-lm is still a Python library. We can't call it directly from Rust without a Python runtime. Two options:

**Option A: Keep the sidecar but massively simplify it** — The sidecar becomes a thin wrapper around our fine-tuned model. No prompt engineering, no model selection, no chat template. Just load model → `generate(prompt)` → return output. The venv setup is still needed but installs only `mlx-lm`.

**Option B: Use llama.cpp/mlx-c for native inference** — Requires converting to GGUF format and using a Rust binding. More complex to implement but eliminates Python entirely.

**Decision: Option A.** The sidecar approach is proven and the simplification is dramatic. The Python overhead is minimal (the sidecar stays running). We can explore Option B in a future release.

### What Changes

| Aspect | Before | After |
|--------|--------|-------|
| Model | Qwen3.5 (0.8B/2B/4B choice) | sotto-cleanup-lfm25-350m-mlx-5bit (single model) |
| Download size | 570MB - 2.97GB | **233MB** |
| Model source | mlx-community HuggingFace | juanquivilla HuggingFace |
| Sidecar prompt | Complex MUST/MUST-NOT + few-shot | `### Input:\n{text}\n\n### Output:\n` |
| Inference | Chat completion with temp/top_p | Greedy generation (temp=0, deterministic) |
| UI: Model selector | Dropdown (0.8B/2B/4B) | **Removed** |
| UI: Markdown mode | Toggle | **Removed** |
| Settings | `llm_model_size`, `llm_markdown_mode` | **Removed** (only `llm_cleanup_enabled`) |
| Cleanup mode | Standard / Markdown | **Standard only** |

## 4. Detailed Design

### 4.1 Sidecar Changes (`sidecar/llm_cleanup.py`)

**Complete rewrite.** The new sidecar:

```python
MODEL_ID = "juanquivilla/sotto-cleanup-lfm25-350m-mlx-5bit"

def cleanup_text(text):
    prompt = f"### Input:\n{text}\n\n### Output:\n"
    output = generate(model, tokenizer, prompt=prompt, max_tokens=512, sampler=greedy_sampler)
    return output.strip()
```

- No system prompt, no chat template, no thinking mode handling
- No mode parameter (always standard)
- Greedy sampling (temp=0) for deterministic output via `generate()` (non-streaming — inference is ~130ms)
- No `--model` CLI argument — hardcoded to our model
- Still supports: `cleanup`, `status`, `download`, `load`, `quit` actions
- **Remove**: `mode` parameter from cleanup action
- **Remove**: `strip_thinking_tags()` — LFM2.5 doesn't emit thinking blocks
- **Remove**: output ratio fallback guard (0.3-2.5x) — fine-tuned model is well-calibrated, false positives harm more than help
- **Keep**: `len(text.split()) < 5` skip — matches Rust-side check in hotkeys/manager.rs

### 4.2 Rust Engine Changes (`src-tauri/src/llm/engine.rs`)

- **Remove** `MODEL_0_8B`, `MODEL_2B`, `MODEL_4B` constants
- **Add** single `SOTTO_MODEL` constant:
  ```rust
  pub const SOTTO_MODEL: ModelConfig = ModelConfig {
      id: "juanquivilla/sotto-cleanup-lfm25-350m-mlx-5bit",
      display_name: "SottoASR Cleanup",
      download_size_mb: 233,
  };
  ```
- **Remove** `model_id_for_size()`, `model_config_for_size()`, `all_model_configs()`
- **Add** `model_config() -> &'static ModelConfig` (returns the single model)
- **Remove** `model_id` field from `LlmEngine` (only one model)
- **Simplify** `spawn_with_model()` → `spawn()` (no model parameter, no `--model` CLI arg to sidecar)
- **Simplify** `cleanup(&mut self, text: &str, mode: CleanupMode)` → `cleanup(&mut self, text: &str)` (no mode)
- **Rename** feature check: `cfg!(feature = "llm-qwen")` → `cfg!(feature = "llm-cleanup")`

### 4.3 Rust Prompts Changes (`src-tauri/src/llm/prompts.rs`)

- **Remove** `CleanupMode` enum entirely (no more Standard/Markdown distinction)

### 4.4 Rust Download Changes (`src-tauri/src/llm/download.rs`)

- **Simplify** `download_model()` — no longer takes `model_size` parameter
- **Simplify** `delete_model()` — always deletes the single model

### 4.5 Rust Commands Changes (`src-tauri/src/commands/llm.rs`)

- **Simplify** `get_llm_status()` — no model size lookup needed
- **Simplify** `download_llm_model()` — no model size from settings
- **Simplify** `delete_llm_model()` — no model size from settings
- **Remove** model-changed respawn logic from commands

### 4.6 Settings Changes (`src-tauri/src/models.rs`)

- **Remove** `llm_markdown_mode` from Settings
- **Remove** `llm_model_size` from Settings
- **Keep** `llm_cleanup_enabled` (the only LLM setting)

### 4.7 Hotkey Manager Changes (`src-tauri/src/hotkeys/manager.rs`)

- **Remove** markdown mode check
- **Remove** model size lookup and model-changed respawn logic
- **Simplify** cleanup call: `llm.cleanup(&text)` (no mode parameter)
- **Keep** 30-second timeout, take/put pattern, stale job check

### 4.8 Frontend Settings Changes

- **Remove** model selector dropdown
- **Remove** markdown mode toggle  
- **Remove** `llmModels` array
- **Simplify** download flow — single button "Download AI Cleanup Model (233 MB)"
- **Keep** enable/disable toggle, download/delete buttons, status display

### 4.9 Frontend Types Changes (`src/lib/utils/tauri.ts`)

- **Remove** `llm_markdown_mode` from Settings interface
- **Remove** `llm_model_size` from Settings interface
- **Simplify** LlmStatus (remove model_name, model_path if unused)

## 5. File Changes

| File | Action | Description |
|------|--------|-------------|
| `src-tauri/sidecar/llm_cleanup.py` | **Rewrite** | Simplify to fine-tuned model only, remove prompts/modes |
| `src-tauri/src/llm/engine.rs` | **Modify** | Single model, remove size selection |
| `src-tauri/src/llm/prompts.rs` | **Delete** | No longer needed (model handles cleanup internally) |
| `src-tauri/src/llm/download.rs` | **Simplify** | Remove model_size parameters |
| `src-tauri/src/llm/mod.rs` | **Modify** | Remove prompts module export |
| `src-tauri/src/commands/llm.rs` | **Simplify** | Remove model size logic |
| `src-tauri/src/models.rs` | **Modify** | Remove llm_markdown_mode, llm_model_size from Settings |
| `src-tauri/src/hotkeys/manager.rs` | **Simplify** | Remove mode/model-size logic from cleanup flow |
| `src/lib/utils/tauri.ts` | **Modify** | Remove llm_markdown_mode, llm_model_size |
| `src/lib/components/settings-panel.svelte` | **Simplify** | Remove model selector, markdown toggle |
| `src/lib/stores/settings.svelte.ts` | **Modify** | Remove unused setting fields |
| `src-tauri/Cargo.toml` | **Modify** | Rename feature `llm-qwen` → `llm-cleanup` |
| `src-tauri/src/state.rs` | **Verify** | Ensure LlmEngine import still works after field removal |

## 6. Migration Plan

### Backward Compatibility

Users with existing settings will have `llm_model_size: "2b"` and `llm_markdown_mode: false` in their `settings.json`. The new code handles this because:
1. **serde_json ignores unknown fields by default** — the old keys are silently dropped during deserialization
2. On next save, the old fields will be dropped naturally (only the current struct fields are serialized)
3. **Do NOT add `#[serde(deny_unknown_fields)]`** to the Settings struct — this would break upgrades

### Feature Flag

Rename the Cargo feature flag from `llm-qwen` to `llm-cleanup` in `Cargo.toml` to reflect the new model. Update `is_feature_compiled()` in `engine.rs` accordingly.

Users with downloaded Qwen models:
1. The old models stay in `~/.cache/huggingface/hub/` — we don't delete them
2. The new model downloads to the same cache location
3. Users can manually clean up old models (~1.4GB) if they want

### First-Launch After Upgrade

**Critical:** If a user has `llm_cleanup_enabled: true` and upgrades, the new model is NOT downloaded. If the sidecar tries to auto-download via `mlx_lm.load()`, the 30-second timeout will expire and cleanup silently fails.

**Solution:** On sidecar startup, if the model is not downloaded, return a clear `{"ok": false, "error": "model_not_downloaded"}` instead of attempting auto-download. The Rust side detects this and:
1. Sets `llm_cleanup_enabled = false` in settings
2. Logs a message: "LLM cleanup disabled — new model requires download from Settings"
3. Uses raw text for this transcription
4. The user re-enables from Settings, which triggers the download flow

### Venv Compatibility

The existing venv at `~/.local/share/com.sottoasr.app/llm-venv/` already has `mlx-lm` installed. Our new sidecar uses the same library. However, the LFM2.5 model may require a newer mlx-lm version. The sidecar startup should run `pip install --quiet --upgrade mlx-lm` to ensure compatibility (adds ~2-3 seconds on first launch after update, no-op on subsequent launches).

## 7. Testing Strategy

### Manual Verification

1. **Fresh install:** App with no prior LLM setup
   - Enable LLM cleanup → shows download button (233 MB)
   - Download → progress → complete
   - Record "uh the server is uh running low on memory" → "The server is running low on memory."
   - Check history: "AI CLEANED" badge, Raw view, Diff view

2. **Upgrade from old version:**
   - App with Qwen 2B downloaded
   - Update → settings still work (cleanup enabled/disabled)
   - Old model-size/markdown settings gracefully ignored
   - New model downloads on first cleanup

3. **Cleanup quality:**
   - Filler removal: "uh um er"
   - Self-correction: "use X wait no use Y"
   - Preserve wording: "let's go ahead and deploy this"
   - Dictation commands: "period comma slash"
   - Short inputs: "ship it"
   - Long dictation: multi-sentence passage

4. **Edge cases:**
   - Input < 5 words → skip cleanup
   - Model not downloaded → cleanup skipped (uses raw text)
   - Cancel recording during cleanup → stale job discarded

## 8. Implementation Tasks

1. [ ] Rewrite `sidecar/llm_cleanup.py` — single model, no prompts, greedy generation, remove --model arg
2. [ ] Modify `llm/engine.rs` — single model config, remove size selection, remove model_id field, stop passing --model to sidecar
3. [ ] Delete `llm/prompts.rs` — no longer needed
4. [ ] Simplify `llm/download.rs` — remove model_size params
5. [ ] Update `llm/mod.rs` — remove prompts module
6. [ ] Rename feature flag in `Cargo.toml`: `llm-qwen` → `llm-cleanup`
7. [ ] Simplify `commands/llm.rs` — remove model size logic
8. [ ] Update `models.rs` — remove llm_markdown_mode, llm_model_size from Settings
9. [ ] Simplify `hotkeys/manager.rs` — remove mode/model-size from cleanup flow, remove model-changed respawn
10. [ ] Update `src/lib/utils/tauri.ts` — remove unused types
11. [ ] Simplify `settings-panel.svelte` — remove model selector, markdown toggle, hardcode 233MB size
12. [ ] Build and test: `cargo tauri dev`
13. [ ] Build production: `cargo tauri build`
14. [ ] Install and manual test
