use rubato::{FftFixedIn, Resampler};

/// Resample audio from source_rate to 16000 Hz mono.
/// Currently unused but retained for future use with backends that support raw sample input.
#[allow(dead_code)]
pub fn resample_to_16khz(samples: &[f32], source_rate: u32) -> Result<Vec<f32>, String> {
    if source_rate == 16000 {
        return Ok(samples.to_vec());
    }

    let target_rate = 16000;
    let chunk_size = 1024;

    let mut resampler = FftFixedIn::<f32>::new(
        source_rate as usize,
        target_rate as usize,
        chunk_size,
        1, // sub-chunks
        1, // 1 channel (mono)
    )
    .map_err(|e| format!("Failed to create resampler: {}", e))?;

    let mut output = Vec::new();
    let num_chunks = samples.len() / chunk_size;

    for i in 0..num_chunks {
        let start = i * chunk_size;
        let end = start + chunk_size;
        let chunk = &samples[start..end];

        let result = resampler
            .process(&[chunk], None)
            .map_err(|e| format!("Resampling error: {}", e))?;

        if let Some(channel) = result.first() {
            output.extend_from_slice(channel);
        }
    }

    // Handle remaining samples
    let remaining = samples.len() % chunk_size;
    if remaining > 0 {
        let mut padded = vec![0.0f32; chunk_size];
        let start = num_chunks * chunk_size;
        padded[..remaining].copy_from_slice(&samples[start..]);

        let result = resampler
            .process(&[&padded], None)
            .map_err(|e| format!("Resampling error: {}", e))?;

        if let Some(channel) = result.first() {
            // Only take the proportional amount of output
            let expected = (remaining as f64 * target_rate as f64 / source_rate as f64) as usize;
            let take = expected.min(channel.len());
            output.extend_from_slice(&channel[..take]);
        }
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_at_16khz() {
        let samples: Vec<f32> = (0..4096).map(|i| (i as f32) / 4096.0).collect();
        let result = resample_to_16khz(&samples, 16000).unwrap();
        assert_eq!(result.len(), samples.len());
        assert_eq!(result, samples);
    }

    #[test]
    fn ratio_at_48khz() {
        // 48kHz -> 16kHz is a 3:1 ratio, so output should be ~1/3 of input
        let samples = vec![0.0f32; 48000]; // 1 second at 48kHz
        let result = resample_to_16khz(&samples, 48000).unwrap();
        // Allow tolerance for resampler edge effects (chunk boundaries, padding)
        let expected = 16000;
        let tolerance = 500;
        assert!(
            (result.len() as i64 - expected as i64).unsigned_abs() < tolerance,
            "Expected ~{} samples, got {}",
            expected,
            result.len()
        );
    }

    #[test]
    fn ratio_at_44100() {
        // 44.1kHz -> 16kHz, output should be ~16000/44100 of input
        let samples = vec![0.0f32; 44100]; // 1 second at 44.1kHz
        let result = resample_to_16khz(&samples, 44100).unwrap();
        let expected = 16000;
        let tolerance = 500;
        assert!(
            (result.len() as i64 - expected as i64).unsigned_abs() < tolerance,
            "Expected ~{} samples, got {}",
            expected,
            result.len()
        );
    }

    #[test]
    fn empty_input_returns_empty() {
        let result = resample_to_16khz(&[], 48000).unwrap();
        assert!(result.is_empty());
    }
}
