//! Minimal, exclusively owned bridge to FluidAudio's batch Parakeet API.
//! See ../README.md for the pinned SDK and upstream provenance.

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::Path;
use std::ptr::NonNull;

extern "C" {
    fn sotto_asr_create() -> *mut c_void;
    fn sotto_asr_destroy(handle: *mut c_void);
    fn sotto_asr_initialize(handle: *mut c_void, error: *mut *mut c_char) -> i32;
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
    fn sotto_vocabulary_prepare(allow_download: bool, error: *mut *mut c_char) -> *mut c_void;
    fn sotto_vocabulary_destroy(handle: *mut c_void);
    fn sotto_asr_attach_vocabulary(handle: *mut c_void, vocabulary: *mut c_void);
    fn sotto_asr_unload_vocabulary(handle: *mut c_void);
    fn sotto_asr_free_string(value: *mut c_char);
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
    pub fn load(allow_download: bool) -> Result<Self, String> {
        let mut error = std::ptr::null_mut();
        // SAFETY: a new independent retained handle; output storage is valid.
        let handle = NonNull::new(unsafe { sotto_vocabulary_prepare(allow_download, &mut error) });
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

    pub fn init_asr(&mut self) -> Result<(), String> {
        if self.ready {
            return Ok(());
        }
        let mut error = std::ptr::null_mut();
        // SAFETY: valid exclusively owned handle and writable output pointer.
        let status = unsafe { sotto_asr_initialize(self.handle.as_ptr(), &mut error) };
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
