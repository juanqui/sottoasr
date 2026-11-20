use rubato::{FftFixedIn, Resampler};

pub const TARGET_RATE: u32 = 16_000;
const CHUNK_SIZE: usize = 1024;

/// Resamples mono audio to 16 kHz in pieces, so a long recording is never in
/// memory as a whole. The output starts at the first input sample (the
/// resampler's delay is trimmed) and, after `finish`, covers the whole
/// input, including the tail still inside the filter when the input ends.
pub struct Resampler16k {
    /// `None` at 16 kHz: samples pass through.
    filter: Option<Filter>,
}

struct Filter {
    resampler: FftFixedIn<f32>,
    source_rate: u32,
    buffer: Vec<Vec<f32>>,
    /// Input that does not fill a block yet.
    pending: Vec<f32>,
    /// Output samples of the filter delay that are still to drop.
    skip: usize,
    received: u64,
    produced: u64,
}

impl Resampler16k {
    pub fn new(source_rate: u32) -> Result<Self, String> {
        if source_rate == TARGET_RATE {
            return Ok(Self { filter: None });
        }
        let resampler = FftFixedIn::<f32>::new(source_rate as usize, TARGET_RATE as usize, CHUNK_SIZE, 1, 1)
            .map_err(|e| format!("Failed to create resampler: {}", e))?;
        Ok(Self { filter: Some(Filter {
            skip: resampler.output_delay(),
            buffer: resampler.output_buffer_allocate(true),
            resampler,
            source_rate,
            pending: Vec::with_capacity(CHUNK_SIZE),
            received: 0,
            produced: 0,
        }) })
    }

    /// Appends the 16 kHz samples that `samples` completes to `output`.
    pub fn push(&mut self, samples: &[f32], output: &mut Vec<f32>) -> Result<(), String> {
        let Some(filter) = &mut self.filter else {
            output.extend_from_slice(samples);
            return Ok(());
        };
        filter.received += samples.len() as u64;
        let mut samples = samples;
        // Complete the block left from the previous call first.
        if !filter.pending.is_empty() {
            let take = (CHUNK_SIZE - filter.pending.len()).min(samples.len());
            filter.pending.extend_from_slice(&samples[..take]);
            samples = &samples[take..];
            if filter.pending.len() < CHUNK_SIZE {
                return Ok(());
            }
            let block = std::mem::take(&mut filter.pending);
            filter.process(Some(&block), false, output)?;
            filter.pending = block;
            filter.pending.clear();
        }
        let mut blocks = samples.chunks_exact(CHUNK_SIZE);
        for block in &mut blocks {
            filter.process(Some(block), false, output)?;
        }
        filter.pending.extend_from_slice(blocks.remainder());
        Ok(())
    }

    /// Appends the rest: the last partial block and the tail in the filter.
    pub fn finish(&mut self, output: &mut Vec<f32>) -> Result<(), String> {
        let Some(filter) = &mut self.filter else { return Ok(()) };
        if !filter.pending.is_empty() {
            let block = std::mem::take(&mut filter.pending);
            filter.process(Some(&block), true, output)?;
        }
        // Zero input pushes the delayed tail out of the filter.
        while filter.produced < filter.expected() {
            filter.process(None, true, output)?;
        }
        Ok(())
    }
}

impl Filter {
    /// The output length for the input so far: the rate ratio, rounded up.
    fn expected(&self) -> u64 {
        (self.received * TARGET_RATE as u64).div_ceil(self.source_rate as u64)
    }

    /// Runs one block (`None`: silence that moves the tail out) and appends
    /// its output without the filter delay. `last` also stops at the
    /// expected length.
    fn process(&mut self, block: Option<&[f32]>, last: bool, output: &mut Vec<f32>) -> Result<(), String> {
        let written = match block {
            Some(block) if block.len() == CHUNK_SIZE => {
                self.resampler.process_into_buffer(&[block], self.buffer.as_mut_slice(), None)
            }
            Some(block) => self.resampler.process_partial_into_buffer(Some(&[block]), self.buffer.as_mut_slice(), None),
            None => self.resampler.process_partial_into_buffer(None::<&[&[f32]]>, self.buffer.as_mut_slice(), None),
        }.map_err(|e| format!("Resampling error: {}", e))?.1;
        let skipped = self.skip.min(written);
        self.skip -= skipped;
        let mut ready = &self.buffer[0][skipped..written];
        if last {
            let room = self.expected().saturating_sub(self.produced) as usize;
            ready = &ready[..ready.len().min(room)];
        }
        output.extend_from_slice(ready);
        self.produced += ready.len() as u64;
        Ok(())
    }
}

/// Resamples a whole mono recording to 16 kHz.
#[cfg(any(feature = "asr-parakeet", test))]
pub fn resample_to_16khz(samples: &[f32], source_rate: u32) -> Result<Vec<f32>, String> {
    let mut resampler = Resampler16k::new(source_rate)?;
    let expected = (samples.len() as u64 * TARGET_RATE as u64).div_ceil(source_rate.max(1) as u64) as usize;
    let mut output = Vec::with_capacity(expected);
    resampler.push(samples, &mut output)?;
    resampler.finish(&mut output)?;
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

    #[test]
    fn pieces_of_any_size_give_the_output_of_the_whole_recording() {
        for rate in [44_100_u32, 48_000, 192_000, 8_000] {
            let samples: Vec<f32> = (0..rate as usize + 777).map(|index| tone(rate, index)).collect();
            let whole = resample_to_16khz(&samples, rate).unwrap();
            // A block boundary inside a piece, pieces smaller than a block,
            // and an empty piece.
            for sizes in [[4_800, 4_800, 4_800], [1, 1_023, 1_025], [333, 0, 4_096]] {
                let mut resampler = Resampler16k::new(rate).unwrap();
                let (mut output, mut rest) = (Vec::new(), samples.as_slice());
                for size in sizes.iter().cycle() {
                    if rest.is_empty() { break; }
                    let (piece, after) = rest.split_at((*size).min(rest.len()));
                    resampler.push(piece, &mut output).unwrap();
                    rest = after;
                }
                assert!(output.len() < whole.len(), "the tail waits for the end at {rate} Hz");
                resampler.finish(&mut output).unwrap();
                assert_eq!(output, whole, "{rate} Hz in pieces of {sizes:?}");
            }
        }
    }
}
