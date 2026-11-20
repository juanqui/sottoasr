use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::models::CleanupMode;
use crate::process::{bounded_command, bounded_output, BoundedError, OwnedChild, SpawnError};
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
    fn cleanup_batch(&mut self, texts: &[String], mode: CleanupMode) -> Result<Vec<BatchItem>, String>;
    /// Send a raw JSON request and return the raw JSON response.
    /// Used by `commands/llm.rs` for the `load` handshake, which bypasses the
    /// typed batch API.
    fn request_raw(&mut self, req: &serde_json::Value) -> Result<serde_json::Value, String>;

    /// Shut down the backend. Default is a no-op.
    /// Production impl kills the sidecar process.
    fn shutdown(&mut self) {}

    /// The options of an Amazon Bedrock backend. `None` is a backend for
    /// the local model.
    fn bedrock_options(&self) -> Option<&crate::models::AwsBedrockSettings> { None }
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

/// Response deadline for one `cleanup_batch` request: the sidecar's 10 s
/// generation alarm plus overhead.
pub(crate) const CLEANUP_BATCH_TIMEOUT: Duration = Duration::from_secs(15);

fn request_timeout(request: &serde_json::Value) -> Duration {
    match request.get("action").and_then(|v| v.as_str()) {
        Some("load") => Duration::from_secs(15),
        // CRITICAL (spec §4.1): the batch action MUST map here. An unmapped
        // action silently gets the 5 s default, which retires the sidecar
        // mid-batch — measured 6.6 s for a real production 16-batch.
        Some("cleanup_batch") => CLEANUP_BATCH_TIMEOUT,
        _ => Duration::from_secs(5),
    }
}

/// Write budget for a request that has no response deadline (the download).
const SUPERVISED_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// Which job a sidecar process does. Both roles run isolated from the user's
/// environment (P-09); only a download may reach Hugging Face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidecarRole {
    /// Load the pinned model and serve cleanup requests, offline.
    Inference,
    /// Fetch and verify the pinned model files.
    Download,
}

/// Hugging Face hub cache shared by the sidecar and the Rust readiness checks.
/// This is the library default under `HOME`; it is passed explicitly because
/// the sidecar environment is cleared.
pub fn hf_hub_cache() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or("Could not determine home directory")?;
    Ok(home.join(".cache").join("huggingface").join("hub"))
}

/// Cache directory of the pinned cleanup model.
pub fn model_cache_dir() -> Result<PathBuf, String> {
    Ok(hf_hub_cache()?.join(format!("models--{}", SOTTO_MODEL.id.replace('/', "--"))))
}

/// A Python command for the cleanup runtime: `-I` and a cleared environment,
/// so user site-packages, `PYTHON*` variables and `PATH` cannot change what
/// runs. Callers append the script or `-c` arguments.
fn isolated_python(python: &Path, role: SidecarRole) -> Result<Command, String> {
    let home = dirs::home_dir().ok_or("Could not determine home directory")?;
    let mut command = Command::new(python);
    command
        .arg("-I")
        .env_clear()
        .env("HOME", &home)
        .env("TMPDIR", std::env::temp_dir())
        .env("PATH", "/usr/bin:/bin")
        .env("HF_HUB_CACHE", hf_hub_cache()?);
    match role {
        SidecarRole::Inference => {
            command.env("HF_HUB_OFFLINE", "1");
        }
        // hf-xet fills the model file in large bursts. A real download of the
        // pinned model (2026-11-19, ~15 MB/s link) left the model cache flat
        // for about 62 s, then jumped 211 MB → 1.43 GB; slower links would
        // trip the 120 s stall watch on a healthy download. With xet disabled
        // the same download grew linearly (~17 MB/s, 89 s versus 100 s).
        SidecarRole::Download => {
            command.env("HF_HUB_DISABLE_XET", "1");
        }
    }
    Ok(command)
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
    /// Spawn the Python sidecar process for `role`.
    pub fn spawn(role: SidecarRole) -> Result<Self, String> {
        // Neither role installs packages implicitly.
        // The reason is derived from the verdict so the history tooltip names
        // the real fault instead of always pointing at a model download.
        let verdict = venv_verdict();
        if !verdict.is_ready() {
            return Err(verdict.unavailable_reason());
        }

        let python = venv_python()?;
        let sidecar_path = Self::sidecar_script_path()?;
        log::info!(
            "Spawning LLM sidecar ({role:?}): {} {}",
            python.display(),
            sidecar_path.display()
        );

        let child = OwnedChild::spawn(
            isolated_python(&python, role)?
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
        self.write_request(req, deadline)?;
        match self
            .responses
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            Ok(line) => self.parse_response(line),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(self.close_protocol(format!(
                "request timed out after {} seconds",
                timeout.as_secs()
            ))),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(self.close_protocol("response channel disconnected"))
            }
        }
    }

    /// Send a request that has no response deadline. `supervise` runs every
    /// `poll` while the reply is pending; an error from it terminates the
    /// process and is returned as-is.
    fn request_supervised(
        &mut self,
        req: &serde_json::Value,
        poll: Duration,
        mut supervise: impl FnMut() -> Result<(), String>,
    ) -> Result<serde_json::Value, String> {
        self.write_request(req, Instant::now() + SUPERVISED_WRITE_TIMEOUT)?;
        loop {
            match self.responses.recv_timeout(poll) {
                Ok(line) => return self.parse_response(line),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Err(reason) = supervise() {
                        self.quit();
                        return Err(reason);
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(self.close_protocol("response channel disconnected"))
                }
            }
        }
    }

    fn write_request(&mut self, req: &serde_json::Value, deadline: Instant) -> Result<(), String> {
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
        Ok(())
    }

    fn parse_response(
        &mut self,
        line: Result<String, String>,
    ) -> Result<serde_json::Value, String> {
        match line {
            Ok(line) => serde_json::from_str(&line)
                .map_err(|_| self.close_protocol("response was not valid JSON")),
            Err(error) => Err(self.close_protocol(error)),
        }
    }

    fn close_protocol(&mut self, reason: impl std::fmt::Display) -> String {
        self.quit();
        format!("Sidecar protocol closed: {reason}")
    }

    /// Register the PID before loading, so startup timeout recovery can kill it.
    pub fn spawn_tracked(pid: Arc<AtomicI32>) -> Result<Self, String> {
        let mut engine = Self::spawn(SidecarRole::Inference)?;
        pid.store(engine.pid as i32, Ordering::SeqCst);
        engine.registered_pid = Some(pid);
        Ok(engine)
    }

    /// Tell the sidecar to download and verify the model. There is no total
    /// deadline: `supervise` runs every `poll` and stops the download by
    /// returning an error (R-07).
    pub fn download_model(
        &mut self,
        poll: Duration,
        supervise: impl FnMut() -> Result<(), String>,
    ) -> Result<(), String> {
        let resp =
            self.request_supervised(&serde_json::json!({"action": "download"}), poll, supervise)?;
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

    fn cleanup_batch(&mut self, texts: &[String], mode: CleanupMode) -> Result<Vec<BatchItem>, String> {
        let response = self.request(&serde_json::json!({
            "action": "cleanup_batch", "texts": texts, "mode": mode
        }))?;
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

/// The app's data directory, which holds the cleanup runtime.
fn app_data_dir() -> Result<PathBuf, String> {
    let data_dir = dirs::data_dir().ok_or("Could not determine data directory")?;
    Ok(data_dir.join("com.sottoasr.app"))
}

/// Get the path to the app-managed Python venv for mlx-lm.
pub fn venv_dir() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("llm-venv"))
}

/// Get the Python executable inside the app's venv.
pub fn venv_python() -> Result<PathBuf, String> {
    Ok(venv_dir()?.join("bin").join("python3"))
}

/// Where the standalone interpreter is provisioned when the Mac has none (G-06).
fn standalone_python_root() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("python"))
}

/// Verdict of the venv readiness probe.
///
/// `Broken` and `Unknown` MUST stay distinct. `Broken` is deterministic state
/// that only a repair can change, so caching it for the process lifetime is
/// correct. `Unknown` means the probe never got an answer — a spawn failure, a
/// timeout, or a system stall — and caching that as a verdict previously
/// disabled cleanup for the life of the process. This is a menu-bar app that
/// runs for days, so one 5-second probe timeout cost three days of cleanup.
/// See docs/specs/2026-04-11-llm-cleanup-reliability.md §4.6.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VenvVerdict {
    /// The interpreter ran and every locked package matches.
    Ready,
    /// The interpreter ran and the four top-level pins match, but other
    /// packages differ from the lock (every 0.11.1 install). Cleanup runs on
    /// it; Ensure reinstalls from the lock.
    Stale(String),
    /// No runtime is installed. Ensure creates one.
    Missing,
    /// The interpreter ran and reported a definitive negative (Python outside
    /// 3.11 to 3.14, a missing or incorrect top-level package), or the runtime
    /// exists without a working interpreter. Only a repair changes this.
    Broken(String),
    /// The probe could not complete. Transient by definition — never trusted
    /// beyond `VENV_UNKNOWN_RETRY_INTERVAL`.
    Unknown(String),
}

impl VenvVerdict {
    pub fn is_ready(&self) -> bool {
        matches!(self, VenvVerdict::Ready | VenvVerdict::Stale(_))
    }

    /// User-facing reason for `LlmCleanupStatus::Unavailable`. This string is
    /// rendered in the history tooltip, so it must name the real fault and the
    /// real recovery. Pointing at the model download when the fault is the
    /// interpreter sends the user to a screen that cannot fix anything.
    pub fn unavailable_reason(&self) -> String {
        match self {
            VenvVerdict::Ready | VenvVerdict::Stale(_) => "Cleanup runtime is ready".into(),
            VenvVerdict::Missing => {
                "Cleanup runtime is not installed. Open Settings → AI cleanup to set it up.".into()
            }
            VenvVerdict::Broken(reason) => format!(
                "Cleanup runtime needs repair: {reason}. Open Settings → AI cleanup and repair it."
            ),
            VenvVerdict::Unknown(reason) => format!(
                "Cleanup runtime check did not complete ({reason}). Cleanup retries on the next recording."
            ),
        }
    }
}

/// How long an `Unknown` verdict is trusted before the probe runs again.
///
/// Short by design: a transient stall must cost at most one recording.
pub const VENV_UNKNOWN_RETRY_INTERVAL: Duration = Duration::from_secs(15);

/// Budget for the full probe (`import mlx_lm` + pin verification).
///
/// Sized off a measured cost, not an estimate. The probe imports the MLX
/// natives, and this project has already measured a cold page-in of ~6.5 s for
/// them (see `llm::cleanup::PREWARM_HANDOFF_WAIT`); a healthy probe costs
/// 1.9–3.2 s on an idle machine. The previous 5 s budget therefore had only
/// ~1.6x headroom and lost the race whenever the app was loading ASR models
/// concurrently. Paid once per process, and still under
/// `llm::cleanup::LLM_CLEANUP_TIMEOUT` (30 s).
pub const VENV_PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// Budget for the trivial version probe. Generous because a system-wide I/O
/// stall, not the interpreter, is what makes it slow.
pub const PYTHON_VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Whether a cached verdict may still be trusted.
///
/// Only `Unknown` expires: every other verdict is deterministic until a
/// preparation changes the runtime, while an unanswered probe must be retried.
fn verdict_is_fresh(verdict: &VenvVerdict, age: Duration, unknown_retry: Duration) -> bool {
    match verdict {
        VenvVerdict::Unknown(_) => age < unknown_retry,
        _ => true,
    }
}

/// A verdict plus the time it was recorded.
struct VenvCacheEntry {
    verdict: VenvVerdict,
    recorded: Instant,
}

/// Probe cache. Production uses the `VENV_CACHE` instance; tests construct
/// their own so a fixture interpreter never pollutes the real verdict.
struct VenvCache {
    entry: Mutex<Option<VenvCacheEntry>>,
    /// Serializes probes so a hung interpreter cannot pile up blocking threads
    /// when several callers ask at once.
    probe_guard: Mutex<()>,
}

impl VenvCache {
    const fn new() -> Self {
        Self {
            entry: Mutex::new(None),
            probe_guard: Mutex::new(()),
        }
    }

    fn reset(&self) {
        *lock(&self.entry) = None;
    }

    /// The cached verdict, re-probing when it is missing or expired.
    fn verdict(
        &self,
        python: &Path,
        timeout: Duration,
        unknown_retry: Duration,
    ) -> VenvVerdict {
        let fresh =
            |entry: &VenvCacheEntry| verdict_is_fresh(&entry.verdict, entry.recorded.elapsed(), unknown_retry);
        if let Some(entry) = lock(&self.entry).as_ref() {
            if fresh(entry) {
                return entry.verdict.clone();
            }
        }

        let _probe = lock(&self.probe_guard);
        // Another thread may have completed the probe while this one waited.
        if let Some(entry) = lock(&self.entry).as_ref() {
            if fresh(entry) {
                return entry.verdict.clone();
            }
        }

        let started = Instant::now();
        let verdict = probe_venv_at(python, timeout);
        let elapsed = started.elapsed();

        // Log on transition. A silently cached negative is what made a
        // three-day outage invisible in the log; this line is the diagnosis.
        let previous = lock(&self.entry).as_ref().map(|e| e.verdict.clone());
        if previous.as_ref() != Some(&verdict) {
            match &verdict {
                VenvVerdict::Ready => log::info!("Venv check: ready in {}ms", elapsed.as_millis()),
                VenvVerdict::Stale(reason) => log::info!(
                    "Venv check: usable but stale after {}ms — {} (the next preparation reinstalls from the lock)",
                    elapsed.as_millis(),
                    reason
                ),
                VenvVerdict::Missing => log::info!("Venv check: no cleanup runtime is installed"),
                VenvVerdict::Broken(reason) => log::warn!(
                    "Venv check: broken after {}ms — {} (cleanup disabled until repaired)",
                    elapsed.as_millis(),
                    reason
                ),
                VenvVerdict::Unknown(reason) => log::warn!(
                    "Venv check: inconclusive after {}ms — {} (re-probing in {}s)",
                    elapsed.as_millis(),
                    reason,
                    unknown_retry.as_secs()
                ),
            }
        }

        *lock(&self.entry) = Some(VenvCacheEntry {
            verdict: verdict.clone(),
            recorded: Instant::now(),
        });
        verdict
    }
}

/// Lock helper that tolerates poisoning: a panic while probing must not turn
/// every later readiness check into a panic.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

static VENV_CACHE: VenvCache = VenvCache::new();

/// The hash-locked runtime (P-04), generated from `sidecar/requirements.in`.
/// The probe compares against this embedded copy; installs read the bundled
/// file beside the sidecar script, which must be identical.
const RUNTIME_LOCK: &str = include_str!("../../sidecar/requirements.lock");

/// The four top-level packages the runtime was qualified with. A mismatch
/// here breaks the runtime; any other lock difference only makes it stale.
/// Must match `RUNTIME_PINS` in sidecar/llm_cleanup.py and the lock (tested).
const RUNTIME_PINS: [(&str, &str); 4] = [
    ("mlx", "0.32.2"),
    ("mlx-lm", "0.31.3"),
    ("transformers", "5.3.0"),
    ("huggingface-hub", "1.7.2"),
];

/// Interpreter versions the lock was verified on, newest first.
const SUPPORTED_PYTHON_VERSIONS: [&str; 4] = ["3.14", "3.13", "3.12", "3.11"];

/// Python's distribution-name normalization (PEP 503), for comparing lock
/// names with installed metadata.
fn normalize_package(name: &str) -> String {
    name.to_ascii_lowercase().replace(['_', '.'], "-")
}

/// `(normalized name, version)` of every lock entry without an environment
/// marker: the packages installed on every Mac (`colorama` is Windows-only).
fn locked_versions(lock: &str) -> Vec<(String, &str)> {
    lock.lines()
        .filter(|line| line.starts_with(|c: char| c.is_ascii_alphanumeric()))
        .filter_map(|line| {
            let requirement = line.trim_end_matches('\\').trim_end();
            if requirement.contains(';') {
                return None;
            }
            let (name, version) = requirement.split_once("==")?;
            Some((normalize_package(name), version))
        })
        .collect()
}

/// How long pip may wait on PyPI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PipNetwork {
    /// pip's own timeouts: a setup the user asked for and waits on.
    Patient,
    /// A background sync of a working runtime: give up quickly when PyPI
    /// cannot be reached, because the next preparation retries.
    Brief,
}

/// The `pip` arguments that install exactly the locked, hash-verified wheels
/// and nothing else (P-04). Run with the venv interpreter and `-I`.
fn pip_install_args(lock: &Path, network: PipNetwork) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-m",
        "pip",
        "--isolated",
        "install",
        "--require-hashes",
        "--only-binary",
        ":all:",
        "--no-deps",
        "--disable-pip-version-check",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    if network == PipNetwork::Brief {
        args.extend(["--timeout", "10", "--retries", "1"].map(OsString::from));
    }
    args.extend([OsString::from("-r"), lock.into()]);
    args
}

/// Probe run by the venv interpreter with `-c`. `@LOCK@` becomes a JSON object
/// of every platform-independent lock entry and `@TOP@` the JSON list of the
/// four top-level names.
///
/// **CRITICAL**: This MUST stay a single line of `;`-separated simple
/// statements. A previous multi-line version lost its indentation to Rust's
/// `\` continuation and failed with an IndentationError, which made every
/// probe fail. Conditionals use `x if cond else None`, never `if cond:`.
///
/// CONTRACT with `probe_venv_at`: exit 1 = Python outside 3.11 to 3.14 (or
/// an import error), exit 2 = a top-level pin differs, both `Broken`; exit 3 =
/// only other locked packages differ, `Stale`; exit 0 = `Ready`. Any other
/// outcome is `Unknown`. Changing these codes changes what the app caches.
const VENV_CHECK_SCRIPT: &str = "import sys, re; \
from importlib.metadata import distributions; \
pv = sys.version_info[:2]; \
py = 'Python ' + '.'.join(map(str, pv)); \
supported = (3, 11) <= pv <= (3, 14); \
sys.stderr.write(py + ' is not Python 3.11 to 3.14') if not supported else None; \
sys.exit(1) if not supported else None; \
norm = lambda name: re.sub('[-_.]+', '-', name or '').lower(); \
got = {norm(d.metadata.get('Name')): d.version for d in distributions()}; \
want = @LOCK@; \
top = sorted(n for n in @TOP@ if got.get(n) != want[n]); \
sys.stderr.write(py + ': qualified packages differ from the pins: ' + ', '.join(top)) if top else None; \
sys.exit(2) if top else None; \
import mlx_lm, huggingface_hub; \
stale = sorted(n for n in want if got.get(n) != want[n]); \
sys.stderr.write(py + (': packages differ from the runtime lock: ' + ', '.join(stale) if stale else ': runtime matches the lock')); \
sys.exit(3 if stale else 0)";

fn build_venv_check_script() -> String {
    let lock: BTreeMap<String, &str> = locked_versions(RUNTIME_LOCK).into_iter().collect();
    let top: Vec<&str> = RUNTIME_PINS.iter().map(|(name, _)| *name).collect();
    VENV_CHECK_SCRIPT
        .replace("@LOCK@", &serde_json::to_string(&lock).expect("lock versions serialize"))
        .replace("@TOP@", &serde_json::to_string(&top).expect("pin names serialize"))
}

/// Current runtime verdict, probing only when the cache is missing or expired.
///
/// The single source of truth for "can cleanup run": `spawn()` and every
/// preparation path read it, so they agree on why cleanup is unavailable.
///
/// The cheap existence check (`bin/python3` file present) used to be the only
/// probe, but that masked a common failure mode: the venv's `python3` is a
/// symlink to a system Python that has since been upgraded or removed, which
/// makes mlx_lm imports blow up at runtime. So the probe actually execs the
/// venv's Python with `import mlx_lm` and caches the verdict rather than
/// re-paying the import cost on every call.
pub fn venv_verdict() -> VenvVerdict {
    let python = match venv_python() {
        Ok(python) => python,
        Err(error) => return VenvVerdict::Unknown(error),
    };
    VENV_CACHE.verdict(&python, VENV_PROBE_TIMEOUT, VENV_UNKNOWN_RETRY_INTERVAL)
}

/// Invalidate the cached verdict.
///
/// Call whenever the installed runtime may have changed: after every swap,
/// successful or not, and after recovery restored a runtime. A stale verdict
/// is what previously stranded cleanup after the one in-app lever had
/// already been used.
fn reset_venv_cache() {
    VENV_CACHE.reset();
}

/// Probe a specific interpreter. Separate from `venv_verdict()` so tests can
/// point it at a fixture. `python` is `<venv>/bin/python3`.
///
/// The classification rule is exact: `Broken` requires a definitive answer —
/// either the check script's own exit code, or a runtime without its
/// interpreter. Everything else (spawn failure, timeout, signal-killed child)
/// is `Unknown`, because retrying is harmless while a wrong `Broken` verdict
/// both disables cleanup and misdirects the user toward a repair that cannot
/// help.
pub(crate) fn probe_venv_at(python: &Path, timeout: Duration) -> VenvVerdict {
    if !python.exists() {
        let venv = python.parent().and_then(Path::parent);
        if venv.is_some_and(|venv| venv.symlink_metadata().is_err()) {
            return VenvVerdict::Missing;
        }
        // `exists` follows links, so this also covers a dangling interpreter.
        return VenvVerdict::Broken(format!("{} is missing", python.display()));
    }
    let mut command = match isolated_python(python, SidecarRole::Inference) {
        Ok(command) => command,
        Err(error) => return VenvVerdict::Unknown(error),
    };
    match bounded_command(command.args(["-c", &build_venv_check_script()]), timeout) {
        Ok(out) if out.status.success() => VenvVerdict::Ready,
        Ok(out) => {
            let stderr_line = last_line(&out.stderr);
            let detail = if stderr_line.is_empty() {
                format!("{} reported an unqualified runtime", python.display())
            } else {
                stderr_line
            };
            match out.status.code() {
                Some(1) | Some(2) => VenvVerdict::Broken(detail),
                Some(3) => VenvVerdict::Stale(detail),
                Some(code) => VenvVerdict::Unknown(format!(
                    "{} exited with {code}: {}",
                    python.display(),
                    String::from_utf8_lossy(&out.stderr).trim()
                )),
                None => {
                    VenvVerdict::Unknown(format!("{} was killed by a signal", python.display()))
                }
            }
        }
        Err(error) => VenvVerdict::Unknown(error),
    }
}

/// The last non-empty line of a process stream (progress meters use `\r`).
fn last_line(stream: &[u8]) -> String {
    String::from_utf8_lossy(stream)
        .split(['\n', '\r'])
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or_default()
        .to_string()
}

/// Require the current cached snapshot's tokenizer, config, and every weight shard.
pub fn is_model_downloaded() -> bool {
    let Ok(cache) = model_cache_dir() else {
        return false;
    };
    snapshot_is_complete(&cache.join("snapshots").join(MODEL_REVISION))
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

/// Fixed install locations scanned for a host interpreter, newest version
/// first: Homebrew, /usr/local, MacPorts, the python.org framework and
/// ~/.local/bin. `PATH` is never consulted (P-09); `/usr/bin/python3` is the
/// Xcode 3.9 shim and is not a candidate.
fn host_python_candidates(home: &Path) -> Vec<PathBuf> {
    SUPPORTED_PYTHON_VERSIONS
        .iter()
        .flat_map(|version| {
            let name = format!("python{version}");
            [
                Path::new("/opt/homebrew/bin").join(&name),
                Path::new("/usr/local/bin").join(&name),
                Path::new("/opt/local/bin").join(&name),
                PathBuf::from(format!(
                    "/Library/Frameworks/Python.framework/Versions/{version}/bin"
                ))
                .join(&name),
                home.join(".local").join("bin").join(&name),
            ]
        })
        .collect()
}

/// The first candidate that runs as Python 3.11 to 3.14. `Ok(None)` means no
/// candidate is usable; `Err` means one could not be checked, which a retry
/// may fix and which must not trigger a Python download.
fn find_compatible_python(candidates: &[PathBuf]) -> Result<Option<PathBuf>, String> {
    let mut inconclusive: Option<String> = None;
    for candidate in candidates.iter().filter(|candidate| candidate.is_file()) {
        match probe_python_version(candidate) {
            PythonProbe::Compatible => {
                log::info!("Using {} for the cleanup runtime", candidate.display());
                return Ok(Some(candidate.clone()));
            }
            PythonProbe::Unusable(reason) => {
                log::debug!("Skipping {}: {}", candidate.display(), reason)
            }
            PythonProbe::Inconclusive(reason) => {
                log::warn!("Could not verify {}: {}", candidate.display(), reason);
                inconclusive = Some(reason);
            }
        }
    }
    match inconclusive {
        Some(reason) => Err(format!(
            "Could not check the Python on this Mac ({reason}). Try again."
        )),
        None => Ok(None),
    }
}

/// A host interpreter, else the provisioned one, else a newly provisioned copy.
fn find_or_provision_python() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or("Could not determine home directory")?;
    let root = standalone_python_root()?;
    let mut candidates = host_python_candidates(&home);
    candidates.push(STANDALONE_PYTHON.executable(&root));
    match find_compatible_python(&candidates)? {
        Some(python) => Ok(python),
        None => provision_python(&root, &STANDALONE_PYTHON),
    }
}

/// A pinned python-build-standalone interpreter archive (G-06).
struct StandalonePython<'a> {
    url: &'a str,
    sha256: &'a str,
    /// Directory under the provisioning root; the archive's `python/` is
    /// renamed to it.
    dir_name: &'a str,
    /// Interpreter inside `bin/`.
    executable: &'a str,
}

impl StandalonePython<'_> {
    fn executable(&self, root: &Path) -> PathBuf {
        root.join(self.dir_name).join("bin").join(self.executable)
    }

    /// The download in progress; never a usable file on its own.
    fn archive(&self, root: &Path) -> PathBuf {
        root.join(format!(".{}.tar.gz.partial", self.dir_name))
    }

    /// Where the archive is extracted before its `python/` is renamed.
    fn staging(&self, root: &Path) -> PathBuf {
        root.join(format!(".staging-{}", self.dir_name))
    }
}

/// Delete a partial download or extraction that a quit left behind. Only a
/// preparation downloads Python, and preparations run one at a time.
fn remove_partial_python(root: &Path, python: &StandalonePython) {
    remove_file_if_present(&python.archive(root));
    if let Err(error) = remove_dir_if_present(&python.staging(root)) {
        log::warn!("{error}");
    }
}

/// Release `20261110`, 25,479,437 bytes. Fetched only when no Python 3.11 to
/// 3.14 exists; the hash fixes the archive's contents before extraction.
const STANDALONE_PYTHON: StandalonePython<'static> = StandalonePython {
    url: "https://github.com/astral-sh/python-build-standalone/releases/download/20261110/cpython-3.13.18%2B20261110-aarch64-apple-darwin-install_only_stripped.tar.gz",
    sha256: "7a2384641584ab9984221fcb0fc9265df465922af71be71410529e2a9c3f0540",
    dir_name: "cpython-3.13.18",
    executable: "python3.13",
};

/// Outer bound for the Python download; curl itself fails a transfer slower
/// than 1 KB/s for 30 s.
const PYTHON_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(3600);
const PYTHON_DOWNLOAD_ERROR: &str =
    "Could not download Python for cleanup. Check your internet connection and try again.";

/// Download, verify and unpack `python` under `root`. The download is an
/// owned child process, so quitting the app stops it, and a partial archive
/// is removed so the next attempt starts over.
fn provision_python(root: &Path, python: &StandalonePython) -> Result<PathBuf, String> {
    std::fs::create_dir_all(root)
        .map_err(|e| format!("Could not create {}: {e}", root.display()))?;
    let archive = python.archive(root);
    log::info!("No Python 3.11 to 3.14 found; downloading {}", python.dir_name);
    if let Err(error) = fetch_python_archive(&archive, python.url) {
        remove_file_if_present(&archive);
        return Err(error);
    }
    let installed = install_python_archive(root, &archive, python);
    remove_file_if_present(&archive);
    installed
}

fn fetch_python_archive(archive: &Path, url: &str) -> Result<(), String> {
    let output = bounded_command(
        Command::new("/usr/bin/curl")
            .args([
                "-q",
                "--fail",
                "-L",
                "--proto",
                "=https",
                "--connect-timeout",
                "15",
                "--speed-limit",
                "1024",
                "--speed-time",
                "30",
                "-o",
            ])
            .arg(archive)
            .arg(url),
        PYTHON_DOWNLOAD_TIMEOUT,
    )
    .map_err(|error| {
        log::warn!("Python download did not finish: {error}");
        PYTHON_DOWNLOAD_ERROR.to_string()
    })?;
    if !output.status.success() {
        log::warn!(
            "Python download failed ({}): {}",
            output.status,
            last_line(&output.stderr)
        );
        return Err(PYTHON_DOWNLOAD_ERROR.into());
    }
    Ok(())
}

/// Verify `archive` against the pin, extract it through a staging directory
/// and rename its `python/` into place. A hash mismatch deletes the archive
/// before anything is extracted.
fn install_python_archive(
    root: &Path,
    archive: &Path,
    python: &StandalonePython,
) -> Result<PathBuf, String> {
    let digest = sha256_of(archive)?;
    if digest != python.sha256 {
        log::warn!("Downloaded Python hash {digest} does not match the pin");
        remove_file_if_present(archive);
        return Err("Downloaded Python failed its integrity check and was deleted. Try again.".into());
    }
    let staging = python.staging(root);
    remove_dir_if_present(&staging)?;
    std::fs::create_dir_all(&staging)
        .map_err(|e| format!("Could not create {}: {e}", staging.display()))?;
    let placed = extract_archive(archive, &staging).and_then(|()| {
        let extracted = staging.join("python");
        if !extracted.join("bin").join(python.executable).is_file() {
            return Err("Downloaded Python has an unexpected layout".to_string());
        }
        let target = root.join(python.dir_name);
        remove_dir_if_present(&target)?;
        std::fs::rename(&extracted, &target)
            .map_err(|e| format!("Could not install the downloaded Python: {e}"))
    });
    if let Err(error) = remove_dir_if_present(&staging) {
        log::warn!("{error}");
    }
    placed?;
    let executable = python.executable(root);
    match probe_python_version(&executable) {
        PythonProbe::Compatible => {
            log::info!("Provisioned {}", executable.display());
            Ok(executable)
        }
        PythonProbe::Unusable(reason) | PythonProbe::Inconclusive(reason) => {
            Err(format!("Downloaded Python does not run ({reason}). Try again."))
        }
    }
}

fn sha256_of(path: &Path) -> Result<String, String> {
    let output = bounded_command(
        Command::new("/usr/bin/shasum").args(["-a", "256"]).arg(path),
        Duration::from_secs(60),
    )?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    match stdout.split_whitespace().next() {
        Some(digest) if output.status.success() => Ok(digest.to_ascii_lowercase()),
        _ => Err("Could not check the downloaded Python".into()),
    }
}

fn extract_archive(archive: &Path, destination: &Path) -> Result<(), String> {
    let output = bounded_command(
        Command::new("/usr/bin/tar")
            .arg("-xzf")
            .arg(archive)
            .arg("-C")
            .arg(destination),
        Duration::from_secs(300),
    )?;
    if !output.status.success() {
        log::warn!("Python archive extraction failed: {}", last_line(&output.stderr));
        return Err("Could not unpack the downloaded Python. Try again.".into());
    }
    Ok(())
}

/// What probing a host interpreter for a usable version learned.
#[derive(Clone, Debug, PartialEq, Eq)]
enum PythonProbe {
    /// Runs natively on Apple Silicon and reports a version from 3.11 through
    /// 3.14.
    Compatible,
    /// A definitive answer: an unsupported version, an Intel interpreter
    /// (which cannot install the arm64-only wheels), a file that cannot run,
    /// a nonzero exit, or unrecognized output. It cannot be used.
    Unusable(String),
    /// The probe could not complete (timeout or a transient spawn failure).
    /// MUST NOT be treated as a verdict about the interpreter: reading a
    /// timeout as "unsupported interpreter" is what blocked the only in-app
    /// repair lever with a message about a perfectly healthy Python 3.14.
    Inconclusive(String),
}

/// Prints 1 only for a native arm64 interpreter in the verified range: an
/// Intel Python under Rosetta reports `x86_64`.
const PYTHON_VERSION_CHECK: &str = "import platform, sys; \
print(1 if (3, 11) <= sys.version_info[:2] <= (3, 14) and platform.machine() == 'arm64' else 0)";

/// `EBADARCH`: a binary built for another CPU, such as an Intel Python on a
/// Mac without Rosetta.
#[cfg(target_os = "macos")]
const BAD_CPU_TYPE: Option<i32> = Some(libc::EBADARCH);
#[cfg(not(target_os = "macos"))]
const BAD_CPU_TYPE: Option<i32> = None;

/// Spawn failures that say the file cannot run here, as opposed to a system
/// that was briefly unable to start a process.
fn cannot_execute(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::NotFound
    ) || error
        .raw_os_error()
        .is_some_and(|code| code == libc::ENOEXEC || Some(code) == BAD_CPU_TYPE)
}

/// Probe an interpreter's version.
fn probe_python_version(python: &Path) -> PythonProbe {
    probe_python_version_with(python, PYTHON_VERSION_PROBE_TIMEOUT)
}

/// Probe with an explicit budget so tests can exercise the timeout path.
fn probe_python_version_with(python: &Path, timeout: Duration) -> PythonProbe {
    let out = match bounded_output(
        Command::new(python).args(["-I", "-c", PYTHON_VERSION_CHECK]),
        timeout,
    ) {
        Ok(out) => out,
        Err(BoundedError::Spawn(SpawnError::Os(error))) if cannot_execute(&error) => {
            return PythonProbe::Unusable(format!(
                "{} cannot run on this Mac: {error}",
                python.display()
            ))
        }
        Err(error) => return PythonProbe::Inconclusive(error.to_string()),
    };
    match String::from_utf8_lossy(&out.stdout).trim() {
        "1" => PythonProbe::Compatible,
        "0" => PythonProbe::Unusable(format!(
            "{} is not an arm64 Python 3.11 to 3.14",
            python.display()
        )),
        other => PythonProbe::Unusable(format!(
            "{} reported {other:?} ({}): {}",
            python.display(),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )),
    }
}

/// Budget for creating a venv, including its bundled pip.
const VENV_CREATE_TIMEOUT: Duration = Duration::from_secs(60);
/// Budget for installing the locked wheels.
const PIP_INSTALL_TIMEOUT: Duration = Duration::from_secs(600);

/// The bundled lock beside the sidecar script. It must equal the embedded copy
/// the probe compares against, or every install would read as stale.
fn runtime_lock_path() -> Result<PathBuf, String> {
    let lock = LlmEngine::sidecar_script_path()?.with_file_name("requirements.lock");
    match std::fs::read_to_string(&lock) {
        Ok(contents) if contents == RUNTIME_LOCK => Ok(lock),
        Ok(_) => Err("Cleanup runtime lock does not match this app; reinstall SottoASR".into()),
        Err(_) => Err("Cleanup runtime lock is missing; reinstall SottoASR".into()),
    }
}

/// Install exactly the locked wheels into the venv at `python`.
fn install_lock(python: &Path, lock: &Path, network: PipNetwork) -> Result<(), String> {
    log::info!("Installing the locked cleanup runtime...");
    let output = bounded_command(
        Command::new(python).arg("-I").args(pip_install_args(lock, network)),
        PIP_INSTALL_TIMEOUT,
    )
    .map_err(|error| format!("Could not install the cleanup runtime: {error}"))?;
    if !output.status.success() {
        log::warn!(
            "pip install failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        let detail = last_line(&output.stderr);
        return Err(if detail.is_empty() {
            "Could not install the cleanup runtime. Try again.".into()
        } else {
            format!("Could not install the cleanup runtime: {detail}")
        });
    }
    Ok(())
}

/// Create a venv at `venv` (which must not exist) and install the lock.
fn create_venv_at(
    host_python: &Path,
    venv: &Path,
    lock: &Path,
    network: PipNetwork,
) -> Result<(), String> {
    log::info!("Creating the cleanup runtime with {}", host_python.display());
    let output = bounded_command(
        Command::new(host_python).args(["-I", "-m", "venv"]).arg(venv),
        VENV_CREATE_TIMEOUT,
    )
    .map_err(|error| format!("Could not create the cleanup Python runtime: {error}"))?;
    if !output.status.success() {
        log::warn!(
            "venv creation failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return Err("Could not create the cleanup Python runtime. Try again.".into());
    }
    install_lock(&venv.join("bin").join("python3"), lock, network)
}

fn remove_dir_if_present(path: &Path) -> Result<(), String> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Could not remove {}: {error}", path.display())),
    }
}

fn remove_file_if_present(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => log::warn!("Could not remove {}: {error}", path.display()),
    }
}

fn create_parent(path: &Path) -> Result<(), String> {
    let parent = path.parent().ok_or("Cleanup runtime has no parent directory")?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("Could not create {}: {e}", parent.display()))
}

/// The runtime steps a preparation runs (R-12). Create, sync and Repair all
/// build a new runtime in a staging directory beside the installed one and
/// rename it into place only after its install and probe passed, so a quit
/// at any point leaves either the old runtime or the new one, never a partial
/// one. The old runtime stays aside until the caller keeps the new one.
/// `AppRuntime` works on the app's directories; tests use temporary ones.
pub(crate) trait RuntimeSetup: Clone + Send + Sync + 'static {
    /// Undo what a quit or crash during an earlier preparation left behind.
    fn recover(&self) -> Result<(), String>;
    fn verdict(&self) -> VenvVerdict;
    /// Build and check a new runtime in staging. The installed one is untouched.
    fn build(&self, network: PipNetwork) -> Result<(), String>;
    /// Swap the staged runtime into place, keeping the old one aside.
    fn install_built(&self) -> Result<(), String>;
    /// Keep the installed runtime: delete every aside copy.
    fn keep_installed(&self);
    /// Put the aside copy back in place of the installed runtime.
    fn restore_previous(&self) -> Result<(), String>;
}

/// The cleanup runtime under the app's data directory.
#[derive(Clone, Copy)]
pub(crate) struct AppRuntime;

impl RuntimeSetup for AppRuntime {
    fn recover(&self) -> Result<(), String> {
        let restored = recover_runtime_at(&venv_dir()?)?;
        remove_partial_python(&standalone_python_root()?, &STANDALONE_PYTHON);
        if restored {
            reset_venv_cache();
        }
        Ok(())
    }

    fn verdict(&self) -> VenvVerdict {
        venv_verdict()
    }

    fn build(&self, network: PipNetwork) -> Result<(), String> {
        let venv = venv_dir()?;
        let lock = runtime_lock_path()?;
        let host_python = find_or_provision_python()?;
        build_runtime_at(&staging_path(&venv), &host_python, &lock, network)
    }

    fn install_built(&self) -> Result<(), String> {
        let venv = venv_dir()?;
        let result = install_built_at(&venv, &staging_path(&venv), chrono::Utc::now());
        reset_venv_cache();
        result
    }

    fn keep_installed(&self) {
        match venv_dir() {
            Ok(venv) => sweep_aside_dirs(&venv),
            Err(error) => log::warn!("{error}"),
        }
    }

    fn restore_previous(&self) -> Result<(), String> {
        let result = restore_previous_at(&venv_dir()?);
        reset_venv_cache();
        result
    }
}

fn sibling_name(venv: &Path, suffix: &str) -> String {
    let name = venv
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    format!("{name}{suffix}")
}

/// Where a new runtime is built before it replaces `venv`.
pub(crate) fn staging_path(venv: &Path) -> PathBuf {
    venv.with_file_name(sibling_name(venv, ".staging"))
}

fn aside_prefix(venv: &Path) -> String {
    sibling_name(venv, ".aside-")
}

/// Undo an interrupted preparation. A runtime stranded in an aside directory
/// with nothing in its place moves back first; a staging directory is an
/// unfinished build and is removed. Returns whether a runtime was restored.
pub(crate) fn recover_runtime_at(venv: &Path) -> Result<bool, String> {
    let mut restored = false;
    if venv.symlink_metadata().is_err() {
        if let Some(aside) = newest_aside(venv) {
            log::warn!("Restoring the cleanup runtime an interrupted preparation moved aside");
            std::fs::rename(&aside, venv).map_err(|e| {
                format!(
                    "Could not restore the previous cleanup runtime from {}: {e}",
                    aside.display()
                )
            })?;
            restored = true;
        }
    }
    let staging = staging_path(venv);
    if staging.symlink_metadata().is_ok() {
        log::warn!("Removing an unfinished cleanup runtime build");
        remove_dir_if_present(&staging)?;
    }
    Ok(restored)
}

fn newest_aside(venv: &Path) -> Option<PathBuf> {
    let prefix = aside_prefix(venv);
    std::fs::read_dir(venv.parent()?)
        .ok()?
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .map(|entry| entry.path())
        // The UTC suffix sorts by time.
        .max()
}

/// Build a new runtime in `staging` from `host_python`, install the lock and
/// require a Ready probe. A fresh venv matters: `python -m venv` over an
/// existing directory keeps a dangling interpreter link, and pip leaves
/// satisfied pins untouched. A failure removes `staging`.
pub(crate) fn build_runtime_at(
    staging: &Path,
    host_python: &Path,
    lock: &Path,
    network: PipNetwork,
) -> Result<(), String> {
    remove_dir_if_present(staging)?;
    create_parent(staging)?;
    let built = create_venv_at(host_python, staging, lock, network).and_then(|()| {
        match probe_venv_at(&staging.join("bin").join("python3"), VENV_PROBE_TIMEOUT) {
            VenvVerdict::Ready => Ok(()),
            VenvVerdict::Missing => {
                Err("The new cleanup runtime has no interpreter. Try again.".to_string())
            }
            VenvVerdict::Stale(detail)
            | VenvVerdict::Broken(detail)
            | VenvVerdict::Unknown(detail) => {
                Err(format!("The new cleanup runtime failed its check: {detail}"))
            }
        }
    });
    if built.is_err() {
        if let Err(error) = remove_dir_if_present(staging) {
            log::warn!("{error}");
        }
    }
    built
}

/// Put the staged runtime in place. The installed runtime moves to
/// `<venv>.aside-<UTC>` first and comes back if the rename fails. The aside
/// copy stays until `sweep_aside_dirs` or `restore_previous_at`.
pub(crate) fn install_built_at(
    venv: &Path,
    staging: &Path,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), String> {
    let aside = if venv.symlink_metadata().is_ok() {
        let aside = venv.with_file_name(format!(
            "{}{}",
            aside_prefix(venv),
            now.format("%Y%m%dT%H%M%SZ")
        ));
        std::fs::rename(venv, &aside)
            .map_err(|e| format!("Could not move the old cleanup runtime aside: {e}"))?;
        Some(aside)
    } else {
        None
    };
    if let Err(error) = std::fs::rename(staging, venv) {
        if let Err(removal) = remove_dir_if_present(staging) {
            log::warn!("{removal}");
        }
        let error = format!("Could not put the new cleanup runtime in place: {error}");
        return Err(restore_aside(venv, aside, error));
    }
    log::info!("Cleanup runtime installed");
    Ok(())
}

/// Replace the installed runtime with the newest aside copy: the new runtime
/// did not load, the previous one did.
pub(crate) fn restore_previous_at(venv: &Path) -> Result<(), String> {
    let aside = newest_aside(venv).ok_or("The previous cleanup runtime is gone")?;
    remove_dir_if_present(venv)?;
    std::fs::rename(&aside, venv).map_err(|e| {
        format!(
            "Could not restore the previous cleanup runtime from {}: {e}",
            aside.display()
        )
    })?;
    log::info!("Previous cleanup runtime restored");
    Ok(())
}

/// Delete every aside copy beside `venv`, including the newest one.
pub(crate) fn sweep_aside_dirs(venv: &Path) {
    let Some(parent) = venv.parent() else {
        return;
    };
    let prefix = aside_prefix(venv);
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            if let Err(error) = remove_dir_if_present(&entry.path()) {
                log::warn!("{error}");
            }
        }
    }
}

/// Move the aside copy back after a failed swap and return the error to
/// report, naming the aside copy when it could not be moved back. The next
/// preparation's recovery retries that move.
fn restore_aside(venv: &Path, aside: Option<PathBuf>, error: String) -> String {
    let Some(aside) = aside else {
        return error;
    };
    match std::fs::rename(&aside, venv) {
        Ok(()) => error,
        Err(rename) => {
            log::warn!("Could not restore the previous cleanup runtime: {rename}");
            format!("{error}. The previous runtime was kept at {}", aside.display())
        }
    }
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

/// The backend of the saved cleanup engine. Returns the handle (ownership
/// transferred to the caller — remember to put it back in `state.llm_engine`
/// after use). This is the only place that makes a backend, so with Amazon
/// Bedrock saved nothing starts the sidecar.
///
/// The caller must hold `state.llm_operation` through the returned handle's
/// use and restoration, so lifecycle commands cannot replace it.
pub async fn ensure_running(state: &AppState) -> Result<Box<dyn LlmBackend>, String> {
    let (engine, options) = {
        let settings = state.settings.lock().await;
        (settings.cleanup_engine, settings.aws_bedrock.clone())
    };
    match engine {
        crate::models::CleanupEngine::Local => ensure_local(state).await,
        crate::models::CleanupEngine::AwsBedrock => ensure_bedrock(state, &options).await,
    }
}

/// The Amazon Bedrock backend for `options`. A resident backend for other
/// options, or a resident sidecar, is retired first.
async fn ensure_bedrock(
    state: &AppState,
    options: &crate::models::AwsBedrockSettings,
) -> Result<Box<dyn LlmBackend>, String> {
    let resident = state.llm_engine.lock().await.take();
    if let Some(resident) = resident {
        if resident.bedrock_options() == Some(options) {
            return Ok(resident);
        }
        retire(state, resident).await;
    }
    Ok(Box::new(super::bedrock::BedrockCleanup::connect(options).await?))
}

/// Stop a backend that is out of `state.llm_engine`. For the sidecar this
/// ends the process, which releases the local model.
pub async fn retire(state: &AppState, mut backend: Box<dyn LlmBackend>) {
    let _ = tokio::task::spawn_blocking(move || backend.shutdown()).await;
    state.llm_pid.store(0, Ordering::SeqCst);
    state.llm_loaded.store(false, Ordering::SeqCst);
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
pub async fn ensure_local(state: &AppState) -> Result<Box<dyn LlmBackend>, String> {
    // Fast path — sidecar already running in the guard.
    if let Some(sidecar) = resident_sidecar(state).await {
        state.llm_loaded.store(true, Ordering::SeqCst);
        return Ok(sidecar);
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

/// The resident sidecar, if it is alive. The slot is empty afterwards: a
/// cloud backend that the engine change left there has no process to stop,
/// and a sidecar that exited is of no use.
async fn resident_sidecar(state: &AppState) -> Option<Box<dyn LlmBackend>> {
    let mut resident = state.llm_engine.lock().await.take()?;
    if resident.bedrock_options().is_some() {
        return None;
    }
    if !resident.is_alive() {
        log::warn!("Resident cleanup process exited; starting a fresh process");
        return None;
    }
    Some(resident)
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

/// The single model configuration for SottoASR transcript cleanup.
pub struct ModelConfig {
    pub id: &'static str,
    pub display_name: &'static str,
    pub download_size_mb: u64,
}

/// The cleanup prompt configuration. Two modes (retype / replace) ship as one
/// canonical pair; this hash pins the combination
/// `sha256(canonical(retype) + "\n" + canonical(replace))` over the four
/// consumed fields — the same value the sidecar asserts at module load.
/// Changing either prompt here without updating the sidecar (and vice versa)
/// fails the load handshake and keeps the raw ASR text.
pub const PROMPT_SHA256: &str = "ad24de2a72e4fdaedc2923c3e0d60734e0b53251d4f816834728b323e5b99ed4";
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
        let error = engine.cleanup_batch(&[source.to_string()], CleanupMode::Retype).unwrap_err();
        assert!(is_responded_timeout(&error));
        assert!(!is_zombie_error(&error));
        // The process survived its own deadline and serves the next request.
        let items = engine.cleanup_batch(&[source.to_string()], CleanupMode::Retype).unwrap();
        assert!(matches!(&items[0], BatchItem::Proposal(Some(s)) if s == source));
        assert!(engine.is_alive());

        let mut failing = stub_engine(
            "import json,sys\nfor line in sys.stdin:\n print(json.dumps({'ok':False,'error_code':'operation_failed','error':'Local cleanup runtime failed; it will restart for the next recording. Original text preserved.'}),flush=True)",
        );
        let error = failing.cleanup_batch(&[source.to_string()], CleanupMode::Replace).unwrap_err();
        assert!(!is_responded_timeout(&error));
        assert!(is_zombie_error(&error));
    }

    /// The value of a top-level `NAME = <literal>` assignment in the sidecar.
    fn sidecar_assignment(sidecar: &'static str, name: &str) -> &'static str {
        let prefix = format!("{name} = ");
        sidecar
            .lines()
            .find_map(|line| line.strip_prefix(prefix.as_str()))
            .unwrap_or_else(|| panic!("sidecar has no {name} assignment"))
    }

    /// S-06: the Rust constants, the bundled sidecar and the lock describe one
    /// runtime and one model.
    #[test]
    fn lock_sidecar_and_rust_pins_agree() {
        let sidecar = include_str!("../../sidecar/llm_cleanup.py");
        let quoted = |name: &str| -> String {
            serde_json::from_str(sidecar_assignment(sidecar, name)).unwrap()
        };
        assert_eq!(quoted("MODEL_ID"), SOTTO_MODEL.id);
        assert_eq!(quoted("MODEL_NAME"), SOTTO_MODEL.display_name);
        assert_eq!(quoted("MODEL_REVISION"), MODEL_REVISION);
        assert_eq!(quoted("PROMPT_SHA256"), PROMPT_SHA256);

        let sidecar_pins: BTreeMap<String, String> =
            serde_json::from_str(sidecar_assignment(sidecar, "RUNTIME_PINS")).unwrap();
        let rust_pins: BTreeMap<String, String> = RUNTIME_PINS
            .iter()
            .map(|(name, version)| (name.to_string(), version.to_string()))
            .collect();
        assert_eq!(sidecar_pins, rust_pins);
        let locked: BTreeMap<String, &str> = locked_versions(RUNTIME_LOCK).into_iter().collect();
        for (name, version) in RUNTIME_PINS {
            assert_eq!(locked.get(name).copied(), Some(version), "{name} differs from the lock");
        }

        assert!(validate_loaded_model(&serde_json::json!({"ok":true})).is_err());
        assert!(validate_loaded_model(&serde_json::json!({"ok":true,"model_id":SOTTO_MODEL.id,"revision":MODEL_REVISION,"prompt_sha256":PROMPT_SHA256,"warmed":true})).is_ok());
    }

    /// P-04: every lock entry is pinned by hash, and the install accepts
    /// nothing else.
    #[test]
    fn every_lock_entry_carries_a_hash_and_the_install_requires_hashes() {
        let lines: Vec<&str> = RUNTIME_LOCK.lines().collect();
        let mut entries = 0;
        for (index, line) in lines.iter().enumerate() {
            if !line.starts_with(|c: char| c.is_ascii_alphanumeric()) {
                continue;
            }
            entries += 1;
            assert!(line.contains("=="), "{line} is not pinned");
            let hashes = lines[index + 1..]
                .iter()
                .take_while(|next| next.starts_with(' '))
                .filter(|next| {
                    next.trim()
                        .trim_end_matches('\\')
                        .trim()
                        .strip_prefix("--hash=sha256:")
                        .is_some_and(|hex| hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()))
                })
                .count();
            assert!(hashes > 0, "{line} carries no hash");
        }
        assert_eq!(entries, 35);
        assert_eq!(locked_versions(RUNTIME_LOCK).len(), 34, "only colorama carries a marker");

        let args = |network| -> Vec<String> {
            pip_install_args(Path::new("/bundle/requirements.lock"), network)
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(
            args(PipNetwork::Patient),
            [
                "-m",
                "pip",
                "--isolated",
                "install",
                "--require-hashes",
                "--only-binary",
                ":all:",
                "--no-deps",
                "--disable-pip-version-check",
                "-r",
                "/bundle/requirements.lock",
            ]
        );
        // The background sync only shortens pip's network patience.
        let brief = args(PipNetwork::Brief);
        assert_eq!(&brief[..9], &args(PipNetwork::Patient)[..9]);
        assert_eq!(
            &brief[9..],
            ["--timeout", "10", "--retries", "1", "-r", "/bundle/requirements.lock"]
        );
    }

    /// R-07: no total deadline, but a cache that stops growing ends the
    /// download and its process. The clock is injected: each poll is 30 s.
    #[test]
    fn stalled_download_fails_after_the_stall_limit() {
        let mut engine = stub_engine("import time; time.sleep(20)");
        let start = Instant::now();
        let mut now = start;
        let mut watch =
            crate::llm::download::DownloadStallWatch::new(1_000, start, Duration::from_secs(120));
        let mut polls = 0;
        let error = engine
            .download_model(Duration::from_millis(5), || {
                polls += 1;
                now += Duration::from_secs(30);
                watch.observe(1_000, now)
            })
            .unwrap_err();
        assert!(error.contains("120 seconds"), "{error}");
        assert_eq!(polls, 4);
        assert!(
            engine.child.lock().try_wait().unwrap().is_some(),
            "the stalled download process is stopped"
        );
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

    /// A real interpreter for tests that run Python code. They fail instead of
    /// passing silently when there is none.
    fn test_interpreter() -> PathBuf {
        let home = dirs::home_dir().expect("home directory");
        std::iter::once(PathBuf::from("python3"))
            .chain(host_python_candidates(&home))
            .find(|candidate| probe_python_version(candidate) == PythonProbe::Compatible)
            .expect(
                "this test needs an arm64 Python 3.11 to 3.14 as python3 on PATH or in \
                 Homebrew, MacPorts, the python.org framework or ~/.local/bin",
            )
    }

    /// The host probe's own decision: the real check script runs on a real
    /// interpreter that reports the fixture's machine type and version.
    #[test]
    fn python_probe_accepts_only_a_native_arm64_interpreter() {
        let real = test_interpreter();
        let directory = tempfile::TempDir::new().unwrap();
        let reporting = |name: &str, machine: &str, version: &str| {
            interpreter_fixture(
                &directory,
                name,
                &format!(
                    "exec '{}' -I - \"$3\" <<'PROBE'\nimport platform, sys\n\
                     sys.version_info = {version}\nplatform.machine = lambda: '{machine}'\n\
                     exec(sys.argv[1])\nPROBE",
                    real.display()
                ),
            )
        };
        let native = probe_python_version(&reporting("native", "arm64", "(3, 13, 0, 'final', 0)"));
        assert_eq!(native, PythonProbe::Compatible);
        for (name, machine, version) in [
            ("rosetta", "x86_64", "(3, 13, 0, 'final', 0)"),
            ("too-new", "arm64", "(3, 15, 0, 'final', 0)"),
        ] {
            let probe = probe_python_version(&reporting(name, machine, version));
            assert!(
                matches!(&probe, PythonProbe::Unusable(reason) if reason.contains("not an arm64 Python 3.11 to 3.14")),
                "{name}: {probe:?}"
            );
        }
    }

    /// The real probe script against a venv whose site-packages carries the
    /// lock's metadata and empty `mlx_lm` / `huggingface_hub` modules:
    /// matching versions are Ready, a floated transitive version is Stale and
    /// keeps cleanup running, a floated top-level pin is Broken.
    #[test]
    fn a_runtime_with_a_floated_transitive_version_is_reported_stale() {
        let base = test_interpreter();
        let directory = tempfile::TempDir::new().unwrap();
        let venv = directory.path().join("llm-venv");
        let created = Command::new(&base)
            .args(["-I", "-m", "venv", "--without-pip"])
            .arg(&venv)
            .status()
            .unwrap();
        assert!(created.success(), "{} could not create a venv", base.display());
        let python = venv.join("bin").join("python3");
        let site = std::fs::read_dir(venv.join("lib"))
            .unwrap()
            .flatten()
            .map(|entry| entry.path().join("site-packages"))
            .find(|path| path.is_dir())
            .expect("venv site-packages");
        let write_dist = |name: &str, version: &str| {
            let dist = site.join(format!("{}.dist-info", name.replace('-', "_")));
            std::fs::create_dir_all(&dist).unwrap();
            std::fs::write(
                dist.join("METADATA"),
                format!("Metadata-Version: 2.1\nName: {name}\nVersion: {version}\n"),
            )
            .unwrap();
        };
        for (name, version) in locked_versions(RUNTIME_LOCK) {
            write_dist(&name, version);
        }
        for module in ["mlx_lm", "huggingface_hub"] {
            std::fs::create_dir_all(site.join(module)).unwrap();
            std::fs::write(site.join(module).join("__init__.py"), "").unwrap();
        }
        assert_eq!(probe_venv_at(&python, Duration::from_secs(10)), VenvVerdict::Ready);

        write_dist("certifi", "2025.1.31");
        let stale = probe_venv_at(&python, Duration::from_secs(10));
        assert!(
            matches!(&stale, VenvVerdict::Stale(reason) if reason.contains("certifi")),
            "{stale:?}"
        );
        assert!(stale.is_ready(), "a stale runtime keeps cleanup running");

        write_dist("transformers", "5.2.0");
        let broken = probe_venv_at(&python, Duration::from_secs(10));
        assert!(
            matches!(&broken, VenvVerdict::Broken(reason) if reason.contains("transformers")),
            "{broken:?}"
        );
    }

    /// Write an executable shell fixture standing in for a Python interpreter.
    fn interpreter_fixture(
        directory: &tempfile::TempDir,
        name: &str,
        body: &str,
    ) -> std::path::PathBuf {
        let path = directory.path().join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write fixture");
        set_fixture_mode(&path, 0o755);
        path
    }

    fn set_fixture_mode(path: &std::path::Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod fixture");
    }

    /// One byte per invocation, so a test can prove whether the probe re-ran.
    fn invocation_count(counter: &std::path::Path) -> usize {
        std::fs::read_to_string(counter).map(|s| s.len()).unwrap_or(0)
    }

    #[test]
    fn probe_classifies_the_check_scripts_own_verdicts() {
        let directory = tempfile::TempDir::new().unwrap();
        for (name, code, stderr) in [
            ("old-python", 1, "Python 3.9 is not Python 3.11 to 3.14"),
            (
                "pin-mismatch",
                2,
                "Python 3.14: qualified packages differ from the pins: mlx-lm",
            ),
            (
                "stale",
                3,
                "Python 3.13: packages differ from the runtime lock: certifi",
            ),
        ] {
            let fixture = interpreter_fixture(
                &directory,
                name,
                &format!("echo '{stderr}' >&2\nexit {code}"),
            );
            let expected = if code == 3 {
                VenvVerdict::Stale(stderr.into())
            } else {
                VenvVerdict::Broken(stderr.into())
            };
            assert_eq!(probe_venv_at(&fixture, Duration::from_secs(5)), expected, "exit {code}");
        }
    }

    #[test]
    fn probe_never_treats_an_unanswered_run_as_a_verdict() {
        let directory = tempfile::TempDir::new().unwrap();
        // A timeout says nothing about the runtime, so it may not be cached as
        // Broken. The budget only has to be shorter than the fixture's sleep.
        let slow = interpreter_fixture(&directory, "slow", "sleep 30");
        assert!(matches!(
            probe_venv_at(&slow, Duration::from_millis(200)),
            VenvVerdict::Unknown(_)
        ));

        // A signal-killed child is not a verdict either, and is distinguishable
        // from a timeout in the reason.
        let killed = interpreter_fixture(&directory, "killed", "kill -9 $$");
        match probe_venv_at(&killed, Duration::from_secs(5)) {
            VenvVerdict::Unknown(reason) => assert!(reason.contains("signal"), "{reason}"),
            other => panic!("a signal-killed probe is not a verdict: {other:?}"),
        }
    }

    #[test]
    fn probe_separates_a_missing_runtime_from_a_broken_one() {
        let directory = tempfile::TempDir::new().unwrap();
        let python = directory.path().join("llm-venv").join("bin").join("python3");
        assert_eq!(probe_venv_at(&python, Duration::from_secs(5)), VenvVerdict::Missing);
        std::fs::create_dir_all(python.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(directory.path().join("removed-python"), &python).unwrap();
        assert!(
            matches!(probe_venv_at(&python, Duration::from_secs(5)), VenvVerdict::Broken(_)),
            "a dangling interpreter link needs a repair"
        );
    }

    #[test]
    fn only_an_unknown_verdict_expires() {
        let retry = Duration::from_secs(15);
        let day = Duration::from_secs(86_400);
        assert!(verdict_is_fresh(&VenvVerdict::Ready, day, retry));
        assert!(verdict_is_fresh(&VenvVerdict::Stale("certifi".into()), day, retry));
        assert!(verdict_is_fresh(&VenvVerdict::Missing, day, retry));
        assert!(verdict_is_fresh(
            &VenvVerdict::Broken("old python".into()),
            day,
            retry
        ));
        assert!(verdict_is_fresh(
            &VenvVerdict::Unknown("timed out".into()),
            Duration::from_secs(1),
            retry
        ));
        assert!(!verdict_is_fresh(
            &VenvVerdict::Unknown("timed out".into()),
            retry,
            retry
        ));
    }

    /// The regression behind the outage: one probe timeout was cached as a
    /// verdict, so every later cleanup short-circuited to Unavailable for the
    /// life of the process. A menu-bar app runs for days.
    ///
    /// The first verdict comes from a spawn failure rather than a timeout, so
    /// the test has no timing dependency.
    #[test]
    fn an_expired_unknown_verdict_reprobes_and_recovers() {
        let directory = tempfile::TempDir::new().unwrap();
        let fixture = interpreter_fixture(&directory, "interpreter", "exit 0");
        set_fixture_mode(&fixture, 0o644);
        let cache = VenvCache::new();

        assert!(matches!(
            cache.verdict(&fixture, Duration::from_secs(5), Duration::ZERO),
            VenvVerdict::Unknown(_)
        ));
        set_fixture_mode(&fixture, 0o755);
        assert!(
            cache.verdict(&fixture, Duration::from_secs(5), Duration::ZERO).is_ready(),
            "an expired Unknown verdict must re-probe instead of staying false forever"
        );
    }

    #[test]
    fn a_fresh_unknown_verdict_does_not_reprobe() {
        let directory = tempfile::TempDir::new().unwrap();
        let fixture = interpreter_fixture(&directory, "interpreter", "exit 0");
        set_fixture_mode(&fixture, 0o644);
        let cache = VenvCache::new();
        let retry = Duration::from_secs(60);

        assert!(matches!(
            cache.verdict(&fixture, Duration::from_secs(5), retry),
            VenvVerdict::Unknown(_)
        ));
        // The interpreter becomes usable, but the cached Unknown is still fresh.
        set_fixture_mode(&fixture, 0o755);
        assert!(matches!(
            cache.verdict(&fixture, Duration::from_secs(5), retry),
            VenvVerdict::Unknown(_)
        ));
    }

    #[test]
    fn a_broken_verdict_is_cached_without_reprobing() {
        let directory = tempfile::TempDir::new().unwrap();
        let counter = directory.path().join("invocations");
        let fixture = interpreter_fixture(
            &directory,
            "broken",
            &format!("printf x >> '{}'\nexit 2", counter.display()),
        );
        let cache = VenvCache::new();
        for _ in 0..3 {
            assert!(matches!(
                cache.verdict(&fixture, Duration::from_secs(5), Duration::ZERO),
                VenvVerdict::Broken(_)
            ));
        }
        assert_eq!(
            invocation_count(&counter),
            1,
            "a deterministic verdict must be probed once, not every call"
        );
    }

    #[test]
    fn resetting_the_cache_forces_a_fresh_probe() {
        let directory = tempfile::TempDir::new().unwrap();
        let counter = directory.path().join("invocations");
        let fixture = interpreter_fixture(
            &directory,
            "broken",
            &format!("printf x >> '{}'\nexit 2", counter.display()),
        );
        let cache = VenvCache::new();
        assert!(matches!(
            cache.verdict(&fixture, Duration::from_secs(5), Duration::ZERO),
            VenvVerdict::Broken(_)
        ));
        cache.reset();
        assert!(matches!(
            cache.verdict(&fixture, Duration::from_secs(5), Duration::ZERO),
            VenvVerdict::Broken(_)
        ));
        assert_eq!(invocation_count(&counter), 2);
    }

    /// Stand-in host interpreter: `-I -m venv <dir>` creates
    /// `<dir>/bin/python3` as a copy of `venv_python`.
    fn host_python_fixture(
        directory: &tempfile::TempDir,
        name: &str,
        venv_python: &Path,
    ) -> PathBuf {
        interpreter_fixture(
            directory,
            name,
            &format!(
                "mkdir -p \"$4/bin\" && cp '{}' \"$4/bin/python3\"",
                venv_python.display()
            ),
        )
    }

    /// R-12: `python -m venv` over the old directory kept the dangling link.
    /// A rebuild makes a fresh venv in staging, installs the lock there and
    /// passes the probe; only then does it replace the runtime and sweep
    /// every aside copy.
    #[test]
    fn repair_replaces_a_venv_whose_interpreter_link_dangles() {
        let directory = tempfile::TempDir::new().unwrap();
        let pip_args = directory.path().join("pip-args");
        let pip = interpreter_fixture(
            &directory,
            "pip-ok",
            &format!(
                "[ \"$2\" = \"-c\" ] && exit 0\nprintf '%s\\n' \"$@\" > '{}'",
                pip_args.display()
            ),
        );
        let host = host_python_fixture(&directory, "host", &pip);
        let lock = directory.path().join("requirements.lock");
        std::fs::write(&lock, RUNTIME_LOCK).unwrap();
        let root = directory.path().join("support");
        let venv = root.join("llm-venv");
        let staging = staging_path(&venv);
        let interpreter = venv.join("bin").join("python3");
        std::fs::create_dir_all(interpreter.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(root.join("upgraded-away").join("python3"), &interpreter)
            .unwrap();
        std::fs::create_dir_all(root.join("llm-venv.aside-20260101T000000Z")).unwrap();
        assert!(matches!(
            probe_venv_at(&interpreter, Duration::from_secs(5)),
            VenvVerdict::Broken(_)
        ));

        build_runtime_at(&staging, &host, &lock, PipNetwork::Patient).unwrap();
        assert!(
            !interpreter.exists(),
            "the installed runtime is untouched until the swap"
        );
        install_built_at(&venv, &staging, chrono::Utc::now()).unwrap();

        assert!(interpreter.is_file(), "the rebuilt interpreter resolves");
        assert!(!staging.exists());
        let recorded = std::fs::read_to_string(&pip_args).unwrap();
        let recorded: Vec<&str> = recorded.lines().collect();
        let expected: Vec<String> = std::iter::once("-I".to_string())
            .chain(
                pip_install_args(&lock, PipNetwork::Patient)
                    .iter()
                    .map(|arg| arg.to_string_lossy().into_owned()),
            )
            .collect();
        assert_eq!(recorded, expected);
        let asides = || -> Vec<_> {
            std::fs::read_dir(&root)
                .unwrap()
                .flatten()
                .map(|entry| entry.file_name())
                .filter(|name| name.to_string_lossy().contains(".aside-"))
                .collect()
        };
        assert_eq!(asides().len(), 2, "the old runtime waits aside until it is let go");
        sweep_aside_dirs(&venv);
        assert!(asides().is_empty(), "{:?}", asides());
    }

    /// A sync whose new runtime does not load goes back to the old one.
    #[test]
    fn an_installed_runtime_can_give_way_to_the_previous_one() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path().join("support");
        let venv = root.join("llm-venv");
        let staging = staging_path(&venv);
        std::fs::create_dir_all(&venv).unwrap();
        std::fs::write(venv.join("marker"), "previous runtime").unwrap();
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("marker"), "synced runtime").unwrap();

        install_built_at(&venv, &staging, chrono::Utc::now()).unwrap();
        assert_eq!(std::fs::read_to_string(venv.join("marker")).unwrap(), "synced runtime");
        restore_previous_at(&venv).unwrap();

        assert_eq!(std::fs::read_to_string(venv.join("marker")).unwrap(), "previous runtime");
        assert_eq!(
            std::fs::read_dir(&root).unwrap().count(),
            1,
            "no aside copy or staging directory is left"
        );
    }

    #[test]
    fn a_failed_build_leaves_the_installed_runtime_untouched() {
        let directory = tempfile::TempDir::new().unwrap();
        let pip = interpreter_fixture(
            &directory,
            "pip-offline",
            "[ \"$2\" = \"-c\" ] && exit 0\necho 'ERROR: network is unreachable' >&2\nexit 1",
        );
        let host = host_python_fixture(&directory, "host", &pip);
        let lock = directory.path().join("requirements.lock");
        std::fs::write(&lock, RUNTIME_LOCK).unwrap();
        let root = directory.path().join("support");
        let venv = root.join("llm-venv");
        std::fs::create_dir_all(venv.join("bin")).unwrap();
        std::fs::write(venv.join("marker"), "previous runtime").unwrap();

        let error =
            build_runtime_at(&staging_path(&venv), &host, &lock, PipNetwork::Brief).unwrap_err();

        assert!(error.contains("network is unreachable"), "{error}");
        assert_eq!(
            std::fs::read_to_string(venv.join("marker")).unwrap(),
            "previous runtime"
        );
        assert_eq!(
            std::fs::read_dir(&root).unwrap().count(),
            1,
            "the failed build leaves no staging or aside directory"
        );
    }

    /// A quit between moving the runtime aside and renaming the new one in
    /// strands the runtime; the next preparation moves it back and drops the
    /// unfinished build and any partial Python download.
    #[test]
    fn recovery_restores_a_stranded_runtime_and_drops_unfinished_work() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path().join("support");
        let venv = root.join("llm-venv");
        let older = root.join("llm-venv.aside-20260101T000000Z");
        let newest = root.join("llm-venv.aside-20261119T101500Z");
        std::fs::create_dir_all(&older).unwrap();
        std::fs::create_dir_all(&newest).unwrap();
        std::fs::write(newest.join("marker"), "newest runtime").unwrap();
        std::fs::create_dir_all(staging_path(&venv).join("bin")).unwrap();

        assert!(recover_runtime_at(&venv).unwrap());
        assert_eq!(
            std::fs::read_to_string(venv.join("marker")).unwrap(),
            "newest runtime"
        );
        assert!(!staging_path(&venv).exists());
        assert!(
            !recover_runtime_at(&venv).unwrap(),
            "an installed runtime is left alone"
        );

        let python_root = directory.path().join("python");
        let python = test_python("unused");
        std::fs::create_dir_all(python.staging(&python_root).join("python")).unwrap();
        std::fs::write(python.archive(&python_root), b"partial").unwrap();
        remove_partial_python(&python_root, &python);
        assert!(!python.archive(&python_root).exists());
        assert!(!python.staging(&python_root).exists());
    }

    fn test_python(sha256: &str) -> StandalonePython<'_> {
        StandalonePython {
            url: "https://example.invalid/unused",
            sha256,
            dir_name: "cpython-test",
            executable: "python3.13",
        }
    }

    /// G-06: the provisioned archive is extracted only after its hash
    /// matches, through a staging directory; a mismatch deletes the download.
    #[test]
    fn standalone_python_installs_only_a_hash_matched_archive() {
        let directory = tempfile::TempDir::new().unwrap();
        let source = directory.path().join("source");
        let packaged = source.join("python").join("bin").join("python3.13");
        std::fs::create_dir_all(packaged.parent().unwrap()).unwrap();
        std::fs::write(&packaged, "#!/bin/sh\necho 1\n").unwrap();
        set_fixture_mode(&packaged, 0o755);
        let root = directory.path().join("python");
        std::fs::create_dir_all(&root).unwrap();
        let archive = root.join(".cpython-test.tar.gz.partial");
        // Pack once and reuse the bytes: gzip stamps the current time, so two
        // packs that straddle a second boundary have different digests.
        let packed = Command::new("/usr/bin/tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&source)
            .arg("python")
            .status()
            .unwrap();
        assert!(packed.success());
        let bytes = std::fs::read(&archive).unwrap();
        let digest = sha256_of(&archive).unwrap();

        let error = install_python_archive(&root, &archive, &test_python(&"0".repeat(64)))
            .unwrap_err();
        assert!(error.contains("integrity check"), "{error}");
        assert!(!archive.exists(), "a mismatched download is deleted");
        assert!(!root.join("cpython-test").exists(), "nothing is extracted");

        std::fs::write(&archive, &bytes).unwrap();
        let installed = install_python_archive(&root, &archive, &test_python(&digest)).unwrap();
        assert_eq!(installed, root.join("cpython-test").join("bin").join("python3.13"));
        assert!(installed.is_file());
        assert!(!root.join(".staging-cpython-test").exists(), "staging is removed");
    }

    #[test]
    fn python_probe_distinguishes_unusable_from_inconclusive() {
        let directory = tempfile::TempDir::new().unwrap();
        // The budget is per case: the fast fixtures only need enough room for
        // /bin/sh to start under a loaded test runner.
        let probe = |name: &str, body: &str, timeout: Duration| {
            probe_python_version_with(&interpreter_fixture(&directory, name, body), timeout)
        };
        let generous = Duration::from_secs(5);
        assert_eq!(probe("new", "echo 1", generous), PythonProbe::Compatible);
        let old = probe("old", "echo 0", generous);
        assert!(
            matches!(&old, PythonProbe::Unusable(reason) if reason.contains("not an arm64 Python 3.11 to 3.14")),
            "{old:?}"
        );
        // A file that cannot be executed is a verdict, not a transient failure.
        let unrunnable = interpreter_fixture(&directory, "not-executable", "echo 1");
        set_fixture_mode(&unrunnable, 0o644);
        let refused = probe_python_version_with(&unrunnable, generous);
        assert!(
            matches!(&refused, PythonProbe::Unusable(reason) if reason.contains("cannot run")),
            "{refused:?}"
        );
        assert!(matches!(
            probe("slow", "sleep 30", Duration::from_millis(200)),
            PythonProbe::Inconclusive(_)
        ));
    }

    /// The old text pointed every failure at a model download, which is what
    /// made the outage unactionable: the model was present and verified.
    #[test]
    fn unavailable_reasons_name_the_real_recovery() {
        let broken =
            VenvVerdict::Broken("python3 is not Python 3.11 to 3.14".into()).unavailable_reason();
        assert!(broken.contains("repair"), "{broken}");
        assert!(!broken.contains("Download the model"), "{broken}");

        let missing = VenvVerdict::Missing.unavailable_reason();
        assert!(missing.contains("not installed"), "{missing}");

        let unknown =
            VenvVerdict::Unknown("Child process timed out after 20 seconds".into())
                .unavailable_reason();
        assert!(unknown.contains("retries"), "{unknown}");
        assert!(!unknown.contains("Download the model"), "{unknown}");
    }

    fn cleanup_state(settings: crate::models::Settings, resident: Box<dyn LlmBackend>) -> AppState {
        use crate::test_support::{MockAsrEngine, MockAudioCapture, MockPasteBackend};
        AppState::new_with_backends(
            Box::new(MockAudioCapture::sine_wave()),
            Box::new(MockAsrEngine::with_text("unused")),
            Some(resident),
            Box::new(MockPasteBackend::new()),
            settings,
        )
    }

    #[tokio::test]
    async fn the_cloud_engine_keeps_its_backend_and_never_uses_a_sidecar() {
        use crate::models::{AwsBedrockSettings, BedrockModel, CleanupEngine, Settings};
        use crate::test_support::{CloudCleanupStandIn, StoppableSidecar};
        // No machine has this profile, so a new backend cannot be made here.
        let options = AwsBedrockSettings {
            profile: Some("sottoasr-test-profile-that-is-not-defined".into()),
            region: Some("us-west-2".into()),
            model: BedrockModel::Haiku55,
        };
        let settings = Settings { llm_cleanup_enabled: true, cleanup_engine: CleanupEngine::AwsBedrock, aws_bedrock: options.clone(), ..Default::default() };
        let state = cleanup_state(settings, Box::new(CloudCleanupStandIn { options: options.clone(), ..Default::default() }));

        // The backend for the saved options is used again.
        let backend = ensure_running(&state).await.unwrap();
        assert_eq!(backend.bedrock_options(), Some(&options));

        // Other options: the old backend goes, and a new one is made.
        *state.llm_engine.lock().await = Some(backend);
        state.settings.lock().await.aws_bedrock.model = BedrockModel::Opus55;
        assert!(ensure_running(&state).await.is_err(), "the profile of the new backend is not in the AWS files");
        assert!(state.llm_engine.lock().await.is_none());

        // A resident sidecar is stopped, not used.
        let sidecar = StoppableSidecar::default();
        let (stopped, batches) = (sidecar.stopped.clone(), sidecar.batches.clone());
        *state.llm_engine.lock().await = Some(Box::new(sidecar));
        state.llm_loaded.store(true, Ordering::SeqCst);
        assert!(ensure_running(&state).await.is_err());
        assert!(stopped.load(Ordering::SeqCst), "the sidecar process ends");
        assert!(!state.llm_loaded.load(Ordering::SeqCst));
        assert_eq!(batches.load(Ordering::SeqCst), 0, "no text went to the local model");
    }

    #[tokio::test]
    async fn the_local_engine_never_gets_a_cloud_backend() {
        use crate::models::Settings;
        use crate::test_support::{CloudCleanupStandIn, StoppableSidecar};
        let cloud = CloudCleanupStandIn::default();
        let state = cleanup_state(Settings::default(), Box::new(cloud));
        assert!(resident_sidecar(&state).await.is_none());
        assert!(state.llm_engine.lock().await.is_none(), "the cloud backend is not kept for the local engine");

        // A live sidecar is the resident handle, as before.
        *state.llm_engine.lock().await = Some(Box::new(StoppableSidecar::default()));
        let sidecar = ensure_running(&state).await.unwrap();
        assert!(sidecar.bedrock_options().is_none());
        assert!(state.llm_loaded.load(Ordering::SeqCst));
    }
}
