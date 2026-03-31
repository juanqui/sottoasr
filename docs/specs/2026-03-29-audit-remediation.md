# Audit Remediation Specification

- **Version:** 1.0
- **Date:** 2026-03-29
- **Status:** Draft
- **Supersedes:** N/A

---

## Table of Contents

1. [Summary](#1-summary)
2. [Critical Issues](#2-critical-issues)
3. [High Priority Issues](#3-high-priority-issues)
4. [Medium Priority Issues](#4-medium-priority-issues)
5. [Low Priority Issues](#5-low-priority-issues)
6. [Testing Infrastructure](#6-testing-infrastructure)
7. [Documentation Updates](#7-documentation-updates)
8. [Implementation Tasks](#8-implementation-tasks)

---

## 1. Summary

This specification documents how to address each finding from the [SottoASR Application Audit Report](../audit/2026-03-29-application-audit.md).

### Audit Review Summary

The audit was reviewed by 5 independent passes. Key findings:

| Category | Finding |
|----------|---------|
| **Critical Runtime Bugs** | CGEventTap potential panic, ASR lock timeout, LLM Drop deadlock |
| **Testing Gaps** | No unit, integration, or frontend tests |
| **Security** | CSP is adequate; sidecar is secure; no injection vulnerabilities found |
| **Reliability** | Missing retry logic, panic recovery, health monitoring |
| **Documentation** | Architecture doc significantly outdated |

### Severity Revision

Based on review feedback, the following severity corrections apply:

| Issue | Original | Revised | Reason |
|-------|----------|---------|--------|
| C-001 | Medium | Low | Per-callback overflow is bounded, not unbounded |
| C-002 | Medium | High | Raw pointer dereference = guaranteed crash |
| C-004 | Medium | Low | Drop with blocking I/O is poor practice but narrow scenario |
| M-003 | Medium | Low | Bounded channel with backpressure is correct behavior |
| D-001 | Medium | High | Architecture doc actively misleads developers |

---

## 2. Critical Issues

These issues MUST be addressed before release.

### CR-001: Potential Panic in CGEventTap Callback

**File:** `src-tauri/src/commands/keycapture.rs:106`

**Issue:** Raw pointer dereference without null check:
```rust
let app = &*(user_info as *const AppHandle);
```

**Fix:**
```rust
if user_info.is_null() {
    return std::ptr::null();
}
let app = &*(user_info as *const AppHandle);
```

**Verification:**
- Add assertion in callback
- Add integration test for key capture lifecycle

---

### CR-002: No Timeout on ASR Engine Lock

**File:** `src-tauri/src/hotkeys/manager.rs:449`

**Issue:** `asr_engine.lock().await` blocks indefinitely if engine hangs.

**Fix:** Use `tokio::time::timeout`:
```rust
use tokio::time::{timeout, Duration};

let result = timeout(Duration::from_secs(60), async {
    let mut engine = state.asr_engine.lock().await;
    engine.transcribe_file(&temp_path_str)
}).await;

match result {
    Ok(Ok(asr_result)) => { /* success */ }
    Ok(Err(e)) => { /* transcription error */ }
    Err(_) => { /* timeout - log and return error */ }
}
```

**Verification:**
- Test with mock ASR that hangs
- Ensure timeout returns error state correctly

---

### CR-003: LLM Engine Drop Deadlock Potential

**File:** `src-tauri/src/llm/engine.rs:199-203`

**Issue:** `Drop` calls blocking `quit()` which could deadlock during async unwinding.

**Fix:** Remove `quit()` from `Drop` and require explicit cleanup:
```rust
impl Drop for LlmEngine {
    fn drop(&mut self) {
        // Don't call quit() here - Drop is called during async unwind
        // which can cause deadlock. Require explicit quit() instead.
    }
}

// Add explicit shutdown method:
pub fn shutdown(&mut self) {
    let _ = self.request(&serde_json::json!({"action": "quit"}));
    // ... existing timeout/kill logic
}
```

**Update call sites:** Ensure `quit()` is called when LLM engine is explicitly removed from the `Option` in `AppState`.

**Verification:**
- Test that LLM engine is properly cleaned up on settings change
- Test that app exits cleanly with LLM running

---

### CR-004: No Panic Recovery in Audio Callback

**File:** `src-tauri/src/audio/capture.rs:51-84`

**Issue:** The cpal callback runs on a real-time thread. If any operation panics, the app crashes.

**Fix:** Wrap callback logic in `catch_unwind`:
```rust
use std::panic::catch_unwind;

let stream = device.build_input_stream(
    &config.into(),
    move |data: &[f32], _: &cpal::InputCallbackInfo| {
        // Wrap entire callback in catch_unwind
        let result = catch_unwind(std::panic::AssertUnwindSafe(|| {
            // ... existing callback logic ...
        }));
        
        if result.is_err() {
            log::error!("Audio callback panicked - resetting capture");
            is_recording_clone.store(false, Ordering::Relaxed);
        }
    },
    // ...
);
```

**Verification:**
- Add chaos testing with random panic injection
- Verify audio stream recovers gracefully

---

### CR-005: No LLM Sidecar Health Monitoring

**File:** `src-tauri/src/llm/engine.rs`

**Issue:** If the Python MLX sidecar crashes, the app silently fails LLM cleanup with no user feedback.

**Fix:** Add health check before each cleanup:
```rust
// Before cleanup, verify sidecar is responsive
fn is_sidecar_healthy(&mut self) -> bool {
    match self.request(&serde_json::json!({"action": "status"})) {
        Ok(resp) => resp.get("ok").and_then(|v| v.as_bool()) == Some(true),
        _ => false,
    }
}

// If unhealthy, respawn before cleanup
if !llm.is_sidecar_healthy() {
    log::warn!("LLM sidecar unhealthy, respawning");
    self.spawn_with_model(&self.model_id)?;
}
```

**Verification:**
- Test with simulated sidecar crash
- Verify error message shown to user

---

## 3. High Priority Issues

### HP-001: No Retry on Model Download

**File:** `src-tauri/src/asr/model.rs:131-217`

**Issue:** Download fails immediately on any network error.

**Fix:** Add retry with exponential backoff:
```rust
const MAX_RETRIES: u32 = 3;
const INITIAL_BACKOFF_MS: u64 = 1000;

for attempt in 0..MAX_RETRIES {
    match download_file(&client, &url, &file_path).await {
        Ok(()) => break,
        Err(e) if attempt < MAX_RETRIES - 1 => {
            let backoff = INITIAL_BACKOFF_MS * 2u64.pow(attempt);
            log::warn!("Download failed (attempt {}), retrying in {}ms: {}", 
                attempt + 1, backoff, e);
            tokio::time::sleep(Duration::from_millis(backoff)).await;
        }
        Err(e) => return Err(e),
    }
}
```

**Verification:**
- Test with network interruption simulation
- Test with HTTP 500/503 responses

---

### HP-002: No Settings Validation

**File:** `src-tauri/src/models.rs:39-65`

**Issue:** Settings values are not validated before use.

**Fix:** Add validation in `update_settings` command:
```rust
impl Settings {
    pub fn validate(&self) -> Result<(), String> {
        if self.max_history < 10 {
            return Err("max_history must be at least 10".into());
        }
        if self.max_history > 10000 {
            return Err("max_history cannot exceed 10000".into());
        }
        if self.llm_model_size != "0.8b" 
            && self.llm_model_size != "2b" 
            && self.llm_model_size != "4b" {
            return Err("llm_model_size must be 0.8b, 2b, or 4b".into());
        }
        // ... other validations
        Ok(())
    }
}
```

**Verification:**
- Add unit tests for validation
- Test edge cases (0, negative, very large values)

---

### HP-003: Architecture Documentation Outdated

**File:** `docs/designs/architecture.md`

**Issue:** Documents parakeet-rs as primary ASR but actual implementation uses FluidAudio. Missing `CleaningUp` state, LLM features, sidecar architecture.

**Fix:** Create new architecture document with accurate information:
- Primary ASR: FluidAudio (CoreML/ANE)
- State machine: Idle → Recording → Transcribing → CleaningUp → Pasting → Idle
- LLM cleanup via Python MLX sidecar
- Settings fields: `llm_cleanup_enabled`, `llm_markdown_mode`, `llm_model_size`

**Verification:**
- Cross-reference all doc sections with actual implementation
- Add doc build CI check

---

## 4. Medium Priority Issues

### MP-001: Short Recording Discarded Silently

**File:** `src-tauri/src/hotkeys/manager.rs:394-399`

**Issue:** Recordings under 4000 samples are discarded without user notification.

**Fix:** Emit a warning event:
```rust
if samples.len() < 4000 {
    log::warn!("Recording too short ({} samples), discarding", samples.len());
    state.set_state(AppStateEnum::Idle);
    app.emit("recording-discarded", serde_json::json!({
        "reason": "too_short",
        "samples": samples.len()
    })).map_err(|e| e.to_string())?;
    app.emit("state-changed", &AppStateEnum::Idle).map_err(|e| e.to_string())?;
    return;
}
```

**Frontend:** Show toast notification when `recording-discarded` event received.

---

### MP-002: No Keyboard Navigation in Settings

**File:** `src/lib/components/settings-panel.svelte`

**Issue:** Settings panel doesn't support keyboard navigation.

**Fix:** Add `tabindex`, keyboard handlers for buttons:
```svelte
<button 
    onclick={handleSave}
    onkeydown={(e) => e.key === 'Enter' && handleSave()}
    disabled={...}
>
    Save
</button>
```

**Verification:**
- Manual accessibility audit

---

### MP-003: Overlay Not Respecting `show_overlay` Setting

**File:** `src-tauri/src/hotkeys/manager.rs:288-289`

**Issue:** `show_overlay(app)` called regardless of settings.

**Fix:** Check setting before showing:
```rust
let settings = state.settings.lock().await;
if settings.show_overlay {
    show_overlay(app);
}
```

---

### MP-004: CSV Export Format Bug

**File:** `src-tauri/src/commands/transcription.rs:84-88`

**Issue:** Stray `.` in format string creates malformed CSV.

**Fix:**
```rust
csv.push_str(&format!(
    "{},{},{},{},{},\"{}\",\"{}\"\n",
    t.id, t.created_at, t.duration_ms, t.word_count, t.llm_applied,
    text_escaped, raw_escaped,
));
```

**Verification:**
- Export sample transcription and validate CSV format

---

## 5. Low Priority Issues

### LP-001: Audio Level Logging in Production

**File:** `src-tauri/src/audio/capture.rs:79-82`

**Fix:** Remove or guard with debug logging:
```rust
#[cfg(debug_assertions)]
if level_emit_count % 30 == 1 {
    log::info!("Audio level: {:.4} (emit #{})", rms, level_emit_count);
}
```

---

### LP-002: Waveform Animation Runs in Background

**File:** `src/lib/components/waveform.svelte`

**Fix:** Pause animation when tab not visible:
```svelte
$effect(() => {
    if (typeof document !== 'undefined') {
        const visibility_handler = () => {
            if (document.hidden) {
                cancelAnimationFrame(animFrameId);
            } else {
                animFrameId = requestAnimationFrame(render);
            }
        };
        document.addEventListener('visibilitychange', visibility_handler);
        return () => document.removeEventListener('visibilitychange', visibility_handler);
    }
});
```

---

### LP-003: Python Sidecar Startup Overhead

**File:** `src-tauri/src/llm/engine.rs`

**Fix:** Keep sidecar alive with idle timeout:
```rust
// Keep sidecar alive for 5 minutes after last use
const SIDECAR_IDLE_TIMEOUT = Duration::from_secs(300);

// In cleanup(), reset idle timer instead of quitting
```

---

## 6. Testing Infrastructure

### TI-001: Add Rust Unit Tests

**Files to test:**
- `state.rs` - State machine transitions
- `audio/capture.rs` - Buffer handling
- `asr/engine.rs` - Engine trait implementations
- `commands/transcription.rs` - Storage operations

**Setup:**
```bash
# Add to Cargo.toml
[dev-dependencies]
mockall = "0.12"
tokio-test = "0.4"
```

**Example test:**
```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_state_transitions() {
        let state = AppState::new();
        state.set_state(AppStateEnum::Recording);
        assert_eq!(state.get_state(), AppStateEnum::Recording);
    }
}
```

---

### TI-002: Add Frontend Tests

**Setup:**
```bash
npm install -D vitest @testing-library/svelte jsdom
```

**Example test:**
```typescript
import { describe, it, expect } from 'vitest';
import { formatDuration } from '../utils/format';

describe('formatDuration', () => {
    it('formats seconds correctly', () => {
        expect(formatDuration(3000)).toBe('0:03');
    });
    it('formats minutes correctly', () => {
        expect(formatDuration(65000)).toBe('1:05');
    });
});
```

---

### TI-003: Add Integration Tests

**Rust integration test:**
```rust
#[tokio::test]
async fn test_recording_flow() {
    // Start app with test harness
    // Simulate hotkey press
    // Verify recording state
    // Simulate hotkey release
    // Verify transcription event
}
```

---

## 7. Documentation Updates

### DOC-001: Update Architecture Document

**Required changes:**

1. **ASR Engine**: Change primary from parakeet-rs to FluidAudio
2. **State Machine**: Add `CleaningUp` state between `Transcribing` and `Pasting`
3. **LLM Feature**: Document full LLM cleanup pipeline
4. **Sidecar**: Document Python MLX sidecar architecture
5. **Data Models**: Add `llm_applied`, `raw_text`, `cancelled` fields to `Transcription`
6. **Settings**: Add `llm_cleanup_enabled`, `llm_markdown_mode`, `llm_model_size`

**Verification:**
- Doc build CI job to check doc compiles
- Review against implementation checklist

---

## 8. Implementation Tasks

### Phase 1: Critical Fixes (Release Blockers)

- [ ] CR-001: Add null check to CGEventTap callback
- [ ] CR-002: Add timeout to ASR engine lock
- [ ] CR-003: Remove blocking I/O from LLM Drop
- [ ] CR-004: Add panic recovery to audio callback
- [ ] CR-005: Add LLM health monitoring

### Phase 2: High Priority (Fix Soon)

- [ ] HP-001: Add download retry with backoff
- [ ] HP-002: Add settings validation
- [ ] HP-003: Update architecture documentation

### Phase 3: Medium Priority (Polish)

- [ ] MP-001: Emit event for discarded recordings
- [ ] MP-002: Add keyboard navigation to settings
- [ ] MP-003: Respect show_overlay setting
- [ ] MP-004: Fix CSV export format

### Phase 4: Low Priority (Nice to Have)

- [ ] LP-001: Guard audio level logging
- [ ] LP-002: Pause waveform in background
- [ ] LP-003: Add sidecar idle timeout

### Phase 5: Testing Infrastructure

- [ ] TI-001: Add Rust unit tests
- [ ] TI-002: Add frontend tests
- [ ] TI-003: Add integration tests

---

## Appendix: File Changes Summary

| File | Changes |
|------|---------|
| `src-tauri/src/commands/keycapture.rs` | Null check, panic recovery |
| `src-tauri/src/hotkeys/manager.rs` | ASR timeout, settings check |
| `src-tauri/src/llm/engine.rs` | Drop fix, health monitoring |
| `src-tauri/src/audio/capture.rs` | Panic recovery, debug logging |
| `src-tauri/src/asr/model.rs` | Download retry |
| `src-tauri/src/models.rs` | Settings validation |
| `src-tauri/src/commands/transcription.rs` | CSV format fix |
| `src/lib/components/settings-panel.svelte` | Keyboard navigation |
| `src/lib/components/overlay-pill.svelte` | Discarded event handler |
| `src/lib/utils/format.ts` | Unit tests |
| `docs/designs/architecture.md` | Full rewrite |

---

*Specification created based on audit report dated 2026-03-29*
