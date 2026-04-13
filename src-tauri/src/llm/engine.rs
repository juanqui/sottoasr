use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI8, AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use crate::state::AppState;

/// Monotonic job ID for stale-result prevention.
static NEXT_JOB_ID: AtomicU64 = AtomicU64::new(1);

/// Get a new unique job ID.
pub fn next_job_id() -> u64 {
    NEXT_JOB_ID.fetch_add(1, Ordering::SeqCst)
}

/// Trait for LLM transcript cleanup backends.
/// Production: Python sidecar via stdin/stdout JSON protocol.
/// Tests: returns canned or transformed text.
pub trait LlmBackend: Send {
    /// Clean up a raw transcript.
    /// Returns the cleaned text, or an error.
    fn cleanup(&mut self, text: &str) -> Result<String, String>;

    /// Send a raw JSON request and return the raw JSON response.
    /// Used by `commands/llm.rs` for protocol-level operations like
    /// `check_update` that bypass the typed `cleanup()` API.
    fn request_raw(&mut self, req: &serde_json::Value) -> Result<serde_json::Value, String>;

    /// Shut down the backend. Default is a no-op.
    /// Production impl kills the sidecar process.
    fn shutdown(&mut self) {}
}

/// The LLM engine manages a Python sidecar process for transcript cleanup.
pub struct LlmEngine {
    child: Child,
    stdin: std::io::BufWriter<std::process::ChildStdin>,
    stdout: BufReader<std::process::ChildStdout>,
    /// PID of the spawned Python sidecar subprocess. Captured once at spawn
    /// time so callers can SIGKILL the process by PID without holding a
    /// mutable reference to `child`. See `kill_orphan()`.
    pid: u32,
}

// We manage the sidecar as a single-owner resource behind TokioMutex.
unsafe impl Send for LlmEngine {}

impl LlmEngine {
    /// Spawn the Python sidecar process.
    pub fn spawn() -> Result<Self, String> {
        // Ensure venv exists
        if !is_venv_ready() {
            log::info!("LLM venv not found, setting up...");
            setup_venv()?;
        }

        let python = venv_python()?;
        let sidecar_path = Self::sidecar_script_path()?;
        log::info!("Spawning LLM sidecar: {} {}", python.display(), sidecar_path.display());

        let mut child = Command::new(&python)
            .arg(&sidecar_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to spawn LLM sidecar: {}", e))?;

        let pid = child.id();
        let stdin = child.stdin.take()
            .ok_or("Failed to open sidecar stdin")?;
        let stdout = child.stdout.take()
            .ok_or("Failed to open sidecar stdout")?;
        let stderr = child.stderr.take()
            .ok_or("Failed to open sidecar stderr")?;

        // Forward sidecar stderr line-by-line into the Rust log so Python
        // exceptions and `[llm_cleanup]` log lines land in SottoASR.log. The
        // reader thread exits when the child closes stderr (which happens on
        // process exit).
        thread::Builder::new()
            .name(format!("llm-sidecar-stderr-{}", pid))
            .spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines() {
                    match line {
                        Ok(l) if !l.is_empty() => {
                            log::warn!("[llm-sidecar] {}", l);
                        }
                        Ok(_) => {}
                        Err(_) => break,
                    }
                }
            })
            .map_err(|e| format!("Failed to spawn sidecar stderr reader: {}", e))?;

        Ok(Self {
            child,
            stdin: std::io::BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
            pid,
        })
    }

    /// PID of the spawned Python subprocess. Captured at spawn time.
    pub fn child_pid(&self) -> u32 {
        self.pid
    }

    /// Send a request and read a response (blocking).
    fn request(&mut self, req: &serde_json::Value) -> Result<serde_json::Value, String> {
        let mut line = serde_json::to_string(req)
            .map_err(|e| format!("JSON serialize failed: {}", e))?;
        line.push('\n');

        self.stdin.write_all(line.as_bytes())
            .map_err(|e| format!("Failed to write to sidecar: {}", e))?;
        self.stdin.flush()
            .map_err(|e| format!("Failed to flush sidecar stdin: {}", e))?;

        let mut response_line = String::new();
        self.stdout.read_line(&mut response_line)
            .map_err(|e| format!("Failed to read from sidecar: {}", e))?;

        if response_line.is_empty() {
            return Err("Sidecar closed stdout (process may have crashed)".into());
        }

        serde_json::from_str(&response_line)
            .map_err(|e| format!("Failed to parse sidecar response: {} (raw: {:?})", e, response_line))
    }

    /// Get model status from the sidecar.
    #[allow(dead_code)]
    pub fn status(&mut self) -> Result<serde_json::Value, String> {
        self.request(&serde_json::json!({"action": "status"}))
    }

    /// Tell the sidecar to download the model.
    pub fn download_model(&mut self) -> Result<(), String> {
        let resp = self.request(&serde_json::json!({"action": "download"}))?;
        if resp.get("ok").and_then(|v| v.as_bool()) == Some(true) {
            Ok(())
        } else {
            Err(resp.get("error").and_then(|v| v.as_str()).unwrap_or("Download failed").into())
        }
    }

    /// Tell the sidecar to load the model into memory.
    pub fn load_model(&mut self) -> Result<(), String> {
        let resp = self.request(&serde_json::json!({"action": "load"}))?;
        if resp.get("ok").and_then(|v| v.as_bool()) == Some(true) {
            Ok(())
        } else {
            Err(resp.get("error").and_then(|v| v.as_str()).unwrap_or("Load failed").into())
        }
    }

    /// Shut down the sidecar.
    pub fn quit(&mut self) {
        let _ = self.request(&serde_json::json!({"action": "quit"}));

        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(3);
        let sleep_interval = std::time::Duration::from_millis(100);

        loop {
            match self.child.try_wait() {
                Ok(Some(_status)) => break,
                Ok(None) => {
                    if start.elapsed() >= timeout {
                        let _ = self.child.kill();
                        let _ = self.child.wait();
                        break;
                    }
                    std::thread::sleep(sleep_interval);
                }
                Err(_e) => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
            }
        }
    }

    /// Find the sidecar script path.
    fn sidecar_script_path() -> Result<std::path::PathBuf, String> {
        // In development: relative to the src-tauri directory
        let dev_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("sidecar")
            .join("llm_cleanup.py");
        if dev_path.exists() {
            return Ok(dev_path);
        }

        // In production: bundled with the .app
        if let Ok(exe) = std::env::current_exe() {
            let app_dir = exe.parent().unwrap_or(std::path::Path::new("."));
            let bundled = app_dir.join("../Resources/sidecar/llm_cleanup.py");
            if bundled.exists() {
                return Ok(bundled);
            }
        }

        Err("LLM sidecar script not found".into())
    }
}

impl LlmBackend for LlmEngine {
    fn cleanup(&mut self, text: &str) -> Result<String, String> {
        let resp = self.request(&serde_json::json!({
            "action": "cleanup",
            "text": text,
        }))?;

        if resp.get("ok").and_then(|v| v.as_bool()) == Some(true) {
            let cleaned = resp.get("text")
                .and_then(|v| v.as_str())
                .unwrap_or(text)
                .to_string();
            if let Some(ms) = resp.get("elapsed_ms").and_then(|v| v.as_u64()) {
                log::info!("LLM cleanup completed in {}ms", ms);
            }
            Ok(cleaned)
        } else {
            let error = resp.get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown error");
            Err(error.to_string())
        }
    }

    fn request_raw(&mut self, req: &serde_json::Value) -> Result<serde_json::Value, String> {
        self.request(req)
    }

    fn shutdown(&mut self) {
        self.quit();
    }
}

impl Drop for LlmEngine {
    fn drop(&mut self) {
        self.quit();
    }
}

/// Check if this platform supports the LLM feature (Apple Silicon + Python 3).
pub fn is_platform_supported() -> bool {
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return false;
    }
    Command::new("python3")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Get the path to the app-managed Python venv for mlx-lm.
pub fn venv_dir() -> Result<std::path::PathBuf, String> {
    let data_dir = dirs::data_dir().ok_or("Could not determine data directory")?;
    Ok(data_dir.join("com.sottoasr.app").join("llm-venv"))
}

/// Get the Python executable inside the app's venv.
pub fn venv_python() -> Result<std::path::PathBuf, String> {
    Ok(venv_dir()?.join("bin").join("python3"))
}

/// Cached result of `is_venv_ready()`:
/// `0` = not yet checked, `1` = ready, `-1` = broken.
/// Reset to `0` in `setup_venv()` and `reset_venv_cache()` so a repair can be detected.
static VENV_READY_CACHE: AtomicI8 = AtomicI8::new(0);

/// Invalidate the cached venv readiness result. Call after any operation that
/// repairs or recreates the venv.
pub fn reset_venv_cache() {
    VENV_READY_CACHE.store(0, Ordering::SeqCst);
}

/// Check if the app's venv exists AND has a working `mlx_lm` install.
///
/// The cheap existence check (`bin/python3` file present) used to be the only
/// probe, but that masked a common failure mode: the venv's `python3` is a
/// symlink to a system Python that has since been upgraded or removed, which
/// makes mlx_lm imports blow up at runtime. Here we actually exec the venv's
/// Python with `import mlx_lm` and cache the result so we don't re-pay the
/// ~500ms import cost on every call.
pub fn is_venv_ready() -> bool {
    match VENV_READY_CACHE.load(Ordering::SeqCst) {
        1 => return true,
        -1 => return false,
        _ => {}
    }

    let python = match venv_python() {
        Ok(p) => p,
        Err(_) => {
            VENV_READY_CACHE.store(-1, Ordering::SeqCst);
            return false;
        }
    };
    if !python.exists() {
        VENV_READY_CACHE.store(-1, Ordering::SeqCst);
        return false;
    }

    let ok = Command::new(&python)
        .args(["-c", "import mlx_lm; import huggingface_hub"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map(|out| {
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                log::warn!(
                    "Venv check: `import mlx_lm` failed ({}): {}",
                    out.status,
                    stderr.trim()
                );
            }
            out.status.success()
        })
        .unwrap_or_else(|e| {
            log::warn!("Venv check: could not exec {}: {}", python.display(), e);
            false
        });

    VENV_READY_CACHE.store(if ok { 1 } else { -1 }, Ordering::SeqCst);
    ok
}

/// Check if the model weights are actually present in the HuggingFace cache.
///
/// The old check only asserted that `snapshots/` was a directory, which is
/// true even after an interrupted `snapshot_download` that left zero weight
/// files behind. We now require at least one `.safetensors` file somewhere
/// under `snapshots/*` before declaring the model downloaded.
pub fn is_model_downloaded() -> bool {
    let cache_dir = match dirs::home_dir() {
        Some(h) => h.join(".cache/huggingface/hub"),
        None => return false,
    };
    let cache_name = format!("models--{}", SOTTO_MODEL.id.replace('/', "--"));
    let snapshots = cache_dir.join(cache_name).join("snapshots");
    let Ok(entries) = std::fs::read_dir(&snapshots) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&path) else {
            continue;
        };
        for f in files.flatten() {
            if f.path().extension().and_then(|e| e.to_str()) == Some("safetensors") {
                return true;
            }
        }
    }
    false
}

/// Create the venv and install mlx-lm. This is a blocking operation (~30-60s).
pub fn setup_venv() -> Result<(), String> {
    let venv = venv_dir()?;
    log::info!("Creating LLM Python venv at {:?}...", venv);

    let status = Command::new("python3")
        .args(["-m", "venv", &venv.to_string_lossy()])
        .status()
        .map_err(|e| format!("Failed to create venv: {}", e))?;
    if !status.success() {
        return Err("python3 -m venv failed".into());
    }

    let python = venv.join("bin").join("python3");

    log::info!("Upgrading pip in venv...");
    let _ = Command::new(&python)
        .args(["-m", "pip", "install", "--upgrade", "pip"])
        .output();

    log::info!("Installing mlx-lm and huggingface_hub into venv...");
    let output = Command::new(&python)
        .args(["-m", "pip", "install", "--upgrade", "mlx-lm", "huggingface_hub"])
        .output()
        .map_err(|e| format!("Failed to run pip: {}", e))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("pip install failed: {}", stderr));
    }

    reset_venv_cache();
    log::info!("LLM venv setup complete");
    Ok(())
}

/// Check if the feature was compiled in.
pub fn is_feature_compiled() -> bool {
    cfg!(feature = "llm-cleanup")
}

/// How long `ensure_running()` waits between spawn attempts.
const SPAWN_RETRY_DELAY_MS: u64 = 500;
/// Number of spawn attempts before giving up.
const SPAWN_MAX_ATTEMPTS: u32 = 2;

/// Heuristic: is this cleanup error string one that indicates the underlying
/// Python subprocess has died (broken pipe / EOF on stdout)? These errors
/// come from `LlmEngine::request()` when writing to or reading from a dead
/// child process. A zombie handle cannot be recovered — it must be dropped.
pub fn is_zombie_error(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("broken pipe")
        || e.contains("closed stdout")
        || e.contains("epipe")
        || e.contains("sidecar closed")
        || e.contains("crashed")
}

/// Ensure a live sidecar handle is available, spawning + loading the model if
/// none is currently running. Returns the handle (ownership transferred to the
/// caller — remember to put it back in `state.llm_engine` after use).
///
/// Retries once with a short backoff on persistent failures. On success,
/// stores the subprocess PID in `state.llm_pid` for `kill_orphan()` use.
/// See docs/specs/2026-04-11-llm-cleanup-reliability.md §4.1.
pub async fn ensure_running(state: &AppState) -> Result<Box<dyn LlmBackend>, String> {
    // Fast path — sidecar already running in the guard.
    {
        let mut guard = state.llm_engine.lock().await;
        if let Some(llm) = guard.take() {
            return Ok(llm);
        }
    }

    // Slow path — spawn + load, with retries.
    let mut last_err = String::new();
    for attempt in 0..SPAWN_MAX_ATTEMPTS {
        log::info!("Spawning LLM sidecar (attempt {}/{})...", attempt + 1, SPAWN_MAX_ATTEMPTS);
        let spawn = tokio::task::spawn_blocking(|| {
            let mut e = LlmEngine::spawn()?;
            e.load_model()?;
            Ok::<_, String>(e)
        }).await;

        match spawn {
            Ok(Ok(engine)) => {
                state.llm_pid.store(engine.child_pid() as i32, Ordering::SeqCst);
                log::info!("LLM sidecar ready (pid={})", engine.child_pid());
                return Ok(Box::new(engine) as Box<dyn LlmBackend>);
            }
            Ok(Err(e)) => { last_err = e; }
            Err(e) => { last_err = format!("spawn task panic: {}", e); }
        }

        if attempt + 1 < SPAWN_MAX_ATTEMPTS {
            tokio::time::sleep(Duration::from_millis(SPAWN_RETRY_DELAY_MS)).await;
        }
    }

    state.llm_pid.store(0, Ordering::SeqCst);
    log::warn!("LLM sidecar could not be started after {} attempts: {}", SPAWN_MAX_ATTEMPTS, last_err);
    Err(last_err)
}

/// Kill any orphaned Python sidecar subprocess whose PID is cached in
/// `state.llm_pid`. Used when a cleanup task panics or times out — the
/// blocking task still owns the `Child` handle but the subprocess is
/// holding Metal memory. SIGKILL by PID releases it immediately.
///
/// Clears `state.llm_pid` to 0 as a side effect so a recycled PID on the
/// next spawn cannot be accidentally killed by a stale call.
/// See docs/specs/2026-04-11-llm-cleanup-reliability.md §4.3.
pub fn kill_orphan(state: &AppState) {
    let pid = state.llm_pid.swap(0, Ordering::SeqCst);
    if pid <= 0 {
        return;
    }
    #[cfg(unix)]
    {
        // SAFETY: libc::kill is a POSIX syscall, safe to call with any i32 pid.
        // We only kill PIDs we spawned ourselves and we clear the cache first
        // to prevent double-kills on recycled PIDs.
        let rc = unsafe { libc::kill(pid, libc::SIGKILL) };
        if rc == 0 {
            log::warn!("SIGKILL sent to orphaned LLM sidecar (pid={})", pid);
        } else {
            // ESRCH = process already gone, not an error in our context
            let errno = std::io::Error::last_os_error();
            log::debug!("kill(pid={}) returned {} ({})", pid, rc, errno);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        log::debug!("kill_orphan: unsupported platform, no-op");
    }
}

/// The single model configuration for SottoASR transcript cleanup.
pub struct ModelConfig {
    pub id: &'static str,
    pub display_name: &'static str,
    pub download_size_mb: u64,
}

pub const SOTTO_MODEL: ModelConfig = ModelConfig {
    id: "juanquivilla/sotto-cleanup-lfm25-350m-mlx-5bit",
    display_name: "SottoASR Cleanup",
    download_size_mb: 233,
};

/// Get the model configuration.
pub fn model_config() -> &'static ModelConfig {
    &SOTTO_MODEL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_job_id_monotonic() {
        let id1 = next_job_id();
        let id2 = next_job_id();
        assert!(id2 > id1, "Job IDs must be monotonically increasing: {} should be > {}", id2, id1);
    }

    #[test]
    fn next_job_id_never_zero() {
        let id = next_job_id();
        assert!(id > 0, "Job ID should never be 0, got {}", id);
    }

    #[test]
    fn sotto_model_id_is_correct() {
        assert_eq!(SOTTO_MODEL.id, "juanquivilla/sotto-cleanup-lfm25-350m-mlx-5bit");
    }

    #[test]
    fn sotto_model_display_name() {
        assert_eq!(SOTTO_MODEL.display_name, "SottoASR Cleanup");
    }

    #[test]
    fn sotto_model_download_size() {
        assert_eq!(SOTTO_MODEL.download_size_mb, 233);
    }

    #[test]
    fn model_config_returns_sotto_model() {
        let config = model_config();
        assert_eq!(config.id, SOTTO_MODEL.id);
        assert_eq!(config.display_name, SOTTO_MODEL.display_name);
        assert_eq!(config.download_size_mb, SOTTO_MODEL.download_size_mb);
    }

    #[test]
    fn is_feature_compiled_returns_expected() {
        let compiled = is_feature_compiled();
        assert_eq!(compiled, cfg!(feature = "llm-cleanup"));
    }

    #[test]
    fn is_zombie_error_detects_broken_pipe() {
        assert!(is_zombie_error("Failed to write to sidecar: Broken pipe"));
        assert!(is_zombie_error("broken pipe (os error 32)"));
    }

    #[test]
    fn is_zombie_error_detects_closed_stdout() {
        assert!(is_zombie_error("Sidecar closed stdout (process may have crashed)"));
    }

    #[test]
    fn is_zombie_error_detects_epipe() {
        assert!(is_zombie_error("write returned EPIPE"));
    }

    #[test]
    fn is_zombie_error_ignores_non_zombie_messages() {
        assert!(!is_zombie_error("Model not loaded"));
        assert!(!is_zombie_error("Failed to load model: OOM"));
        assert!(!is_zombie_error("Invalid JSON response"));
    }
}
