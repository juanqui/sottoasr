//! Minimal, exclusively owned bridge to FluidAudio's batch Parakeet API.
//! See ../README.md for the pinned SDK, model revisions and upstream provenance.

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::Path;
use std::ptr::NonNull;

/// SHA-256 manifest of every file the bridge loads from the pinned model
/// revisions. Regenerate it with `scripts/check-model-manifest.py --write`.
pub const MODEL_MANIFEST: &str = include_str!("../model-manifest.json");

extern "C" {
    fn sotto_asr_create() -> *mut c_void;
    fn sotto_asr_destroy(handle: *mut c_void);
    fn sotto_asr_download(directory: *const c_char, error: *mut *mut c_char) -> i32;
    fn sotto_asr_initialize(handle: *mut c_void, directory: *const c_char, error: *mut *mut c_char) -> i32;
    fn sotto_asr_transcribe(
        handle: *mut c_void,
        path: *const c_char,
        terms: *const c_char,
        text: *mut *mut c_char,
        candidates: *mut *mut c_char,
        duration: *mut f64,
        processing_time: *mut f64,
        error: *mut *mut c_char,
    ) -> i32;
    fn sotto_vocabulary_download(directory: *const c_char, fetch_models: bool, error: *mut *mut c_char) -> i32;
    fn sotto_vocabulary_prepare(directory: *const c_char, error: *mut *mut c_char) -> *mut c_void;
    fn sotto_vocabulary_destroy(handle: *mut c_void);
    fn sotto_asr_attach_vocabulary(handle: *mut c_void, vocabulary: *mut c_void);
    fn sotto_asr_unload_vocabulary(handle: *mut c_void);
    fn sotto_sha256_file(path: *const c_char, digest: *mut *mut c_char, error: *mut *mut c_char) -> i32;
    fn sotto_asr_free_string(value: *mut c_char);
}

/// Fetches missing speech model files into `directory` at the pinned revision.
/// Present files are never replaced and nothing is loaded.
pub fn download_speech_model(directory: &Path) -> Result<(), String> {
    let directory = c_path(directory)?;
    let mut error = std::ptr::null_mut();
    // SAFETY: the path lives through this synchronous call; output is writable.
    let status = unsafe { sotto_asr_download(directory.as_ptr(), &mut error) };
    check_status(status, error)
}

/// Repairs an unreadable vocabulary tokenizer and, when `fetch_models` is set,
/// fetches missing vocabulary model files into `directory` at the pinned
/// revision. Nothing is loaded.
pub fn download_vocabulary_model(directory: &Path, fetch_models: bool) -> Result<(), String> {
    let directory = c_path(directory)?;
    let mut error = std::ptr::null_mut();
    // SAFETY: the path lives through this synchronous call; output is writable.
    let status = unsafe { sotto_vocabulary_download(directory.as_ptr(), fetch_models, &mut error) };
    check_status(status, error)
}

/// Lowercase hexadecimal SHA-256 of the file's contents, read in chunks.
pub fn sha256_file(path: &Path) -> Result<String, String> {
    let path = c_path(path)?;
    let mut digest = std::ptr::null_mut();
    let mut error = std::ptr::null_mut();
    // SAFETY: the path lives through this synchronous call; outputs are writable.
    let status = unsafe { sotto_sha256_file(path.as_ptr(), &mut digest, &mut error) };
    let digest = take_string(digest);
    check_status(status, error)?;
    Ok(digest)
}

pub struct FluidAudio {
    handle: NonNull<c_void>,
    ready: bool,
}

// SAFETY: the retained Swift handle has no thread affinity. Rust's &mut self
// API serializes all access, and each Swift async operation finishes before
// returning. The handle is never exposed or shared with another owner.
unsafe impl Send for FluidAudio {}

pub struct VocabularyModel {
    handle: NonNull<c_void>,
}

// SAFETY: detached immutable Swift model resources have no thread affinity.
// Their retained handle is either dropped or exclusively transferred to ASR.
unsafe impl Send for VocabularyModel {}

impl VocabularyModel {
    /// Loads verified files from `directory`. Never downloads.
    pub fn load(directory: &Path) -> Result<Self, String> {
        let directory = c_path(directory)?;
        let mut error = std::ptr::null_mut();
        // SAFETY: a new independent retained handle; the path lives through
        // this synchronous call and output storage is valid.
        let handle = NonNull::new(unsafe { sotto_vocabulary_prepare(directory.as_ptr(), &mut error) });
        check_status(if handle.is_some() { 0 } else { -1 }, error)?;
        Ok(Self { handle: handle.ok_or("Vocabulary model unavailable")? })
    }
}

impl Drop for VocabularyModel {
    fn drop(&mut self) {
        // SAFETY: this owns exactly one retain and has not been transferred.
        unsafe { sotto_vocabulary_destroy(self.handle.as_ptr()) };
    }
}

#[derive(Debug)]
pub struct AsrResult {
    pub text: String,
    pub vocabulary_candidates: String,
    pub duration: f64,
    pub processing_time: f64,
    pub rtfx: f32,
}

impl FluidAudio {
    pub fn new() -> Result<Self, String> {
        // SAFETY: returns a new retained handle with independent ownership.
        let handle = NonNull::new(unsafe { sotto_asr_create() })
            .ok_or("Failed to allocate FluidAudio bridge")?;
        Ok(Self {
            handle,
            ready: false,
        })
    }

    /// Loads verified speech model files from `directory`. Never downloads.
    pub fn init_asr(&mut self, directory: &Path) -> Result<(), String> {
        if self.ready {
            return Ok(());
        }
        self.reload_asr(directory)
    }

    /// Loads the files in `directory` again, replacing a loaded model. On
    /// failure the bridge reports not ready until a load succeeds.
    pub fn reload_asr(&mut self, directory: &Path) -> Result<(), String> {
        let directory = c_path(directory)?;
        self.ready = false;
        let mut error = std::ptr::null_mut();
        // SAFETY: valid exclusively owned handle, a path that lives through
        // this synchronous call, and a writable output pointer.
        let status = unsafe { sotto_asr_initialize(self.handle.as_ptr(), directory.as_ptr(), &mut error) };
        check_status(status, error)?;
        self.ready = true;
        Ok(())
    }

    pub fn is_asr_available(&self) -> bool {
        self.ready
    }

    pub fn is_apple_silicon(&self) -> bool {
        cfg!(target_arch = "aarch64")
    }

    pub fn attach_vocabulary(&mut self, model: VocabularyModel) {
        let model = std::mem::ManuallyDrop::new(model);
        // SAFETY: this consumes the model's retain and transfers it to the
        // exclusively borrowed ASR handle. Swift replaces its previous owner.
        unsafe { sotto_asr_attach_vocabulary(self.handle.as_ptr(), model.handle.as_ptr()) };
    }

    pub fn unload_vocabulary(&mut self) {
        // SAFETY: this synchronous call holds exclusive ownership.
        unsafe { sotto_asr_unload_vocabulary(self.handle.as_ptr()) };
    }

    pub fn transcribe_file(&mut self, path: impl AsRef<Path>) -> Result<AsrResult, String> {
        self.transcribe_file_with_vocabulary(path, &[])
    }

    pub fn transcribe_file_with_vocabulary(&mut self, path: impl AsRef<Path>, terms: &[String]) -> Result<AsrResult, String> {
        if !self.ready {
            return Err("FluidAudio is not initialized".into());
        }
        let path = path.as_ref();
        if !path.is_file() {
            return Err(format!("Audio file does not exist: {}", path.display()));
        }
        let path =
            CString::new(path.to_string_lossy().as_bytes()).map_err(|_| "Invalid audio path")?;
        let terms = CString::new(serde_json::to_string(terms).map_err(|error| error.to_string())?)
            .map_err(|_| "Invalid vocabulary")?;
        let mut text = std::ptr::null_mut();
        let mut candidates = std::ptr::null_mut();
        let mut error = std::ptr::null_mut();
        let mut duration = 0.0;
        let mut processing_time = 0.0;
        // SAFETY: handle is exclusively borrowed, the path lives through this
        // synchronous call, and all output pointers refer to writable storage.
        let status = unsafe {
            sotto_asr_transcribe(
                self.handle.as_ptr(),
                path.as_ptr(),
                terms.as_ptr(),
                &mut text,
                &mut candidates,
                &mut duration,
                &mut processing_time,
                &mut error,
            )
        };
        let text = take_string(text);
        let vocabulary_candidates = take_string(candidates);
        check_status(status, error)?;
        Ok(AsrResult {
            text,
            vocabulary_candidates,
            duration,
            processing_time,
            rtfx: if processing_time > 0.0 {
                (duration / processing_time) as f32
            } else {
                0.0
            },
        })
    }
}

fn c_path(path: &Path) -> Result<CString, String> {
    CString::new(path.to_string_lossy().as_bytes())
        .map_err(|_| format!("Invalid path: {}", path.display()))
}

fn check_status(status: i32, error: *mut c_char) -> Result<(), String> {
    let error = take_string(error);
    if status == 0 {
        Ok(())
    } else if error.is_empty() {
        Err("FluidAudio operation failed".into())
    } else {
        Err(error)
    }
}

fn take_string(value: *mut c_char) -> String {
    if value.is_null() {
        return String::new();
    }
    // SAFETY: bridge outputs are NUL-terminated strdup allocations. Copy the
    // entire string before releasing it with the matching Swift/C allocator.
    unsafe {
        let text = CStr::from_ptr(value).to_string_lossy().into_owned();
        sotto_asr_free_string(value);
        text
    }
}

impl Drop for FluidAudio {
    fn drop(&mut self) {
        // SAFETY: this is the sole owner of the retained Swift handle.
        unsafe { sotto_asr_destroy(self.handle.as_ptr()) };
    }
}
