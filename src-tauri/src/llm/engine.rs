use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI32, AtomicI8, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use crate::process::{bounded_command, OwnedChild};
use crate::state::AppState;

/// Monotonic job ID for stale-result prevention.
static NEXT_JOB_ID: AtomicU64 = AtomicU64::new(1);

/// Get a new unique job ID.
pub fn next_job_id() -> u64 {
    NEXT_JOB_ID.fetch_add(1, Ordering::SeqCst)
}

/// One cleaned-text proposal returned by a batch, typed per item.
/// `Proposal(Some(""))` is a VALID reply (a fully-erased segment, spec E1) —
/// success is never tested by truthiness; `Proposal(None)` means the item
/// answered ok without a usable text and is treated as a per-item failure.
/// `TimedOut`/`Failed` keep their region's raw text but say nothing about the
/// endpoint's health: a valid response carrying these RETAINS the handle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BatchItem {
    Proposal(Option<String>),
    TimedOut,
    Failed(String),
}

/// Largest batch the endpoint accepts and the client's chunk size (spec §2).
pub const BATCH_CAP: usize = 16;

/// One item's raw-input budget in bytes (the planner's window cap, enforced
/// server-side per item as new work — spec §4.5). Distinct from the output
/// cap: cleanup never returns more bytes than the source carried (the
/// source-side bound lives in `validation::MAX_CLEANUP_BYTES` — spec: there
/// is no separate MAX_TEXT_BYTES in Rust).
pub const MAX_ITEM_BYTES: usize = 4_096;

/// Batch framing (spec §4.5): chosen so the caps statically dominate every
/// valid planned batch — request ≤ 16 × (4 096 B raw × 6 JSON-escape +
/// field overhead) ≈ 0.4 MB; response ≤ 16 × (32 000 B raw × 6 + metadata)
/// ≈ 3.1 MB. Readers guard at limit+1 bytes; no eager cap-sized allocation.
/// Mirrored in sidecar/llm_cleanup.py.
pub const MAX_REQUEST_LINE_BYTES: u64 = 1024 * 1024;
pub const MAX_RESPONSE_LINE_BYTES: u64 = 4 * 1024 * 1024;

/// Trait for LLM transcript cleanup backends.
/// Production: Python sidecar via stdin/stdout JSON protocol.
/// Tests: return untrusted batched proposals; the source validator
/// authorizes reconstruction separately.
pub trait LlmBackend: Send {
    fn is_alive(&mut self) -> bool { true }
    /// Head-batch cleanup: one request for 1..=16 texts, one typed
    /// [`BatchItem`] per input. REQUIRED — no default: a default looping
    /// a serial single-text call would be a prohibited compatibility shim,
    /// and no serial path exists anywhere after the cutover (spec §4.1).
    /// `Err(String)` is the whole-request protocol/transport channel
    /// (bad JSON, index-set mismatch, over-cap line, EOF, pipe death): the
    /// caller MUST see these to retire the handle. Per-item
    /// `BatchOutcome`-style failures are `Ok` entries, never `Err`.
    fn cleanup_batch(&mut self, texts: &[String]) -> Result<Vec<BatchItem>, String>;

    /// Send a raw JSON request and return the raw JSON response.
    /// Used by `commands/llm.rs` for protocol-level operations like
    /// `check_update` that bypass the typed batch API.
    fn request_raw(&mut self, req: &serde_json::Value) -> Result<serde_json::Value, String>;

    /// Shut down the backend. Default is a no-op.
    /// Production impl kills the sidecar process.
    fn shutdown(&mut self) {}
}

/// The LLM engine manages a Python sidecar process for transcript cleanup.
pub struct LlmEngine {
    child: OwnedChild,
    stdin: std::process::ChildStdin,
    responses: mpsc::Receiver<Result<String, String>>,
    registered_pid: Option<Arc<AtomicI32>>,
    /// Identifies the registered owned process during timeout recovery while a
    /// blocking worker still owns this engine. See `kill_orphan()`.
    pid: u32,
}

// We manage the sidecar as a single-owner resource behind TokioMutex.
// Child handles and the response receiver are Send; no unsafe implementation needed.

fn request_timeout(request: &serde_json::Value) -> Duration {
    Duration::from_secs(match request.get("action").and_then(|v| v.as_str()) {
        Some("download") => 900,
        Some("load") => 15,
        // CRITICAL (spec §4.1): the batch action MUST map here. An unmapped
        // action silently gets the 5 s default, which retires the sidecar
        // mid-batch — measured 6.6 s for a real production 16-batch.
        Some("cleanup_batch") => 15, // Ten-second generation budget + overhead.
        Some("check_update") => 10,
        _ => 5,
    })
}

/// Read responses on a dedicated blocking thread. A partial line cannot bypass
/// the caller's deadline, and a broken protocol cannot allocate unbounded text.
fn response_reader(
    stdout: std::process::ChildStdout,
) -> Result<mpsc::Receiver<Result<String, String>>, String> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("llm-sidecar-stdout".into())
        .spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut bytes = Vec::new();
                let response = match reader
                    .by_ref()
                    .take(MAX_RESPONSE_LINE_BYTES)
                    .read_until(b'\n', &mut bytes)
                {
                    Ok(0) => Err("Sidecar closed stdout (process may have crashed)".into()),
                    Ok(_) if bytes.last() != Some(&b'\n') => {
                        Err("Sidecar response exceeded protocol limit or ended early".into())
                    }
                    Ok(_) => String::from_utf8(bytes)
                        .map_err(|_| "Sidecar response was not UTF-8".into()),
                    Err(_) => Err("Failed to read sidecar stdout".into()),
                };
                let failed = response.is_err();
                if sender.send(response).is_err() || failed {
                    break;
                }
            }
        })
        .map_err(|e| format!("Failed to spawn sidecar response reader: {e}"))?;
    Ok(receiver)
}

impl LlmEngine {
    /// Spawn the Python sidecar process.
    pub fn spawn() -> Result<Self, String> {
        // Runtime inference and update checks never install packages implicitly.
        if !is_venv_ready() {
            return Err(
                "Cleanup runtime needs setup. Download the model from Settings to install it."
                    .into(),
            );
        }

        let python = venv_python()?;
        let sidecar_path = Self::sidecar_script_path()?;
        log::info!(
            "Spawning LLM sidecar: {} {}",
            python.display(),
            sidecar_path.display()
        );

        let child = OwnedChild::spawn(
            Command::new(&python)
                .arg(&sidecar_path)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )?;
        let pid = child.id();
        let stdin = nonblocking_stdin(
            child
                .lock()
                .stdin
                .take()
                .ok_or("Failed to open sidecar stdin")?,
        )?;
        let stdout = child
            .lock()
            .stdout
            .take()
            .ok_or("Failed to open sidecar stdout")?;
        let stderr = child
            .lock()
            .stderr
            .take()
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

        let responses = response_reader(stdout)?;
        Ok(Self {
            child,
            stdin,
            responses,
            registered_pid: None,
            pid,
        })
    }

    /// PID of the spawned Python subprocess. Captured at spawn time.
    pub fn child_pid(&self) -> u32 {
        self.pid
    }

    /// Send a request and read a response (blocking).
    fn request(&mut self, req: &serde_json::Value) -> Result<serde_json::Value, String> {
        self.request_with_timeout(req, request_timeout(req))
    }

    fn request_with_timeout(
        &mut self,
        req: &serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, String> {
        let deadline = Instant::now() + timeout;
        let mut line =
            serde_json::to_string(req).map_err(|e| format!("JSON serialize failed: {}", e))?;
        line.push('\n');
        if line.len() as u64 > MAX_REQUEST_LINE_BYTES {
            return Err("Cleanup request exceeds protocol limit".into());
        }

        // A large Unicode transcript can exceed pipe capacity. A hung child
        // must not keep write_all blocked before the response deadline starts.
        let mut remaining = line.as_bytes();
        while !remaining.is_empty() {
            if Instant::now() >= deadline {
                return Err(self.close_protocol("request write timed out"));
            }
            match self.stdin.write(remaining) {
                Ok(0) => return Err(self.close_protocol("request pipe closed while writing")),
                Ok(written) => remaining = &remaining[written..],
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(
                        Duration::from_millis(5)
                            .min(deadline.saturating_duration_since(Instant::now())),
                    );
                }
                Err(error) => {
                    return Err(self.close_protocol(format!("request write failed: {error}")))
                }
            }
        }

        let response = self
            .responses
            .recv_timeout(deadline.saturating_duration_since(Instant::now()));
        let response_line = match response {
            Ok(Ok(line)) => line,
            Ok(Err(error)) => {
                return Err(self.close_protocol(error));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                return Err(self.close_protocol(format!(
                    "request timed out after {} seconds",
                    timeout.as_secs()
                )));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(self.close_protocol("response channel disconnected"));
            }
        };
        serde_json::from_str(&response_line)
            .map_err(|_| self.close_protocol("response was not valid JSON"))
    }

    fn close_protocol(&mut self, reason: impl std::fmt::Display) -> String {
        self.quit();
        format!("Sidecar protocol closed: {reason}")
    }

    /// Register the PID before loading, so startup timeout recovery can kill it.
    pub fn spawn_tracked(pid: Arc<AtomicI32>) -> Result<Self, String> {
        let mut engine = Self::spawn()?;
        pid.store(engine.pid as i32, Ordering::SeqCst);
        engine.registered_pid = Some(pid);
        Ok(engine)
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
            Err(resp
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("Download failed")
                .into())
        }
    }

    /// Tell the sidecar to load the model into memory.
    pub fn load_model(&mut self) -> Result<(), String> {
        let resp = self.request(&serde_json::json!({"action": "load"}))?;
        validate_loaded_model(&resp)
    }

    /// The sidecar has no unsaved user state. Terminate directly rather than
    /// sending a blocking quit request to a potentially hung inference process.
    pub fn quit(&mut self) {
        self.child.terminate();
        if let Some(pid) = &self.registered_pid {
            let _ = pid.compare_exchange(self.pid as i32, 0, Ordering::SeqCst, Ordering::SeqCst);
        }
    }

    /// Find the sidecar script path.
    fn sidecar_script_path() -> Result<std::path::PathBuf, String> {
        let dev_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("sidecar")
            .join("llm_cleanup.py");
        let executable = std::env::current_exe().ok();
        resolve_sidecar_script(executable.as_deref(), &dev_path)
    }
}

/// ChildStdin is unbuffered. Only the app-owned write end is nonblocking;
/// Python keeps its ordinary blocking input loop.
fn nonblocking_stdin(stdin: std::process::ChildStdin) -> Result<std::process::ChildStdin, String> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let fd = stdin.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
        {
            return Err(format!(
                "Unable to configure sidecar input: {}",
                std::io::Error::last_os_error()
            ));
        }
    }
    Ok(stdin)
}

fn resolve_sidecar_script(
    executable: Option<&std::path::Path>,
    dev_path: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    // Bundles must use the protocol shipped with their Rust binary, even when
    // the source checkout still exists and contains a newer sidecar revision.
    if let Some(app_dir) = executable.and_then(std::path::Path::parent) {
        let bundled = app_dir.join("../Resources/sidecar/llm_cleanup.py");
        if bundled.is_file() {
            return Ok(bundled);
        }
        if app_dir.file_name().is_some_and(|name| name == "MacOS")
            && app_dir
                .parent()
                .and_then(std::path::Path::file_name)
                .is_some_and(|name| name == "Contents")
        {
            return Err("Bundled cleanup sidecar is missing; reinstall the app".into());
        }
    }
    if dev_path.is_file() {
        return Ok(dev_path.to_path_buf());
    }
    Err("LLM sidecar script not found".into())
}

pub fn validate_loaded_model(response: &serde_json::Value) -> Result<(), String> {
    if response["ok"] != true {
        return Err(response["error"]
            .as_str()
            .unwrap_or("Cleanup model could not load")
            .into());
    }
    if response["model_id"] != SOTTO_MODEL.id
        || response["revision"] != MODEL_REVISION
        || response["prompt_sha256"] != PROMPT_SHA256
        || response["warmed"] != true
    {
        return Err(
            "Cleanup model or prompt does not match this app; prepare cleanup again".into(),
        );
    }
    Ok(())
}

/// Stable sentinel for a *responded* deadline miss: the sidecar observed its
/// own generation deadline, cancelled the generator, and reported a clean
/// error line. On the current build/machine that response class was followed
/// by healthy reuse of the same process (docs/journals/2026-09-12-correction-
/// baseline-experiments.md §4, with its caveats), so — unlike pipe death —
/// this must not retire the resident handle. Deliberately avoids the substring
/// "timed out" so the zombie heuristic below cannot re-classify it.
pub const RESPONDED_TIMEOUT_ERROR: &str =
    "Cleanup deadline reached; original text preserved";

/// True when cleanup failed with the sidecar's own deadline response.
pub fn is_responded_timeout(err: &str) -> bool {
    err == RESPONDED_TIMEOUT_ERROR
}

/// Interpret one `cleanup_batch` response against the `n` texts it answers.
/// `Ok` holds exactly one item per input, in input order. `Err((retire,
/// reason))` splits the two whole-request channels the caller must
/// distinguish (spec §4.1):
/// - `retire = true` — the response is UNTRUSTWORTHY (no `results` array,
///   count ≠ n, non-int/duplicate/out-of-range index, `ok` item without a
///   string `text`, unknown status, over-cap line): the endpoint may emit
///   anything next, so the process is killed and restarted on next use.
/// - `retire = false` — a TYPED endpoint answer (outer `invalid_request`
///   for an over-cap or malformed request the sidecar declined, the
///   responded generation `timeout`, ordinary failed codes): a valid JSON
///   line from a healthy process; the resident handle stays.
///
/// Per-item failures/timeouts are `Ok` entries, never `Err`.
pub fn parse_batch_response(
    response: &serde_json::Value,
    n: usize,
) -> Result<Vec<BatchItem>, (bool, String)> {
    // The `ok` discriminator must be an EXPLICIT JSON boolean. Absent,
    // null, `"true"`, `1` — any of these is a malformed line from an
    // endpoint whose protocol behavior is unknown: retire, never guess.
    let ok = match response.get("ok") {
        Some(serde_json::Value::Bool(ok)) => *ok,
        _ => return Err((true, "batch response ok flag is missing or not boolean".into())),
    };
    if !ok {
        // A typed failure additionally requires BOTH schema halves: a
        // recognized error_code AND a string `error` message (the sidecar's
        // `safe_response` always emits both). A failure line with an
        // unknown/missing/non-string half is malformed, not a decline.
        let code = match response.get("error_code") {
            Some(serde_json::Value::String(code)) => code.as_str(),
            _ => {
                return Err((
                    true,
                    "failure response carries no string error_code".into(),
                ))
            }
        };
        let reason = match response.get("error") {
            Some(serde_json::Value::String(reason)) => reason.as_str(),
            _ => {
                return Err((
                    true,
                    format!("failure response for {code} carries no string error"),
                ))
            }
        };
        return Err(match code {
            // Protocol-healthy responded deadline — the retention sentinel,
            // never the zombie class (deliberately avoids "timed out").
            "timeout" => (false, RESPONDED_TIMEOUT_ERROR.to_string()),
            // A line over the framing cap desyncs the stream by definition
            // (the sidecar stops serving after request_limit; response_limit
            // replaced a batch whose bytes we never saw).
            "request_limit" | "response_limit" => (
                true,
                format!("Cleanup {code}: sidecar line exceeded the protocol cap"),
            ),
            "operation_failed" | "model_identity" | "model_verification" | "runtime_setup"
            | "model_setup" => (
                true,
                format!("Cleanup runtime requires restart ({code}); original text preserved"),
            ),
            other => (
                false,
                {
                    // The reason comes from the endpoint; keep it readable but
                    // never let it masquerade as a retire prefix (the `other`
                    // code is authoritative for the status class).
                    format!("Cleanup endpoint declined: {reason} [{other}]")
                },
            ),
        });
    }
    let Some(results) = response.get("results").and_then(serde_json::Value::as_array) else {
        return Err((true, "batch response carries no results array".into()));
    };
    if results.len() != n {
        return Err((
            true,
            format!("batch response has {} results for {n} texts", results.len()),
        ));
    }
    let mut items: Vec<Option<BatchItem>> = vec![None; n];
    for entry in results {
        let Some(index) = entry.get("index").and_then(serde_json::Value::as_u64) else {
            return Err((true, "batch result carries a non-integer index".into()));
        };
        let Ok(index) = usize::try_from(index) else {
            return Err((true, "batch result index out of range".into()));
        };
        if index >= n {
            return Err((
                true,
                format!("batch result index {index} outside 0..{n}"),
            ));
        }
        if items[index].is_some() {
            return Err((true, format!("batch result index {index} is duplicated")));
        }
        items[index] = Some(match entry.get("status").and_then(serde_json::Value::as_str) {
            Some("ok") => {
                let Some(text) = entry.get("text").and_then(serde_json::Value::as_str) else {
                    return Err((true, "ok batch result carries no text".into()));
                };
                // Trust-but-verify the endpoint's own output cap (spec E1:
                // "" is a VALID proposal; oversize is demoted per-item, not
                // batch-wide — the source validator remains the authority).
                if text.len() > crate::llm::validation::MAX_CLEANUP_BYTES {
                    BatchItem::Failed("proposal_limit".into())
                } else {
                    BatchItem::Proposal(Some(text.to_string()))
                }
            }
            Some("timeout") => BatchItem::TimedOut,
            Some("failed") => BatchItem::Failed(
                entry
                    .get("error_code")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("failed")
                    .to_string(),
            ),
            _ => return Err((true, "batch result carries an unknown status".into())),
        });
    }
    // `results.len() == n` + unique in-range indices already prove the set is
    // a permutation of 0..n-1; this is the belt, not the authority.
    if items.iter().any(Option::is_none) {
        return Err((true, "batch result index set is not a permutation".into()));
    }
    Ok(items.into_iter().map(Option::unwrap).collect())
}

impl LlmBackend for LlmEngine {
    fn is_alive(&mut self) -> bool {
        matches!(self.child.lock().try_wait(), Ok(None))
    }

    fn cleanup_batch(&mut self, texts: &[String]) -> Result<Vec<BatchItem>, String> {
        let response =
            self.request(&serde_json::json!({"action": "cleanup_batch", "texts": texts}))?;
        match parse_batch_response(&response, texts.len()) {
            Ok(items) => Ok(items),
            Err((true, reason)) => {
                // Untrustworthy endpoint: terminate now and clear the
                // registered PID so the next use respawns (spec §4.1).
                self.quit();
                Err(format!("Sidecar protocol closed: {reason}"))
            }
            Err((false, reason)) => Err(reason),
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
    // Python discovery belongs to explicit preparation, never a Settings read.
    true
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

/// Minimum version verified by the current local model experiments.
/// Keep in sync with MIN_MLX_LM in sidecar/llm_cleanup.py.
const MIN_MLX_LM: &str = "0.31.3";

/// Reproducible package versions exercised together on Apple Silicon. Explicit
/// setup upgrades compatible existing environments in place; inference never installs.
const RUNTIME_PACKAGES: [&str; 4] = [
    "mlx==0.32.2",
    "mlx-lm==0.31.3",
    "transformers==5.3.0",
    "huggingface-hub==1.7.2",
];

/// Build the Python one-liner used by `is_venv_ready()` to verify the venv has
/// both `mlx_lm` and `huggingface_hub` importable, a new-enough mlx-lm, and
/// a supported Python version (3.11+).
///
/// **CRITICAL**: This MUST be a single Python statement (semicolon-separated).
/// A previous version used a multi-line `format!` string with `\` line
/// continuations. Rust's `\` continuation eats all leading whitespace on the
/// next line, which collapsed the Python indentation inside `if ... :` blocks
/// and produced an IndentationError. That made `is_venv_ready()` always return
/// false and turned every spawn() call into a full venv wipe-and-rebuild.
/// See commit message for v0.7.3.
///
/// Exit code: 0 iff Python >= 3.11, mlx_lm is importable, huggingface_hub is
/// importable, and `mlx_lm.__version__ >= MIN_MLX_LM`. Exit 1 for old Python,
/// exit 2 for old mlx-lm. Writes a short status line to stderr so the Rust
/// log forwarder shows the detected versions.
fn build_venv_check_script() -> String {
    // Use conditional expressions (ternary) instead of compound `if:` statements,
    // because Python doesn't allow compound statements after semicolons on a
    // single line. `sys.exit(1) if cond else None` is valid: when cond is true,
    // `sys.exit(1)` raises SystemExit and the process ends; when false, `None`
    // is evaluated and execution continues.
    format!(
        "import sys; \
         pv = sys.version_info[:2]; \
         sys.stderr.write('Python ' + '.'.join(str(x) for x in pv) + ' < 3.11\\n') if pv < (3, 11) else None; \
         sys.exit(1) if pv < (3, 11) else None; \
         import mlx_lm, huggingface_hub; \
         v = getattr(mlx_lm, '__version__', '0.0.0'); \
         parts = [int(x) for x in v.split('.')[:3] if x.isdigit()]; \
         parts += [0] * (3 - len(parts)); \
         need = [int(x) for x in '{min}'.split('.')]; \
         from importlib.metadata import version; \
         pins = {pins}; \
         ok = parts >= need and all(version(p.split('==')[0]) == p.split('==')[1] for p in pins); \
         sys.stderr.write('Python ' + '.'.join(str(x) for x in pv) + ', mlx-lm ' + v + (' OK' if ok else ' differs from qualified runtime pins')); \
         sys.exit(0 if ok else 2)",
        min = MIN_MLX_LM,
        pins = serde_json::to_string(&RUNTIME_PACKAGES).expect("static package pins serialize")
    )
}

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

    let check_script = build_venv_check_script();

    let ok = bounded_command(
        Command::new(&python).args(["-c", &check_script]),
        Duration::from_secs(5),
    )
    .map(|out| {
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            log::warn!("Venv check: failed ({}): {}", out.status, stderr.trim());
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

/// Require the current cached snapshot's tokenizer, config, and every weight shard.
pub fn is_model_downloaded() -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    let cache = home
        .join(".cache/huggingface/hub")
        .join(format!("models--{}", SOTTO_MODEL.id.replace('/', "--")));
    let snapshot = cache.join("snapshots").join(MODEL_REVISION);
    snapshot_is_complete(&snapshot)
}

fn snapshot_is_complete(snapshot: &std::path::Path) -> bool {
    let marker = snapshot.join("sotto-verified.json");
    let Ok(bytes) = std::fs::read(marker) else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    if value["schema_version"] != 1
        || value["model_id"] != SOTTO_MODEL.id
        || value["revision"] != MODEL_REVISION
    {
        return false;
    }
    let Some(files) = value["files"].as_object() else {
        return false;
    };
    for name in [
        "config.json",
        "tokenizer_config.json",
        "tokenizer.json",
        "chat_template.jinja",
        "generation_config.json",
        "model.safetensors.index.json",
        "model.safetensors",
    ] {
        let Some(record) = files.get(name) else {
            return false;
        };
        let Ok(metadata) = snapshot.join(name).metadata() else {
            return false;
        };
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|time| time.as_nanos());
        if record["size"].as_u64() != Some(metadata.len())
            || record["mtime_ns"].as_u64().map(u128::from) != modified
        {
            return false;
        }
    }
    let present = |name: &str| {
        snapshot
            .join(name)
            .metadata()
            .is_ok_and(|m| m.is_file() && m.len() > 0)
    };
    if ![
        "config.json",
        "tokenizer_config.json",
        "tokenizer.json",
        "chat_template.jinja",
    ]
    .iter()
    .all(|name| present(name))
    {
        return false;
    }
    let index = snapshot.join("model.safetensors.index.json");
    if !index.exists() {
        return present("model.safetensors");
    }
    let Ok(contents) = std::fs::read_to_string(index) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&contents) else {
        return false;
    };
    let Some(map) = value.get("weight_map").and_then(|v| v.as_object()) else {
        return false;
    };
    !map.is_empty()
        && map.values().all(|value| {
            value
                .as_str()
                .is_some_and(|name| !name.contains('/') && !name.contains('\\') && present(name))
        })
}

/// Locate a Python 3.11+ interpreter on the host, preferring newer versions.
///
/// On macOS the default `python3` often resolves to Python 3.9 (shipped with
/// Xcode Command Line Tools), which cannot install `transformers>=5.0` and
/// therefore forces pip to pick an mlx-lm version with a broken LFM2
/// loader. Scanning for an explicit 3.11+ interpreter avoids this trap.
/// Returns an error if no Python 3.11+ interpreter is found — we no longer
/// fall back to Python 3.9 because transformers v5 models use
/// `TokenizersBackend`, which doesn't exist in transformers v4.
fn find_compatible_python() -> Result<std::path::PathBuf, String> {
    const SEARCH_DIRS: &[&str] = &[
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/opt/local/bin",
        "/usr/bin",
    ];
    // Newest → oldest so we prefer the freshest interpreter available.
    const VERSIONS: &[&str] = &["3.14", "3.13", "3.12", "3.11"];

    for version in VERSIONS {
        let bin_name = format!("python{}", version);
        for dir in SEARCH_DIRS {
            let candidate = std::path::PathBuf::from(dir).join(&bin_name);
            if candidate.exists() && is_python_311_or_newer(&candidate) {
                log::info!("Using {} for LLM venv", candidate.display());
                return Ok(candidate);
            }
        }
        // Also try PATH-relative lookup.
        if let Ok(out) =
            bounded_command(Command::new("which").arg(&bin_name), Duration::from_secs(2))
        {
            if out.status.success() {
                let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !path.is_empty() {
                    let candidate = std::path::PathBuf::from(&path);
                    if is_python_311_or_newer(&candidate) {
                        log::info!("Using {} for LLM venv", candidate.display());
                        return Ok(candidate);
                    }
                }
            }
        }
    }

    Err(
        "Python 3.11+ is required for the LLM feature but not found on this system. \
         Install it with: brew install python"
            .into(),
    )
}

/// Returns true iff the given interpreter reports a version >= 3.11.
fn is_python_311_or_newer(python: &std::path::Path) -> bool {
    let Ok(out) = bounded_command(
        Command::new(python).args([
            "-c",
            "import sys; print(1 if sys.version_info >= (3, 11) else 0)",
        ]),
        Duration::from_secs(3),
    ) else {
        return false;
    };
    out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "1"
}

/// Install/upgrade the runtime during an explicit model download.
/// Preserve existing environments and refuse an incompatible interpreter.
pub fn setup_venv() -> Result<(), String> {
    let venv = venv_dir()?;

    if venv.exists() && !is_python_311_or_newer(&venv.join("bin/python3")) {
        return Err("Existing cleanup runtime has an unsupported Python interpreter. Its files were preserved; repair the runtime before downloading.".into());
    }
    if let Some(parent) = venv.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create venv parent dir: {}", e))?;
    }

    if !venv.exists() {
        let host_python = find_compatible_python()?;
        let status = bounded_command(
            Command::new(&host_python).args(["-m", "venv", &venv.to_string_lossy()]),
            Duration::from_secs(60),
        )?
        .status;
        if !status.success() {
            return Err("Could not create cleanup Python runtime".into());
        }
    }
    let python = venv.join("bin").join("python3");

    log::info!("Installing the verified cleanup runtime into venv...");
    let output = bounded_command(
        Command::new(&python)
            .args([
                "-m",
                "pip",
                "install",
                "--upgrade",
                "--disable-pip-version-check",
                "--timeout",
                "30",
                "--retries",
                "2",
            ])
            .args(RUNTIME_PACKAGES),
        Duration::from_secs(600),
    )?;
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
/// A *responded* deadline (`RESPONDED_TIMEOUT_ERROR`) is protocol-healthy and
/// intentionally does not match: that handle is reused (see `is_responded_timeout`).
pub fn is_zombie_error(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("broken pipe")
        || e.contains("closed stdout")
        || e.contains("epipe")
        || e.contains("sidecar closed")
        || e.contains("crashed")
        || e.contains("timed out")
        || e.starts_with("sidecar protocol closed:")
        || e.starts_with("cleanup runtime requires restart")
}

/// Ensure a live sidecar handle is available, spawning + loading the model if
/// none is currently running. Returns the handle (ownership transferred to the
/// caller — remember to put it back in `state.llm_engine` after use).
///
/// The caller must hold `state.llm_operation` through the returned handle's
/// use and restoration, so lifecycle commands cannot replace its cached PID.
/// Retries once with a short backoff on persistent failures. On success,
/// stores the subprocess PID in `state.llm_pid` for `kill_orphan()` use.
/// See docs/specs/2026-04-11-llm-cleanup-reliability.md §4.1.
pub async fn ensure_running(state: &AppState) -> Result<Box<dyn LlmBackend>, String> {
    // Fast path — sidecar already running in the guard.
    {
        let mut guard = state.llm_engine.lock().await;
        if let Some(mut llm) = guard.take() {
            if llm.is_alive() {
                state.llm_loaded.store(true, Ordering::SeqCst);
                return Ok(llm);
            }
            log::warn!("Resident cleanup process exited; starting a fresh process");
        }
    }
    state.llm_loaded.store(false, Ordering::SeqCst);

    // Slow path — spawn + load, with retries.
    let mut last_err = String::new();
    for attempt in 0..SPAWN_MAX_ATTEMPTS {
        log::info!(
            "Spawning LLM sidecar (attempt {}/{})...",
            attempt + 1,
            SPAWN_MAX_ATTEMPTS
        );
        let pid = Arc::clone(&state.llm_pid);
        let spawn = tokio::task::spawn_blocking(move || {
            let mut e = LlmEngine::spawn_tracked(pid)?;
            e.load_model()?;
            Ok::<_, String>(e)
        })
        .await;

        match spawn {
            Ok(Ok(engine)) => {
                state
                    .llm_pid
                    .store(engine.child_pid() as i32, Ordering::SeqCst);
                state.llm_loaded.store(true, Ordering::SeqCst);
                log::info!("LLM sidecar ready (pid={})", engine.child_pid());
                return Ok(Box::new(engine) as Box<dyn LlmBackend>);
            }
            Ok(Err(e)) => {
                last_err = e;
            }
            Err(e) => {
                last_err = format!("spawn task panic: {}", e);
            }
        }

        if attempt + 1 < SPAWN_MAX_ATTEMPTS {
            tokio::time::sleep(Duration::from_millis(SPAWN_RETRY_DELAY_MS)).await;
        }
    }

    state.llm_pid.store(0, Ordering::SeqCst);
    log::warn!(
        "LLM sidecar could not be started after {} attempts: {}",
        SPAWN_MAX_ATTEMPTS,
        last_err
    );
    Err(last_err)
}

/// Terminate the registered owned subprocess after a cleanup panic or timeout.
/// The blocking task can retain the engine, so the registry shares only its
/// Child handle for termination and reaping. A cached numeric PID alone never
/// authorizes signalling an unrelated process. Clear it before recovery so a
/// subsequent call cannot target a newly spawned engine.
/// See docs/specs/2026-04-11-llm-cleanup-reliability.md §4.3.
pub fn kill_orphan(state: &AppState) {
    state.llm_loaded.store(false, Ordering::SeqCst);
    let pid = state.llm_pid.swap(0, Ordering::SeqCst);
    if pid <= 0 {
        return;
    }
    crate::process::terminate_registered(pid as u32);
    log::warn!("Terminated registered cleanup process (pid={})", pid);
}

/// Check if a newer model is available on HuggingFace.
/// Returns Ok(true) if update available, Ok(false) if up to date, Err on failure.
/// Does NOT load the MLX model — only reads refs/main and calls repo_info().
///
/// A temporary unloaded sidecar performs the network query. It never borrows
/// the resident process, changes its PID, or delays active cleanup inference.
pub async fn check_model_update(_app: &tauri::AppHandle) -> Result<bool, String> {
    tokio::task::spawn_blocking(|| match LlmEngine::spawn() {
        Ok(mut e) => {
            let resp = e.request_raw(&serde_json::json!({"action": "check_update"}));
            e.quit();
            match resp {
                Ok(response) => model_update_available(&response),
                Err(e) => Err(format!("Check failed: {}", e)),
            }
        }
        Err(e) => Err(format!("Could not spawn sidecar: {}", e)),
    })
    .await
    .map_err(|e| format!("Check panicked: {}", e))?
}

fn model_update_available(response: &serde_json::Value) -> Result<bool, String> {
    if response.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err(format!(
            "Model update check failed: {}",
            response
                .get("error")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("invalid sidecar response")
        ));
    }
    response
        .get("update_available")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| "Model update check returned no availability result".into())
}

/// The single model configuration for SottoASR transcript cleanup.
pub struct ModelConfig {
    pub id: &'static str,
    pub display_name: &'static str,
    pub download_size_mb: u64,
}

pub const PROMPT_SHA256: &str = "2edd80834efc831c1f7d37f93da35c209622525b39dcc766c01159f6ad87de7f";

pub const MODEL_REVISION: &str = "32f8dd5df1188512a20413f1297083238306634c";

pub const SOTTO_MODEL: ModelConfig = ModelConfig {
    id: "openbmb/MiniCPM5-2B-MLX",
    display_name: "MiniCPM5 2B (official, 4-bit)",
    download_size_mb: 1427,
};

/// Get the model configuration.
pub fn model_config() -> &'static ModelConfig {
    &SOTTO_MODEL
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch_ok(texts: usize) -> serde_json::Value {
        let results: Vec<serde_json::Value> = (0..texts)
            .map(|i| serde_json::json!({"index": i, "status": "ok", "text": "", "elapsed_ms": 1}))
            .collect();
        serde_json::json!({"ok": true, "results": results})
    }

    #[test]
    fn batch_parser_accepts_the_wire_shape_and_empty_proposal() {
        // E1: "" is a VALID proposal (the fully-erasing 1-word case).
        let items = parse_batch_response(&batch_ok(1), 1).unwrap();
        assert!(matches!(&items[0], BatchItem::Proposal(Some(s)) if s.is_empty()));
        // Per-item status union.
        let resp = serde_json::json!({"ok":true,"results":[
            {"index":0,"status":"timeout"},
            {"index":1,"status":"failed","error_code":"incomplete_generation"},
            {"index":2,"status":"ok","text":"clean"}]});
        let items = parse_batch_response(&resp, 3).unwrap();
        assert!(matches!(items[0], BatchItem::TimedOut));
        assert!(matches!(&items[1], BatchItem::Failed(c) if c == "incomplete_generation"));
        assert!(matches!(&items[2], BatchItem::Proposal(Some(s)) if s == "clean"));
        // Out-of-cap ok text demoted per-item, siblings unharmed.
        let big = "x".repeat(crate::llm::validation::MAX_CLEANUP_BYTES + 1);
        let resp = serde_json::json!({"ok":true,"results":[
            {"index":0,"status":"ok","text":big},
            {"index":1,"status":"ok","text":"ok"}]});
        let items = parse_batch_response(&resp, 2).unwrap();
        assert!(matches!(&items[0], BatchItem::Failed(c) if c == "proposal_limit"));
        assert!(matches!(&items[1], BatchItem::Proposal(Some(s)) if s == "ok"));
        assert_eq!(
            request_timeout(&serde_json::json!({"action":"cleanup_batch","texts":[]})),
            Duration::from_secs(15)
        );
    }

    #[test]
    fn batch_parser_splits_untrustworthy_and_typed_whole_request_errors() {
        // Untrustworthy (retire = true): kill + respawn on next use.
        for response in [
            serde_json::json!({"ok": true}), // no results array
            serde_json::json!({"ok":true,"results":[]}), // count ≠ n
            serde_json::json!({"ok":true,"results":[
                {"index":0,"status":"ok","text":"a"},
                {"index":0,"status":"ok","text":"b"}]}), // duplicate index
            serde_json::json!({"ok":true,"results":[{"index":1,"status":"ok","text":"a"}]}), // out of range
            serde_json::json!({"ok":true,"results":[{"index":"0","status":"ok","text":"a"}]}), // non-int
            serde_json::json!({"ok":true,"results":[{"index":0,"status":"ok"}]}), // ok without text
            serde_json::json!({"ok":true,"results":[{"index":0,"status":"done"}]}), // unknown status
            serde_json::json!({"ok":false,"error_code":"request_limit"}),
            serde_json::json!({"ok":false,"error_code":"response_limit"}),
            serde_json::json!({"ok":false,"error_code":"operation_failed"}),
        ] {
            let (retire, _) = parse_batch_response(&response, 1).unwrap_err();
            assert!(retire, "must retire the endpoint for {response}");
        }
        // The `ok` discriminator itself: anything that is not an explicit
        // JSON boolean is a malformed protocol line — retire, never guess.
        for response in [
            serde_json::json!({}), // no discriminator at all
            serde_json::json!([]), // not an object
            serde_json::json!({"ok": "true", "results": []}), // string, not bool
            serde_json::json!({"ok": null, "results": []}),   // explicit null
            serde_json::json!({"ok": 1, "results": []}),      // number, not bool
            // A typed failure needs BOTH halves of the schema (safe_response
            // always emits both); a half is as untrustworthy as none.
            serde_json::json!({"ok": false}), // no code, no error
            serde_json::json!({"ok": false, "error_code": "timeout"}), // no error message
            serde_json::json!({"ok": false, "error_code": 7, "error": "x"}), // non-string code
            serde_json::json!({"ok": false, "error": "boom"}), // code missing
        ] {
            let (retire, _) = parse_batch_response(&response, 1).unwrap_err();
            assert!(retire, "must retire the endpoint for {response}");
        }
        // Typed (retire = false): handle reused. Full schema on the wire.
        let (retire, reason) = parse_batch_response(
            &serde_json::json!({"ok":false,"error_code":"timeout","error":"Cleanup request timed out"}),
            1,
        )
        .unwrap_err();
        assert!(!retire);
        assert!(is_responded_timeout(&reason), "{reason}");
        assert!(!is_zombie_error(&reason));
        let (retire, reason) = parse_batch_response(
            &serde_json::json!({"ok":false,"error_code":"invalid_request","error":"too many texts; maximum is 16"}),
            1,
        )
        .unwrap_err();
        assert!(!retire);
        // A laundered endpoint reason can never masquerade as a retire prefix.
        assert!(!is_zombie_error(&reason), "{reason}");
        // An unknown code with a full schema is still a DECLINE (retained
        // handle) — it is the missing/non-string half that retires.
        let (retire, reason) = parse_batch_response(
            &serde_json::json!({"ok":false,"error_code":"some_future_code","error":"whatever the endpoint says"}),
            1,
        )
        .unwrap_err();
        assert!(!retire, "known-shape unknown code is a decline");
        assert!(!is_zombie_error(&reason), "{reason}");
    }

    /// Regression (2026-09-12, batched by design): the sidecar's own
    /// `timeout` error_code classifies as a responded deadline (protocol-
    /// healthy, handle reused); `operation_failed` keeps the restart
    /// classification — proven over REAL subprocesses with the batched wire.
    #[test]
    fn responded_deadline_is_not_a_zombie_over_the_real_protocol() {
        let source = "Please keep the final instruction.";
        let mut engine = stub_engine(concat!(
            "import json,sys\n",
            "n=0\n",
            "for line in sys.stdin:\n",
            " n+=1\n",
            " if n==1:\n",
            "  print(json.dumps({'ok':False,'error_code':'timeout','error':'Cleanup timed out; original text preserved.'}),flush=True)\n",
            " else:\n",
            "  r=json.loads(line)\n",
            "  print(json.dumps({'ok':True,'results':[{'index':i,'status':'ok','text':t} for i,t in enumerate(r.get('texts',[]))]}),flush=True)\n",
        ));
        let error = engine.cleanup_batch(&[source.to_string()]).unwrap_err();
        assert!(is_responded_timeout(&error));
        assert!(!is_zombie_error(&error));
        // The process survived its own deadline and serves the next request.
        let items = engine.cleanup_batch(&[source.to_string()]).unwrap();
        assert!(matches!(&items[0], BatchItem::Proposal(Some(s)) if s == source));
        assert!(engine.is_alive());

        let mut failing = stub_engine(
            "import json,sys\nfor line in sys.stdin:\n print(json.dumps({'ok':False,'error_code':'operation_failed','error':'Local cleanup runtime failed; it will restart for the next recording. Original text preserved.'}),flush=True)",
        );
        let error = failing.cleanup_batch(&[source.to_string()]).unwrap_err();
        assert!(!is_responded_timeout(&error));
        assert!(is_zombie_error(&error));
    }

    #[test]
    fn rust_and_bundled_sidecar_select_the_same_pinned_model() {
        let sidecar = include_str!("../../sidecar/llm_cleanup.py");
        assert!(sidecar.contains(SOTTO_MODEL.id));
        assert!(sidecar.contains(MODEL_REVISION));
        assert!(sidecar.contains(PROMPT_SHA256));
        assert!(validate_loaded_model(&serde_json::json!({"ok":true})).is_err());
        assert!(validate_loaded_model(&serde_json::json!({"ok":true,"model_id":SOTTO_MODEL.id,"revision":MODEL_REVISION,"prompt_sha256":PROMPT_SHA256,"warmed":true})).is_ok());
        assert!(sidecar.contains(SOTTO_MODEL.display_name));
    }

    #[test]
    fn failed_model_update_check_is_not_reported_as_up_to_date() {
        let failed = serde_json::json!({"ok": false, "error": "offline"});
        assert!(model_update_available(&failed)
            .unwrap_err()
            .contains("offline"));
        assert!(model_update_available(&serde_json::json!({"ok": true})).is_err());
        for available in [false, true] {
            assert_eq!(
                model_update_available(&serde_json::json!({
                    "ok": true, "update_available": available,
                })),
                Ok(available)
            );
        }
    }

    #[test]
    fn installed_runtime_matches_the_verified_minimum() {
        assert!(RUNTIME_PACKAGES.contains(&format!("mlx-lm=={MIN_MLX_LM}").as_str()));
    }

    fn stub_engine(script: &str) -> LlmEngine {
        let child = OwnedChild::spawn(
            Command::new("python3")
                .args(["-c", script])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null()),
        )
        .unwrap();
        let pid = child.id();
        let responses = response_reader(child.lock().stdout.take().unwrap()).unwrap();
        let stdin = nonblocking_stdin(child.lock().stdin.take().unwrap()).unwrap();
        LlmEngine {
            stdin,
            child,
            responses,
            pid,
            registered_pid: Some(Arc::new(AtomicI32::new(pid as i32))),
        }
    }

    #[test]
    #[cfg(unix)]
    fn blocked_input_obeys_the_complete_request_deadline() {
        let mut engine = stub_engine("import time; time.sleep(20)");
        let started = Instant::now();
        let error = engine
            .request_with_timeout(
                &serde_json::json!({"action": "cleanup", "text": "語".repeat(60_000)}),
                Duration::from_millis(80),
            )
            .unwrap_err();
        assert!(error.contains("write timed out"));
        assert!(is_zombie_error(&error));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(engine.child.lock().try_wait().unwrap().is_some());
        assert_eq!(
            engine
                .registered_pid
                .as_ref()
                .unwrap()
                .load(Ordering::SeqCst),
            0
        );
    }

    #[tokio::test]
    async fn runtime_fault_after_three_requests_retires_the_resident_process() {
        use crate::test_support::{MockAudioCapture, MockAsrEngine, MockPasteBackend};
        // Batched wire: answer every text in the request, until the stub
        // dies on the fourth request with the restart-class fault.
        let engine = stub_engine("import json,sys\nn=0\nfor line in sys.stdin:\n r=json.loads(line); n+=1\n if n<=3:\n  print(json.dumps({'ok':True,'results':[{'index':i,'status':'ok','text':'Please keep the final instruction.'} for i in range(len(r['texts']))]}),flush=True)\n else:\n  print(json.dumps({'ok':False,'error_code':'operation_failed','error':'Local cleanup operation failed; original text preserved.'}),flush=True)");
        let pid = engine.registered_pid.as_ref().unwrap().clone();
        let mut state = AppState::new_with_backends(Box::new(MockAudioCapture::sine_wave()),
            Box::new(MockAsrEngine::with_text("unused")), Some(Box::new(engine)),
            Box::new(MockPasteBackend::new()), crate::models::Settings::default());
        state.llm_pid = pid.clone();
        let source = "Please um keep the final instruction.";
        for _ in 0..3 {
            let (_, status) = crate::llm::cleanup::run_cleanup(&state, source, &[]).await;
            assert!(matches!(status, crate::models::LlmCleanupStatus::Applied { .. }));
        }
        let (output, status) = crate::llm::cleanup::run_cleanup(&state, source, &[]).await;
        assert_eq!(output, source);
        assert!(matches!(status, crate::models::LlmCleanupStatus::Failed { .. }));
        assert!(state.llm_engine.lock().await.is_none());
        assert!(!state.llm_loaded.load(Ordering::SeqCst));
        assert_eq!(pid.load(Ordering::SeqCst), 0);
        // A fresh process handles the next recording; no poisoned handle survives.
        *state.llm_engine.lock().await = Some(Box::new(stub_engine("import json,sys\nfor line in sys.stdin:\n r=json.loads(line)\n print(json.dumps({'ok':True,'results':[{'index':i,'status':'ok','text':'Please keep the final instruction.'} for i in range(len(r['texts']))]}),flush=True)")));
        let (_, status) = crate::llm::cleanup::run_cleanup(&state, source, &[]).await;
        assert!(matches!(status, crate::models::LlmCleanupStatus::Applied { .. }));
    }

    #[test]
    fn partial_pipe_writes_deliver_the_complete_unicode_request() {
        let mut engine = stub_engine(
            "import json,sys; request=json.loads(sys.stdin.readline()); print(json.dumps({'length':len(request['text']), 'tail':request['text'][-4:]}), flush=True)",
        );
        let text = format!("{}尾端保持", "語".repeat(60_000));
        let result = engine
            .request_with_timeout(
                &serde_json::json!({"action": "cleanup", "text": text}),
                Duration::from_secs(2),
            )
            .unwrap();
        assert_eq!(result["length"], 60_004);
        assert_eq!(result["tail"], "尾端保持");
    }

    #[test]
    fn partial_response_deadline_kills_and_clears_pid() {
        let mut engine = stub_engine(
            "import sys,time; sys.stdout.write('{'); sys.stdout.flush(); time.sleep(20)",
        );
        let started = std::time::Instant::now();
        let result = engine.request_with_timeout(
            &serde_json::json!({"action":"status"}),
            Duration::from_millis(80),
        );
        assert!(result.unwrap_err().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(
            engine
                .registered_pid
                .as_ref()
                .unwrap()
                .load(Ordering::SeqCst),
            0
        );
        assert!(engine.child.lock().try_wait().unwrap().is_some());
    }

    #[test]
    fn oversized_protocol_response_fails_without_unbounded_allocation() {
        // The stub emits ONE line larger than MAX_RESPONSE_LINE_BYTES with no
        // terminator: the bounded reader must cut it and close the protocol
        // without ever allocating the whole oversized line unbounded.
        let mut engine = stub_engine(&format!(
            "import sys,time; sys.stdout.write('x'*{}); sys.stdout.flush(); time.sleep(20)",
            MAX_RESPONSE_LINE_BYTES as usize + 16,
        ));
        let result = engine.request_with_timeout(
            &serde_json::json!({"action":"status"}),
            Duration::from_secs(2),
        );
        let error = result.unwrap_err();
        assert!(error.contains("protocol limit"), "{error}");
        assert!(is_zombie_error(&error));
        assert!(engine.child.lock().try_wait().unwrap().is_some());
    }

    #[test]
    fn malformed_json_closes_the_protocol_and_marks_the_handle_dead() {
        let mut engine = stub_engine("import time; print('not-json', flush=True); time.sleep(20)");
        let error = engine
            .request_with_timeout(
                &serde_json::json!({"action":"status"}),
                Duration::from_secs(2),
            )
            .unwrap_err();
        assert!(is_zombie_error(&error));
        assert!(engine.child.lock().try_wait().unwrap().is_some());
    }

    #[test]
    fn shutdown_does_not_wait_for_a_quit_reply() {
        let mut engine = stub_engine("import time; time.sleep(20)");
        let started = std::time::Instant::now();
        engine.quit();
        engine.quit();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(engine.child.lock().try_wait().unwrap().is_some());
    }

    #[test]
    fn bundled_sidecar_takes_precedence_over_a_newer_source_checkout() {
        let directory = tempfile::TempDir::new().unwrap();
        let macos = directory.path().join("SottoASR.app/Contents/MacOS");
        let bundled = directory
            .path()
            .join("SottoASR.app/Contents/Resources/sidecar/llm_cleanup.py");
        let dev = directory.path().join("llm_cleanup.py");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::create_dir_all(bundled.parent().unwrap()).unwrap();
        std::fs::write(&bundled, "bundled protocol").unwrap();
        std::fs::write(&dev, "different dev protocol").unwrap();

        let resolved = resolve_sidecar_script(Some(&macos.join("sottoasr")), &dev).unwrap();
        assert_eq!(
            resolved.canonicalize().unwrap(),
            bundled.canonicalize().unwrap()
        );
        assert_eq!(resolve_sidecar_script(None, &dev).unwrap(), dev);
        let incomplete = directory
            .path()
            .join("Incomplete.app/Contents/MacOS/sottoasr");
        assert!(resolve_sidecar_script(Some(&incomplete), &dev).is_err());
        assert!(resolve_sidecar_script(None, &directory.path().join("missing.py")).is_err());
    }

    #[test]
    fn next_job_id_monotonic() {
        let id1 = next_job_id();
        let id2 = next_job_id();
        assert!(
            id2 > id1,
            "Job IDs must be monotonically increasing: {} should be > {}",
            id2,
            id1
        );
    }

    #[test]
    fn next_job_id_never_zero() {
        let id = next_job_id();
        assert!(id > 0, "Job ID should never be 0, got {}", id);
    }

    #[test]
    fn sotto_model_id_is_correct() {
        assert_eq!(SOTTO_MODEL.id, "openbmb/MiniCPM5-2B-MLX");
    }

    #[test]
    fn sotto_model_display_name() {
        assert_eq!(SOTTO_MODEL.display_name, "MiniCPM5 2B (official, 4-bit)");
    }

    #[test]
    fn sotto_model_download_size() {
        assert_eq!(SOTTO_MODEL.download_size_mb, 1427);
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
        assert!(is_zombie_error(
            "Sidecar closed stdout (process may have crashed)"
        ));
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


    /// Run `python3 -c "..."` against the real `build_venv_check_script()` to
    /// prove the generated Python actually parses and executes. We don't care
    /// whether mlx_lm is importable here — we care about SyntaxError /
    /// IndentationError, which would surface as exit 1 with parse error on
    /// stderr. Exit code 1 (ModuleNotFoundError) is an acceptable outcome in
    /// a minimal Python; exit 2 (version check failed) is also acceptable;
    /// exit 0 (ok) is acceptable. Any syntax/indent error fails the test.
    #[test]
    fn venv_check_script_parses_in_python() {
        let script = build_venv_check_script();
        let python = which_python3();
        let Some(python) = python else {
            // No system python3 available in the test env — skip silently.
            eprintln!("(skipping: no python3 on PATH)");
            return;
        };
        let out = Command::new(&python)
            .args(["-c", &script])
            .output()
            .expect("exec python3");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stderr.contains("SyntaxError") && !stderr.contains("IndentationError"),
            "generated venv check script failed to parse as Python: {}",
            stderr
        );
    }

    fn which_python3() -> Option<std::path::PathBuf> {
        let out = Command::new("which").arg("python3").output().ok()?;
        if !out.status.success() {
            return None;
        }
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if path.is_empty() {
            None
        } else {
            Some(std::path::PathBuf::from(path))
        }
    }
}
