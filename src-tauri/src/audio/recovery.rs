//! Private, disk-backed capture. Audio callbacks enqueue; only the writer touches disk.
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::time::{Duration, Instant};

pub fn recordings_dir() -> Result<PathBuf, String> {
    #[cfg(test)]
    let root = {
        static ROOT: std::sync::LazyLock<tempfile::TempDir> =
            std::sync::LazyLock::new(|| tempfile::tempdir().unwrap());
        ROOT.path().to_path_buf()
    };
    #[cfg(not(test))]
    let root = dirs::data_dir()
        .ok_or("Could not determine data directory")?
        .join("com.sottoasr.app");
    let directory = root.join("recordings");
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    if std::fs::symlink_metadata(&directory)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_symlink()
    {
        return Err("The recording directory must not be a symbolic link".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    Ok(directory)
}

pub fn recording_id(path: &Path) -> Result<String, String> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("Invalid recording filename")?;
    let raw = name
        .strip_prefix("sotto_")
        .and_then(|name| name.strip_suffix(".wav"))
        .ok_or("Invalid recording filename")?;
    let id = uuid::Uuid::parse_str(raw)
        .map_err(|_| "Invalid recording filename")?
        .to_string();
    if id != raw {
        return Err("Invalid recording filename".into());
    }
    Ok(id)
}

pub(crate) struct RecordingWriter {
    pub path: PathBuf,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<Result<usize, String>>>,
}

pub(crate) type WriterStart = (
    RecordingWriter,
    mpsc::SyncSender<Vec<f32>>,
    mpsc::Sender<u32>,
);

impl RecordingWriter {
    pub fn start(
        path: PathBuf,
        on_error: Arc<dyn Fn(String) + Send + Sync>,
    ) -> Result<WriterStart, String> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options
            .open(&path)
            .map_err(|e| format!("Could not create recording at {}: {e}", path.display()))?;
        let sync_file = file.try_clone().map_err(|e| e.to_string())?;
        let (sender, receiver) = mpsc::sync_channel::<Vec<f32>>(128);
        let (rate_sender, rate_receiver) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let error_path = path.clone();
        let worker = std::thread::Builder::new()
            .name("recording-writer".into())
            .spawn(move || {
                let result = (|| -> Result<usize, String> {
                    let rate = rate_receiver
                        .recv()
                        .map_err(|_| "Microphone did not start")?;
                    if rate == 0 {
                        return Err("Microphone sample rate is zero".into());
                    }
                    let spec = hound::WavSpec {
                        channels: 1,
                        sample_rate: rate,
                        bits_per_sample: 32,
                        sample_format: hound::SampleFormat::Float,
                    };
                    let mut writer = hound::WavWriter::new(std::io::BufWriter::new(file), spec)
                        .map_err(|e| e.to_string())?;
                    writer.flush().map_err(|e| e.to_string())?;
                    sync_file.sync_all().map_err(|e| e.to_string())?;
                    if let Some(parent) = error_path.parent() {
                        std::fs::File::open(parent)
                            .and_then(|file| file.sync_all())
                            .map_err(|e| e.to_string())?;
                    }
                    let mut count = 0usize;
                    let mut checkpoint = Instant::now();
                    loop {
                        let chunk = if stopping.load(Ordering::Acquire) {
                            match receiver.try_recv() {
                                Ok(chunk) => Some(chunk),
                                Err(_) => break,
                            }
                        } else {
                            match receiver.recv_timeout(Duration::from_millis(100)) {
                                Ok(chunk) => Some(chunk),
                                Err(mpsc::RecvTimeoutError::Timeout) => None,
                                Err(mpsc::RecvTimeoutError::Disconnected) => {
                                    // Backends may release their sender before Stop.
                                    // Stop still owns completion and final padding.
                                    std::thread::sleep(Duration::from_millis(100));
                                    None
                                }
                            }
                        };
                        if let Some(chunk) = chunk {
                            for sample in chunk {
                                writer.write_sample(sample).map_err(|e| e.to_string())?;
                                count += 1;
                            }
                        }
                        if checkpoint.elapsed() >= Duration::from_secs(1) {
                            writer.flush().map_err(|e| e.to_string())?;
                            sync_file.sync_all().map_err(|e| e.to_string())?;
                            checkpoint = Instant::now();
                        }
                    }
                    // Padding belongs to successful Stop only, never to periodic checkpoints.
                    if stopping.load(Ordering::Acquire) {
                        for _ in 0..(rate as u64 * super::wav::TRAILING_SILENCE_MS as u64 / 1000) {
                            writer.write_sample(0.0_f32).map_err(|e| e.to_string())?;
                        }
                    }
                    writer.finalize().map_err(|e| e.to_string())?;
                    sync_file.sync_all().map_err(|e| e.to_string())?;
                    Ok(count)
                })();
                result.map_err(|error| {
                    let error = format!(
                        "Recording storage failed: {error}. Audio retained at {}",
                        error_path.display()
                    );
                    on_error(error.clone());
                    error
                })
            })
            .map_err(|e| e.to_string())?;
        Ok((
            Self {
                path,
                stop,
                worker: Some(worker),
            },
            sender,
            rate_sender,
        ))
    }

    pub fn finish(mut self) -> Result<usize, String> {
        self.stop.store(true, Ordering::Release);
        self.worker.take().unwrap().join().map_err(|_| {
            format!(
                "Recording worker failed. Audio retained at {}",
                self.path.display()
            )
        })?
    }

    /// The rate channel must be closed first. Startup never initialized WAV
    /// serialization, so only an empty reservation can be removed.
    pub fn abandon_start(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() { let _ = worker.join(); }
        if std::fs::metadata(&self.path).is_ok_and(|metadata| metadata.len() == 0) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

impl Drop for RecordingWriter {
    fn drop(&mut self) {
        // Never block managed-state destruction. A disconnected stream drains
        // its remaining queue; explicit Stop owns the join and final padding.
        self.stop.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_survives_abrupt_process_exit() {
        const ENV: &str = "SOTTO_CHECKPOINT_CHILD";
        if let Ok(path) = std::env::var(ENV) {
            let path = PathBuf::from(path);
            let (_writer, sender, rate) =
                RecordingWriter::start(path.clone(), Arc::new(|error| panic!("{error}"))).unwrap();
            rate.send(16000).unwrap();
            sender.send(vec![0.25; 32000]).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if hound::WavReader::open(&path).is_ok_and(|reader| reader.duration() == 32000) {
                    break;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(20));
            }
            std::process::exit(73); // Bypass writer Drop/finalize and every destructor.
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("checkpoint.wav");
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "audio::recovery::tests::checkpoint_survives_abrupt_process_exit",
                "--nocapture",
            ])
            .env(ENV, &path)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(73));
        let mut reader = hound::WavReader::open(path).unwrap();
        assert_eq!(reader.duration(), 32000);
        assert!(reader
            .samples::<f32>()
            .all(|sample| sample.unwrap() == 0.25));
    }

    #[test]
    fn finalization_drains_samples_without_overwriting_existing_audio() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("recording.wav");
        let (writer, sender, rate) =
            RecordingWriter::start(path.clone(), Arc::new(|error| panic!("{error}"))).unwrap();
        rate.send(16000).unwrap();
        sender.send(vec![0.25; 16000]).unwrap();
        sender.send(vec![0.75, -0.5]).unwrap();
        assert_eq!(writer.finish().unwrap(), 16002);
        let bytes = std::fs::read(&path).unwrap();
        assert!(RecordingWriter::start(path.clone(), Arc::new(|_| {})).is_err());
        assert_eq!(bytes, std::fs::read(&path).unwrap());
        let mut reader = hound::WavReader::open(path).unwrap();
        let samples: Vec<f32> = reader.samples().map(Result::unwrap).collect();
        assert_eq!(&samples[16000..16002], &[0.75, -0.5]);
        assert_eq!(samples.len(), 28002);
    }
}
