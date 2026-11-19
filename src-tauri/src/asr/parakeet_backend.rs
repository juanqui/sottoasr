//! parakeet-rs ASR backend — cross-platform.
//!
//! Uses ONNX Runtime for inference on CPU. Works on macOS, Windows, Linux.
//! ~20-30x real-time factor on Apple Silicon CPU.
//!
//! Model files must be downloaded separately (see model.rs for download logic).
//! Uses Parakeet TDT 0.6B v3 INT8 quantized ONNX model (~639 MB).
//!
//! Reference: https://github.com/altunenes/parakeet-rs

use super::engine::{AsrEngine, AsrResult};
use super::model;
use crate::audio::resample;
use parakeet_rs::Transcriber; // Import the trait for transcribe_samples

/// parakeet-rs ASR engine using ONNX Runtime.
pub struct ParakeetEngine {
    engine: Option<parakeet_rs::ParakeetTDT>,
    ready: bool,
}

impl ParakeetEngine {
    pub fn new() -> Self {
        Self {
            engine: None,
            ready: false,
        }
    }
}

impl AsrEngine for ParakeetEngine {
    fn init(&mut self) -> Result<(), String> {
        if self.ready {
            return Ok(());
        }

        let model_dir = model::get_model_dir()?;

        if !model::is_model_available() {
            return Err("Parakeet ONNX model not downloaded. Run model download first.".into());
        }

        log::info!("Loading Parakeet TDT model from {:?}...", model_dir);
        let start = std::time::Instant::now();

        let engine = parakeet_rs::ParakeetTDT::from_pretrained(
            model_dir.to_str().ok_or("Invalid model path")?,
            None,
        )
        .map_err(|e| format!("Failed to load Parakeet model: {}", e))?;

        log::info!("Parakeet TDT loaded in {:.1}s", start.elapsed().as_secs_f64());

        self.engine = Some(engine);
        self.ready = true;
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.ready
    }

    fn transcribe_file(&mut self, path: &str) -> Result<AsrResult, String> {
        let (samples, sample_rate) = read_mono_wav(path)?;
        self.transcribe_samples(&samples, sample_rate)
    }

    fn transcribe_samples(&mut self, samples: &[f32], sample_rate: u32) -> Result<AsrResult, String> {
        let engine = self.engine.as_mut()
            .ok_or("Parakeet engine not initialized")?;

        if samples.is_empty() {
            return Err("No audio samples provided".into());
        }
        if sample_rate == 0 {
            return Err("The recording has no sample rate".into());
        }

        let duration_secs = samples.len() as f64 / sample_rate as f64;
        log::info!("Transcribing {:.1}s of audio ({} samples)", duration_secs, samples.len());

        let start = std::time::Instant::now();

        // The model accepts only 16 kHz; recordings arrive at the device rate.
        let samples = resample::resample_to_16khz(samples, sample_rate)?;
        let result = engine.transcribe_samples(
            samples,
            resample::TARGET_RATE,
            1,    // mono
            None, // no timestamp mode
        )
        .map_err(|e| format!("Transcription failed: {}", e))?;

        let processing_time = start.elapsed().as_secs_f64();

        Ok(AsrResult { unboosted_text: None,
            text: result.text,
            duration_secs,
            processing_time_secs: processing_time,
            rtfx: if processing_time > 0.0 { (duration_secs / processing_time) as f32 } else { 0.0 },
        })
    }

    fn is_model_available(&self) -> bool {
        model::is_model_available()
    }

    fn backend_name(&self) -> &'static str {
        "parakeet-rs (ONNX/CPU)"
    }
}

/// Decodes a recording to mono f32 samples at its own rate.
fn read_mono_wav(path: &str) -> Result<(Vec<f32>, u32), String> {
    let unreadable = |error: hound::Error| format!("Could not read the recording: {error}");
    let mut reader = hound::WavReader::open(path).map_err(unreadable)?;
    let spec = reader.spec();
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>(),
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample.max(1) - 1)) as f32;
            reader.samples::<i32>().map(|sample| sample.map(|value| value as f32 * scale)).collect::<Result<_, _>>()
        }
    }.map_err(unreadable)?;
    let channels = usize::from(spec.channels.max(1));
    let mono = if channels == 1 {
        interleaved
    } else {
        interleaved.chunks_exact(channels).map(|frame| frame.iter().sum::<f32>() / channels as f32).collect()
    };
    Ok((mono, spec.sample_rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_16_bit_recording_is_one_mono_frame_per_stereo_frame() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("stereo.wav");
        let spec = hound::WavSpec { channels: 2, sample_rate: 48_000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for _ in 0..4_800 {
            writer.write_sample(16_384i16).unwrap();
            writer.write_sample(0i16).unwrap();
        }
        writer.finalize().unwrap();

        let (samples, sample_rate) = read_mono_wav(path.to_str().unwrap()).unwrap();
        assert_eq!(sample_rate, 48_000);
        assert_eq!(samples.len(), 4_800);
        assert!((samples.len() as f64 / sample_rate as f64 - 0.1).abs() < 1e-9, "the duration is 100 ms");
        assert!(samples.iter().all(|sample| (sample - 0.25).abs() < 1e-6), "channels are averaged at full scale");
    }
}
