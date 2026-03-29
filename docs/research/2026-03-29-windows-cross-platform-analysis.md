# Windows Cross-Platform Compatibility Analysis

- **Version:** 1.0
- **Date:** 2026-03-29
- **Status:** Draft

## Summary

This research analyzes the feasibility of extending SottoASR from macOS-only to support Windows. The application is currently heavily macOS-optimized, leveraging Apple-specific technologies like CoreML, Apple Neural Engine (ANE), and CGEvent APIs. This document identifies the macOS-specific components and evaluates cross-platform alternatives for each.

**Key Finding:** Windows support is achievable with moderate effort. The primary challenges are the paste-at-cursor functionality (which requires different APIs on Windows) and the ASR backend (FluidAudio is macOS-only). The parakeet-rs ONNX-based backend already supports Windows and can serve as the default for the Windows build.

---

## Current Architecture Analysis

### macOS-Specific Components

The following components currently have macOS-only implementations:

| Component | File | macOS Technology | Cross-Platform Alternative |
|-----------|------|------------------|---------------------------|
| Paste at cursor | `src-tauri/src/paste/macos.rs` | CGEvent (CoreGraphics) | enigo crate (SendInput) |
| ASR (default) | `src-tauri/src/asr/fluidaudio_backend.rs` | CoreML + Apple Neural Engine | parakeet-rs (ONNX) |
| Overlay window | `src-tauri/src/hotkeys/manager.rs` | NSPanel (tauri-nspanel) | Standard Tauri WebviewWindow |
| Accessibility check | `src-tauri/src/paste/macos.rs` | AXIsProcessTrusted() | Not required on Windows |
| Clipboard | `src-tauri/src/paste/macos.rs` | arboard (macOS-specific path) | Already cross-platform |

### Already Cross-Platform Components

| Component | Technology | Status |
|-----------|-----------|--------|
| Audio capture | cpal 0.15 | ✅ Works on Windows via WASAPI |
| Global hotkeys | tauri-plugin-global-shortcut | ✅ Supported on Windows |
| System tray | tauri tray-icon | ✅ Supported on Windows |
| Frontend | Svelte 5 + TypeScript | ✅ Platform agnostic |
| Tauri framework | v2 | ✅ Cross-platform |

---

## Detailed Analysis

### 1. Paste-at-Cursor Functionality

#### Current Implementation (macOS)

The current paste functionality uses CoreGraphics CGEvent APIs to:
1. Copy transcribed text to clipboard via arboard
2. Use CGEvent to simulate Cmd+V keyboard shortcut
3. Restore original clipboard contents after paste
4. Requires Accessibility permission (AXIsProcessTrusted)

This approach requires macOS-specific APIs and permissions.

#### Windows Alternative: enigo crate

The [enigo](https://github.com/enigo-rs/enigo) crate provides cross-platform keyboard and mouse simulation:

```rust
use enigo::{Enigo, Direction, Key, Keyboard};

fn paste_text(text: &str) -> Result<(), String> {
    // 1. Copy to clipboard (using arboard, cross-platform)
    let mut clipboard = arboard::Clipboard::new()
        .map_err(|e| format!("Clipboard error: {}", e))?;
    clipboard.set_text(text)
        .map_err(|e| format!("Set text error: {}", e))?;

    // 2. Simulate Ctrl+V using enigo (cross-platform)
    let mut enigo = Enigo::new()
        .map_err(|e| format!("Enigo error: {}", e))?;
    enigo.key(Key::Control, Direction::Press)
        .map_err(|e| format!("Key press error: {}", e))?;
    enigo.key(Key::Unicode('v'), Direction::Press)
        .map_err(|e| format!("Key press error: {}", e))?;
    enigo.key(Key::Unicode('v'), Direction::Release)
        .map_err(|e| format!("Key release error: {}", e))?;
    enigo.key(Key::Control, Direction::Release)
        .map_err(|e| format!("Key release error: {}", e))?;

    Ok(())
}
```

**Key Advantages of enigo:**
- Works on Windows without special permissions (uses SendInput API)
- No UAC elevation required
- No Accessibility permission needed
- Cross-platform: Windows, macOS, Linux, BSD
- 1,687 GitHub stars, well-maintained

**⚠️ Important Limitation - UIPI:**
This is a Windows API-level limitation (not specific to enigo). Windows UIPI (User Interface Privilege Isolation) prevents sending input to processes running at a higher integrity level. This means:
- ✅ Works with most desktop apps (VS Code, Chrome, Notepad, browsers, editors)
- ❌ Cannot send input to elevated/admin applications (running as Administrator)
- ❌ Cannot send input to system processes (Task Manager, etc.)

This is generally acceptable for a speech-to-text app that pastes into user applications. The app runs at medium integrity level, and most user apps do too, so input will work in 99% of cases.

**Windows API Integration:**

For getting the foreground window PID on Windows, use the `windows` crate:

```rust
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
use windows::Win32::Foundation::CloseHandle;

fn get_foreground_pid() -> u32 {
    unsafe {
        let hwnd = GetForegroundWindow();
        let mut process_id: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut process_id));
        process_id
    }
}
```

#### Comparison

| Aspect | macOS (CGEvent) | Windows (enigo) |
|--------|-----------------|-----------------|
| Permission required | Accessibility (TCC) | None |
| API level | CoreGraphics | SendInput (user-mode) |
| Reliability | High | High |
| Elevation needed | No | No |
| Code complexity | Medium | Low |

---

### 2. ASR Backend

#### Current Implementation (macOS)

The default ASR backend uses FluidAudio, which leverages:
- CoreML for model inference
- Apple Neural Engine (ANE) for hardware acceleration
- Performance: ~190x Real-Time Factor (RTF)

This is macOS-only and cannot be ported to Windows.

#### Windows Alternative: parakeet-rs

The [parakeet-rs](https://github.com/altunenes/parakeet-rs) crate provides ONNX-based speech recognition:

- Uses NVIDIA Parakeet TDT 0.6B model
- Runs on CPU via ONNX Runtime (default)
- Performance: ~20-30x RTF on CPU (slower than macOS ANE but acceptable)
- Already available as `asr-parakeet` feature flag
- **Windows GPU Acceleration:** Supports DirectML (Windows GPU) and CUDA/TensorRT (NVIDIA) execution providers via ONNX Runtime. This enables GPU acceleration on Windows without requiring NVIDIA hardware—useful for users with AMD GPUs or integrated graphics.

**Integration Status:**
- Already implemented in `src-tauri/src/asr/parakeet_backend.rs`
- Model download from HuggingFace works on Windows
- Cross-platform by design

#### Performance Expectations

| Platform | Backend | RTF | Hardware |
|----------|---------|-----|----------|
| macOS | FluidAudio (CoreML/ANE) | ~190x | Apple Silicon |
| Windows | parakeet-rs (ONNX/CPU) | Varies by CPU | Any modern CPU |

> ⚠️ **Note on RTF claims:** The exact CPU performance of parakeet-rs varies significantly by CPU model and configuration. GPU performance on Windows (via DirectML) can achieve much higher RTF. The 20-30x figure is a conservative estimate for mid-range CPUs; newer/ faster CPUs will perform better.

While Windows performance is slower than macOS ANE, reasonable CPU performance (10-30x RTF depending on hardware) means 1 minute of audio processes in 2-6 seconds—well within acceptable latency for a speech-to-text application.

#### Recommendation

For Windows builds, default to `asr-parakeet` feature. This is already supported via feature flags in Cargo.toml.

#### Alternative ASR Option: sherpa-onnx

For users who need multi-language support, [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) is an alternative worth considering:

| Aspect | parakeet-rs | sherpa-onnx |
|--------|-------------|-------------|
| Models | NVIDIA Parakeet | Whisper, SenseVoice, Paraformer |
| Languages | English-focused | 100+ languages |
| Performance | Fast (varies by CPU) | Varies by model and CPU |
| Rust crate | Native | Native |
| GPU support | DirectML, CUDA, TensorRT | DirectML, CUDA, TensorRT |

**Recommendation:** Keep parakeet-rs as default for speed and English optimization. Add sherpa-onnx as a future option for multi-language users.

---

### 3. Overlay Window

#### Current Implementation (macOS)

Uses `tauri-nspanel` to create an NSPanel-style floating window:
- Always on top (floats above other windows)
- No title bar
- Transparent background capable
- Doesn't appear in window list or Cmd+Tab

#### Windows Alternative: Standard Tauri WebviewWindow

Tauri v2 supports all required features natively:

```rust
// In src-tauri/src/hotkeys/manager.rs
WebviewWindowBuilder::new(
    &handle,
    "overlay",
    tauri::WebviewUrl::App("overlay.html".into()),
)
.title("")  // Empty title for borderless
.inner_size(300.0, 200.0)
.always_on_top(true)
.decorations(false)  // Frameless
.skip_taskbar(true)  // Don't show in taskbar
.focused(true)
.build()
```

**Configuration differences:**
- `skip_taskbar` replaces macOS NSPanel's "not in window list" behavior
- `always_on_top` is supported on both platforms
- `decorations: false` creates borderless window on both platforms

**⚠️ Known Tauri v2 Windows Issues:**

1. **skip_taskbar bug (Tauri #10422):** The `skip_taskbar` option does not reliably hide the window from the Windows taskbar. The window may still appear in the taskbar despite the setting. A workaround may require using Windows APIs directly to set `WS_EX_TOOLWINDOW` extended window style.

2. **decorations:false issues (Tauri #11345, #12042):** When `decorations: false` is set on Windows 11, there are reported issues with window repositioning and resizing. Custom titlebars may not function correctly.

3. **always_on_top edge cases:** Historical issues exist with window freezing when dragged while always on top on Windows 10. This appears partially resolved in newer Tauri versions.

**Recommendation:** Test the overlay window extensively on Windows. If `skip_taskbar` proves unreliable, consider alternative approaches such as using a smaller always-on-top window that doesn't have the taskbar issue, or implementing a native Windows overlay using additional platform-specific code.

---

### 4. Permissions Model

#### macOS Permissions

| Permission | Purpose | How to Request |
|------------|---------|----------------|
| Microphone | Audio capture | TCC prompt (handled by OS) |
| Accessibility | CGEvent posting | Manual in System Settings |

#### Windows Permissions

| Permission | Purpose | How to Request |
|------------|---------|----------------|
| Microphone | Audio capture | UAC prompt on first use |
| None | Keyboard simulation | Not required (SendInput is user-mode) |

**Key Finding:** Windows does not require special permissions for keyboard simulation. The `SendInput` API operates at the user level and does not require UIAccess or elevation.

⚠️ **Important Clarification:** While SendInput does NOT require UIAccess or UAC elevation, it IS subject to UIPI (User Interface Privilege Isolation). This means:
- Cannot send input to processes at higher integrity levels
- Cannot send input to certain protected system processes
- Works normally with applications running at the same integrity level (medium)

This is generally not an issue for SottoASR since both the app and target applications (browsers, editors, etc.) typically run at medium integrity level.

This is a significant advantage over macOS where Accessibility permission is required and can be problematic for users.

---

### 5. Windows-Specific Considerations

#### File System Paths

Windows uses different directory structures than macOS. Use the `directories` crate for cross-platform paths:

| Purpose | macOS Path | Windows Path |
|---------|------------|---------------|
| App config | `~/Library/Application Support/SottoASR` | `%APPDATA%\SottoASR` |
| Logs | `~/Library/Logs/SottoASR` | `%LOCALAPPDATA%\SottoASR\logs` |
| Model cache | `~/Library/Caches/SottoASR/models` | `%LOCALAPPDATA%\SottoASR\models` |

The `dirs` crate (already a dependency) provides `dirs::data_dir()`, `dirs::cache_dir()`, etc., which handle platform-appropriate paths automatically.

#### Windows Microphone Permission Model

Windows 10/11 has a dedicated microphone privacy setting:
- Users can enable/disable microphone access per-app in **Settings > Privacy & Security > Microphone**
- Unlike macOS TCC, there's no reliable API to check permission status programmatically
- If microphone access is denied, WASAPI simply fails to activate the audio stream

**Error handling:** Handle `ERROR_ACCESS_DENIED` or `AUDCLNT_E_DEVICE_INVALIDATED` errors gracefully and guide users to Windows Settings.

#### Clipboard Quirks on Windows

While arboard is cross-platform, Windows has specific behaviors:
- **Clipboard locking:** Other applications can hold clipboard locks, causing operations to fail
- **Timing sensitivity:** Windows clipboard operations may need brief delays

**Recommendation:** Add retry logic for clipboard operations and small delays around Ctrl+V simulation.

#### High DPI / Multi-Monitor Issues

⚠️ Known Tauri Windows issues:
- Issue #10263: Window cannot switch correctly between monitors with different scaling
- Issue #12043: Awkward window dragging between DPI-scaled monitors

The overlay window is particularly affected since it's a small floating window that users might drag between monitors.

**Mitigation:** 
- Set `resizable(false)` for the overlay to avoid sizing issues
- Test extensively with multiple monitors at different DPI scales

---

### 6. Security Considerations

#### Clipboard Security

⚠️ **Important Security Notes:**

1. **Clipboard data exposure:** The app writes transcribed text to the system clipboard. Malware with clipboard access could potentially observe clipboard contents. This is a general Windows limitation, not specific to SottoASR.

2. **Sensitive data handling:** The current implementation:
   - Copies transcribed text to clipboard
   - Sends Ctrl+V to paste into target app
   - Restores original clipboard contents after ~500ms
   - This behavior should be documented for security-conscious users

3. **Antimalware false positives:** Keyboard simulation (enigo/SendInput) may trigger false positives in some antivirus/EDR software, as it's functionally similar to keylogging. Document this for users who encounter alerts.

#### ASR Model Download Security

- **Model integrity:** Verify model checksums if available from HuggingFace
- **HTTPS only:** Ensure all model downloads use HTTPS to prevent MITM attacks
- **Version pinning:** Document specific model versions used for reproducibility

---

### 7. System Integration

#### Menu Bar / System Tray

Tauri v2's `tray-icon` plugin works on Windows:
- Creates system tray icon
- Right-click menu supported
- Click handling works

**Difference:** On Windows, the tray icon appears in the system tray (bottom-right). On macOS, it appears in the menu bar (top-right). This is an OS convention difference that cannot be changed.

#### Global Hotkeys

`tauri-plugin-global-shortcut` v2.3.1 supports Windows:
- Register global shortcuts
- Works when app is in background
- No special permissions needed

#### Auto-start

`tauri-plugin-autostart` v2.5.1 supports Windows:
- Adds to Windows startup (Registry: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`)
- User-configurable

---

### 8. Build and Distribution

#### Code Signing

| Platform | Requirement | Tool |
|----------|-------------|------|
| macOS | Developer ID + notarization | tauri-plugin-tauri-signing |
| Windows | Code signing certificate | SignTool or Azure SignTool |

**Windows SmartScreen:**
- Unsigned apps trigger SmartScreen warning
- Code signing required for production distribution
- EV certificates prevent SmartScreen warnings immediately
- OV certificates work after building reputation

**Recommendation:** Obtain a code signing certificate for Windows releases. GitHub Actions can automate signing with Azure SignTool.

#### Installer Format

| Platform | Format | Tool |
|----------|--------|------|
| macOS | .dmg | Tauri default |
| Windows | .msi or .exe (NSIS) | Tauri bundler |

Tauri v2 supports both MSI and NSIS installers for Windows via `bundle.targets` configuration.

---

## Implementation Roadmap

### Phase 1: Core Infrastructure (Estimated: 2-3 days)

1. **Add Windows dependencies:**
   ```toml
   # Cargo.toml
   [target.'cfg(windows)'.dependencies]
   enigo = "0.6"
   windows = { version = "0.58", features = ["Win32_UI_WindowsAndMessaging", "Win32_Foundation"] }
   ```

2. **Create Windows paste implementation:**
   - Create `src-tauri/src/paste/windows.rs`
   - Implement using enigo for keyboard simulation
   - Use Windows API for foreground window detection

3. **Conditionally compile paste module:**
   ```rust
   // src-tauri/src/paste/mod.rs
   #[cfg(target_os = "macos")]
   mod macos;
   
   #[cfg(target_os = "windows")]
   mod windows;
   ```

### Phase 2: ASR Backend (Estimated: 1 day)

1. **Configure Windows build to use parakeet-rs:**
   ```toml
   # Cross-platform feature configuration
   [target.'cfg(windows)'.features]
   default = ["custom-protocol", "asr-parakeet"]
   
   [target.'cfg(target_os = "macos")'.features]
   default = ["custom-protocol", "asr-fluidaudio"]
   ```

2. **Ensure model download works on Windows** (already verified - uses HTTP)

### Phase 3: UI Adjustments (Estimated: 1-2 days)

1. **Update overlay window creation:**
   - Use standard WebviewWindowBuilder for both platforms
   - Conditionally use NSPanel only on macOS

2. **Handle platform-specific settings:**
   - Keyboard shortcut display (Cmd vs Ctrl)
   - Menu bar vs system tray location

### Phase 4: Testing and Polish (Estimated: 2-3 days)

1. **Windows testing:**
   - Audio capture via WASAPI
   - Paste functionality reliability
   - Global hotkey registration
   - System tray behavior

2. **Comprehensive test scenarios:**

   **Paste functionality:**
   - Test with plain text editors (Notepad)
   - Test with code editors (VS Code)
   - Test with browsers (Chrome, Firefox, Edge)
   - Test with terminals (PowerShell, cmd)
   - Test with rich text editors (Word, Outlook)
   - Test with IDEs (Visual Studio)

   **Edge cases:**
   - Test paste into elevated (admin) applications — verify graceful failure
   - Test with clipboard locked by another app
   - Test with target app losing focus mid-operation
   - Test with IMEs enabled (international keyboards)
   - Test full-screen applications

   **Window/UI:**
   - Test overlay on multiple monitors with different DPI
   - Test overlay appears in taskbar (should not)
   - Test system tray icon behavior

   **Security:**
   - Test antimalware/EDR false positives for keyboard simulation
   - Verify clipboard restoration after paste

3. **Build configuration:**
   - Windows-specific tauri.conf.json settings
   - Code signing setup

---

## Risk Assessment

| Risk | Severity | Mitigation |
|------|----------|------------|
| Paste fails on some apps | Medium | Test with common apps (VS Code, Chrome, Notepad); note UIPI restrictions |
| Paste fails on elevated apps | Medium | Document limitation; users must run target apps without elevation |
| Paste fails due to clipboard lock | Medium | Add retry logic with delays |
| Performance slower on Windows | Low | User expectation management |
| Audio device enumeration issues | Medium | Add device selection UI |
| Microphone permission denied | Medium | Handle gracefully; guide users to Windows Settings |
| Global hotkey conflicts | Low | Allow users to customize |
| Code signing cost | Medium | Use OV certificate initially |
| Overlay appears in taskbar | High | Test extensively; implement Windows API workaround if needed |
| Window decorations issues | Medium | Test overlay window extensively on Windows 11 |
| Multi-monitor DPI issues | Medium | Test with multiple monitors; set resizable(false) |

---

## Open Questions

1. **Should we support Windows on Apple Silicon (Mac with Parallels/Boot Camp)?**
   - This would require x86_64 Windows build
   - Not recommended for v1.0

2. **Should we offer FluidAudio on Windows if hardware allows?**
   - FluidAudio uses CoreML which doesn't work on Windows
   - Not feasible without major rewrites

3. **How to handle microphone permission UI on Windows?**
   - Windows doesn't have a unified permissions UI like macOS
   - Rely on Windows UAC prompts

---

## Appendix: Reference Implementations

### enigo Keyboard Simulation

```rust
use enigo::{Enigo, Direction, Key, Keyboard, Settings};

fn simulate_ctrl_v(enigo: &mut Enigo) -> Result<(), enigo::EnigoError> {
    // Press and hold Ctrl
    enigo.key(Key::Control, Direction::Press)?;
    
    // Press and release V
    enigo.key(Key::Unicode('v'), Direction::Press)?;
    enigo.key(Key::Unicode('v'), Direction::Release)?;
    
    // Release Ctrl
    enigo.key(Key::Control, Direction::Release)?;
    
    Ok(())
}
```

### Windows Foreground Window

```rust
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
use windows::Win32::Foundation::CloseHandle;

fn get_foreground_process_id() -> u32 {
    unsafe {
        let hwnd = GetForegroundWindow();
        let mut process_id: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut process_id));
        process_id
    }
}
```

### Tauri Window Configuration

```rust
// Cross-platform overlay window
WebviewWindowBuilder::new(
    &handle,
    "overlay",
    tauri::WebviewUrl::App("overlay.html".into()),
)
.title("SottoASR")
.inner_size(280.0, 180.0)
.resizable(false)
.always_on_top(true)
#[cfg(target_os = "windows")]
.skip_taskbar(true)
#[cfg(target_os = "macos")]
.decorations(false)
.focused(true)
.build()
```

---

## References

- [enigo crate](https://crates.io/crates/enigo) - Cross-platform input simulation
- [parakeet-rs](https://github.com/altunenes/parakeet-rs) - ONNX-based ASR
- [Tauri v2 system tray](https://v2.tauri.app/learn/system-tray/) - Cross-platform tray
- [Tauri v2 global-shortcut](https://v2.tauri.app/plugin/global-shortcut/) - Cross-platform hotkeys
- [Windows SendInput API](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput) - Keyboard simulation
- [Tauri Windows code signing](https://v2.tauri.app/distribute/sign/windows/) - Distribution guide