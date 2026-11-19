use rubato::{FftFixedIn, Resampler};

pub const TARGET_RATE: u32 = 16_000;
const CHUNK_SIZE: usize = 1024;

/// Resamples mono audio to 16 kHz. The output starts at the first input
/// sample (the resampler's delay is trimmed) and covers the whole input,
/// including the tail still inside the filter when the input ends.
pub fn resample_to_16khz(samples: &[f32], source_rate: u32) -> Result<Vec<f32>, String> {
    if source_rate == TARGET_RATE {
        return Ok(samples.to_vec());
    }
    if samples.is_empty() {
        return Ok(Vec::new());
    }

    let mut resampler = FftFixedIn::<f32>::new(source_rate as usize, TARGET_RATE as usize, CHUNK_SIZE, 1, 1)
        .map_err(|e| format!("Failed to create resampler: {}", e))?;
    let delay = resampler.output_delay();
    let expected = (samples.len() as u64 * TARGET_RATE as u64).div_ceil(source_rate as u64) as usize;
    let mut buffer = resampler.output_buffer_allocate(true);
    let mut output = Vec::with_capacity(delay + expected + buffer[0].len());
    let error = |e: rubato::ResampleError| format!("Resampling error: {}", e);

    let mut chunks = samples.chunks_exact(CHUNK_SIZE);
    for chunk in &mut chunks {
        let input: [&[f32]; 1] = [chunk];
        let (_, written) = resampler.process_into_buffer(&input[..], buffer.as_mut_slice(), None).map_err(error)?;
        output.extend_from_slice(&buffer[0][..written]);
    }
    let remainder: [&[f32]; 1] = [chunks.remainder()];
    if !remainder[0].is_empty() {
        let (_, written) = resampler.process_partial_into_buffer(Some(&remainder[..]), buffer.as_mut_slice(), None)
            .map_err(error)?;
        output.extend_from_slice(&buffer[0][..written]);
    }
    // Zero input pushes the delayed tail out of the filter.
    while output.len() < delay + expected {
        let (_, written) = resampler.process_partial_into_buffer(None::<&[&[f32]]>, buffer.as_mut_slice(), None)
            .map_err(error)?;
        output.extend_from_slice(&buffer[0][..written]);
    }
    output.truncate(delay + expected);
    output.copy_within(delay.., 0);
    output.truncate(expected);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 100 Hz completes whole periods in a whole second, so both ends of the
    /// signal sit at zero crossings and the filter sees no step.
    fn tone(rate: u32, index: usize) -> f32 {
        0.5 * (2.0 * std::f32::consts::PI * 100.0 * index as f32 / rate as f32).sin()
    }

    #[test]
    fn passthrough_at_16khz() {
        let samples: Vec<f32> = (0..4096).map(|i| (i as f32) / 4096.0).collect();
        let result = resample_to_16khz(&samples, 16000).unwrap();
        assert_eq!(result, samples);
    }

    #[test]
    fn output_length_alignment_and_tail_at_48khz() {
        let samples: Vec<f32> = (0..48_000).map(|index| tone(48_000, index)).collect();
        let result = resample_to_16khz(&samples, 48_000).unwrap();
        assert_eq!(result.len(), 16_000);
        // The delay is trimmed exactly: one sample of shift would already be
        // off by 0.02 near a zero crossing. This covers the last 10 ms too,
        // which only the flush moves out of the filter.
        for (index, value) in result.iter().enumerate() {
            let expected = tone(16_000, index);
            assert!((value - expected).abs() < 0.005, "sample {index}: {value} vs {expected}");
        }
        let tail_energy: f32 = result[15_840..].iter().map(|value| value * value).sum();
        assert!(tail_energy > 15.0, "tail was lost: {tail_energy}");
    }

    fn assert_aligned(result: &[f32], upto: usize, tolerance: f32) {
        for (index, value) in result[..upto].iter().enumerate() {
            let expected = tone(16_000, index);
            assert!((value - expected).abs() < tolerance, "sample {index}: {value} vs {expected}");
        }
    }

    #[test]
    fn delay_is_trimmed_when_upsampling_from_8khz() {
        let samples: Vec<f32> = (0..8_000).map(|index| tone(8_000, index)).collect();
        let result = resample_to_16khz(&samples, 8_000).unwrap();
        assert_eq!(result.len(), 16_000);
        assert_aligned(&result, result.len(), 0.01);
    }

    #[test]
    fn delay_is_trimmed_at_odd_44100_lengths() {
        for length in [12_345, 88_517] {
            let samples: Vec<f32> = (0..length).map(|index| tone(44_100, index)).collect();
            let result = resample_to_16khz(&samples, 44_100).unwrap();
            assert_eq!(result.len(), (length * 16_000).div_ceil(44_100));
            // An odd cut ends mid-wave; the filter rings over the last 20 ms.
            assert_aligned(&result, result.len() - 320, 0.01);
        }
    }

    #[test]
    fn output_length_is_the_rate_ratio_rounded_up() {
        let samples = vec![0.25f32; 44_100];
        for (rate, length) in [(44_100, 44_100), (44_100, 1), (44_100, 2), (48_000, 1_000), (48_000, 3), (8_000, 7), (8_000, 8_001)] {
            let expected = (length as u64 * 16_000).div_ceil(rate as u64) as usize;
            assert_eq!(resample_to_16khz(&samples[..length], rate).unwrap().len(), expected, "{length} samples at {rate} Hz");
        }
    }

    #[test]
    fn empty_input_returns_empty() {
        assert!(resample_to_16khz(&[], 48000).unwrap().is_empty());
    }
}
