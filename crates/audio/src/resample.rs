/// Resample audio from one sample rate to another using linear interpolation.
///
/// If the source and target rates are equal, the samples are returned as-is.
pub fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || samples.is_empty() {
        return samples.to_vec();
    }

    let ratio = from_rate as f64 / to_rate as f64;
    let output_len = ((samples.len() as f64) / ratio).ceil() as usize;
    let mut output = Vec::with_capacity(output_len);

    for i in 0..output_len {
        let src_idx = i as f64 * ratio;
        let idx_floor = src_idx.floor() as usize;
        let frac = (src_idx - idx_floor as f64) as f32;

        let sample = if idx_floor + 1 < samples.len() {
            samples[idx_floor] * (1.0 - frac) + samples[idx_floor + 1] * frac
        } else if idx_floor < samples.len() {
            samples[idx_floor]
        } else {
            0.0
        };

        output.push(sample);
    }

    output
}

/// Convert multi-channel interleaved audio to mono by averaging all channels.
///
/// If the audio is already mono (`channels == 1`), the samples are returned as-is.
pub fn to_mono(samples: &[f32], channels: u16) -> Vec<f32> {
    if channels <= 1 || samples.is_empty() {
        return samples.to_vec();
    }

    let ch = channels as usize;
    let frame_count = samples.len() / ch;
    let mut mono = Vec::with_capacity(frame_count);

    for frame in 0..frame_count {
        let offset = frame * ch;
        let mut sum = 0.0f32;
        for c in 0..ch {
            sum += samples[offset + c];
        }
        mono.push(sum / channels as f32);
    }

    mono
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_rate_returns_same_samples() {
        let samples = vec![0.1, 0.2, 0.3, 0.4, 0.5];
        let result = resample(&samples, 16000, 16000);
        assert_eq!(result, samples);
    }

    #[test]
    fn downsample_produces_correct_length() {
        // 1 second of audio at 44100 Hz
        let samples: Vec<f32> = (0..44100).map(|i| (i as f32 / 44100.0).sin()).collect();
        let result = resample(&samples, 44100, 16000);

        // Should produce approximately 16000 samples for 1 second of audio
        let expected_len = ((44100_f64 / (44100_f64 / 16000_f64)).ceil()) as usize;
        assert_eq!(result.len(), expected_len);

        // Verify approximately 16000 samples (within a small margin)
        assert!(
            (result.len() as i64 - 16000).unsigned_abs() <= 1,
            "Expected ~16000 samples, got {}",
            result.len()
        );
    }

    #[test]
    fn upsample_produces_correct_length() {
        // 1 second of audio at 8000 Hz
        let samples: Vec<f32> = (0..8000).map(|i| (i as f32 / 8000.0).sin()).collect();
        let result = resample(&samples, 8000, 16000);

        // Should produce approximately 16000 samples
        assert!(
            (result.len() as i64 - 16000).unsigned_abs() <= 1,
            "Expected ~16000 samples, got {}",
            result.len()
        );
    }

    #[test]
    fn resample_empty_input() {
        let result = resample(&[], 44100, 16000);
        assert!(result.is_empty());
    }

    #[test]
    fn to_mono_single_channel_returns_same() {
        let samples = vec![0.1, 0.2, 0.3, 0.4];
        let result = to_mono(&samples, 1);
        assert_eq!(result, samples);
    }

    #[test]
    fn to_mono_stereo_averages_pairs() {
        // Stereo: [L0, R0, L1, R1]
        let samples = vec![0.2, 0.4, 0.6, 0.8];
        let result = to_mono(&samples, 2);
        assert_eq!(result.len(), 2);
        assert!((result[0] - 0.3).abs() < f32::EPSILON);
        assert!((result[1] - 0.7).abs() < f32::EPSILON);
    }

    #[test]
    fn to_mono_empty_input() {
        let result = to_mono(&[], 2);
        assert!(result.is_empty());
    }
}
