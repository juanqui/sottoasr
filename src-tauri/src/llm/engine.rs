use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic job ID for stale-result prevention.
static NEXT_JOB_ID: AtomicU64 = AtomicU64::new(1);

/// Get a new unique job ID.
pub fn next_job_id() -> u64 {
    NEXT_JOB_ID.fetch_add(1, Ordering::SeqCst)
}

/// The LLM engine manages a Python sidecar process for transcript cleanup.
pub struct LlmEngine {
    child: Child,
    stdin: std::io::BufWriter<std::process::ChildStdin>,
    stdout: BufReader<std::process::ChildStdout>,
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
            .stderr(Stdio::inherit()) // sidecar logs go to app stderr
            .spawn()
            .map_err(|e| format!("Failed to spawn LLM sidecar: {}", e))?;

        let stdin = child.stdin.take()
            .ok_or("Failed to open sidecar stdin")?;
        let stdout = child.stdout.take()
            .ok_or("Failed to open sidecar stdout")?;

        Ok(Self {
            child,
            stdin: std::io::BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
        })
    }

    /// Send a raw request and read a response (blocking). Public for direct protocol access.
    pub fn request_raw(&mut self, req: &serde_json::Value) -> Result<serde_json::Value, String> {
        self.request(req)
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

    /// Clean up a transcript.
    pub fn cleanup(&mut self, text: &str) -> Result<String, String> {
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

/// Check if the app's venv exists and has mlx-lm installed.
pub fn is_venv_ready() -> bool {
    venv_python()
        .map(|p| p.exists())
        .unwrap_or(false)
}

/// Check if the model weights are available in the HuggingFace cache.
pub fn is_model_downloaded() -> bool {
    let cache_dir = match dirs::home_dir() {
        Some(h) => h.join(".cache/huggingface/hub"),
        None => return false,
    };
    let cache_name = format!("models--{}", SOTTO_MODEL.id.replace('/', "--"));
    cache_dir.join(cache_name).join("snapshots").is_dir()
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

    log::info!("LLM venv setup complete");
    Ok(())
}

/// Check if the feature was compiled in.
pub fn is_feature_compiled() -> bool {
    cfg!(feature = "llm-cleanup")
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
