//! Checked serialization shared by recording, cancellation, and raw-sample ASR.

use std::io::{Seek, Write};
use std::path::Path;

pub const TRAILING_SILENCE_MS: u32 = 750;

pub fn captured_duration_ms(sample_count: usize, sample_rate: u32) -> u64 {
    if sample_rate == 0 {
        return 0;
    }
    ((sample_count as u128 * 1000) / sample_rate as u128) as u64
}

pub fn write_recording_wav(path: &Path, samples: &[f32], sample_rate: u32) -> Result<(), String> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path).map_err(|e| format!("WAV create failed: {e}"))?;
    let result = write_wav(std::io::BufWriter::new(file), samples, sample_rate)
        .map_err(|e| format!("WAV write failed: {e}"));
    if result.is_err() { let _ = std::fs::remove_file(path); }
    result
}

/// Successful inference can discard its temporary source. Failed inference
/// keeps the private WAV so a transient model error does not erase the speech.
#[cfg(any(feature = "asr-fluidaudio", test))]
pub fn finish_recording_wav<T>(path: &Path, result: Result<T, String>) -> Result<T, String> {
    match result {
        Ok(value) => {
            let _ = std::fs::remove_file(path);
            Ok(value)
        }
        Err(error) => Err(format!("{error}. Audio retained at {}", path.display())),
    }
}

/// The audio is the recovery copy until history acknowledges durable storage.
/// Interrupted recordings remain available even when their partial text saved.
pub fn finish_after_history(path: &Path, interrupted: bool, saved: bool) {
    if !interrupted && saved {
        let _ = std::fs::remove_file(path);
    }
}

fn write_wav<W: Write + Seek>(
    writer: W,
    samples: &[f32],
    sample_rate: u32,
) -> Result<(), hound::Error> {
    if sample_rate == 0 {
        return Err(hound::Error::FormatError("sample rate must be positive"));
    }
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::new(writer, spec)?;
    for &sample in samples {
        writer.write_sample(sample)?;
    }
    // Retain the existing final-speech boundary while the fixed runtime handles
    // partial windows. This silence is never counted as captured duration.
    let padding = sample_rate as u64 * TRAILING_SILENCE_MS as u64 / 1000;
    for _ in 0..padding {
        writer.write_sample(0.0_f32)?;
    }
    writer.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Cursor, SeekFrom};

    #[test]
    fn successful_asr_keeps_recovery_audio_until_history_is_durable() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("captured.wav");
        write_recording_wav(&path, &[0.1, 0.2, 0.3], 16_000).unwrap();
        let captured = std::fs::read(&path).unwrap();
        finish_after_history(&path, false, false);
        assert_eq!(std::fs::read(&path).unwrap(), captured);
        finish_after_history(&path, true, true);
        assert_eq!(std::fs::read(&path).unwrap(), captured);
        finish_after_history(&path, false, true);
        assert!(!path.exists());
    }

    #[test]
    fn failed_inference_retains_private_audio_and_never_overwrites_existing_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("recording.wav");
        write_recording_wav(&path, &[0.25; 32], 16_000).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let original = std::fs::read(&path).unwrap();
        assert!(write_recording_wav(&path, &[0.75; 32], 16_000).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let error = finish_recording_wav::<()>(&path, Err("ASR unavailable".into())).unwrap_err();
        assert!(error.contains("Audio retained at"));
        assert_eq!(std::fs::read(&path).unwrap(), original);
        finish_recording_wav(&path, Ok(())).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn preserves_all_audio_including_tail_at_native_rates() {
        for sample_rate in [16_000, 44_100, 48_000] {
            let mut samples = vec![0.25_f32; sample_rate as usize * 60 + 13];
            let end = samples.len();
            samples[end - 4..].copy_from_slice(&[0.71, -0.62, 0.53, -0.44]);
            let mut bytes = Cursor::new(Vec::new());
            write_wav(&mut bytes, &samples, sample_rate).unwrap();
            bytes.set_position(0);
            let mut reader = hound::WavReader::new(bytes).unwrap();
            assert_eq!(reader.spec().sample_rate, sample_rate);
            let actual: Vec<f32> = reader.samples().collect::<Result<_, _>>().unwrap();
            assert_eq!(&actual[..samples.len()], samples);
            assert_eq!(
                actual.len(),
                samples.len() + sample_rate as usize * 750 / 1000
            );
            assert!(actual[samples.len()..].iter().all(|sample| *sample == 0.0));
            assert_eq!(
                captured_duration_ms(samples.len(), sample_rate),
                (samples.len() as u64 * 1000) / sample_rate as u64
            );
        }
    }

    struct FailingWriter {
        bytes: Cursor<Vec<u8>>,
        fail_after: usize,
        fail_finalize: bool,
    }
    impl Write for FailingWriter {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            if self.bytes.position() as usize + data.len() > self.fail_after {
                return Err(io::Error::other("simulated full disk"));
            }
            self.bytes.write(data)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl Seek for FailingWriter {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            if self.fail_finalize {
                return Err(io::Error::other("simulated header failure"));
            }
            self.bytes.seek(position)
        }
    }

    #[test]
    fn sample_write_failure_is_returned() {
        let writer = FailingWriter {
            bytes: Cursor::new(Vec::new()),
            fail_after: 100,
            fail_finalize: false,
        };
        assert!(write_wav(writer, &[0.25; 100], 16_000)
            .unwrap_err()
            .to_string()
            .contains("full disk"));
    }

    #[test]
    fn silence_write_failure_is_returned() {
        let writer = FailingWriter {
            bytes: Cursor::new(Vec::new()),
            fail_after: 200,
            fail_finalize: false,
        };
        assert!(write_wav(writer, &[0.25; 4], 16_000)
            .unwrap_err()
            .to_string()
            .contains("full disk"));
    }

    #[test]
    fn finalize_failure_is_returned() {
        let writer = FailingWriter {
            bytes: Cursor::new(Vec::new()),
            fail_after: usize::MAX,
            fail_finalize: true,
        };
        assert!(write_wav(writer, &[0.25; 4], 16_000)
            .unwrap_err()
            .to_string()
            .contains("header failure"));
    }
}
