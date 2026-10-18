/// Zero crossings of the windowed-sinc kernel on each side of its centre.
const ZERO_CROSSINGS: usize = 16;

/// Kernel table entries per zero crossing (linearly interpolated between).
const TABLE_RESOLUTION: usize = 256;

/// Resample audio from one sample rate to another.
///
/// Uses band-limited (windowed-sinc) interpolation. When downsampling, the
/// kernel is a low-pass filter just below the new Nyquist frequency, so
/// content above it (e.g. 8–24 kHz of a 48 kHz microphone) is removed instead
/// of folding back into the speech band as noise.
///
/// If the source and target rates are equal, the samples are returned as-is.
pub fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || samples.is_empty() || from_rate == 0 || to_rate == 0 {
        return samples.to_vec();
    }

    // Input samples per output sample.
    let step = from_rate as f64 / to_rate as f64;
    let output_len = (samples.len() as f64 / step).ceil() as usize;

    // Cutoff at 95% of the lower of the two Nyquist frequencies, expressed as
    // a fraction of the input Nyquist frequency.
    let cutoff = 0.95 / step.max(1.0);
    let half_width = ZERO_CROSSINGS as f64 / cutoff;
    let table = kernel_table();
    let last_input = samples.len() - 1;

    let mut output = Vec::with_capacity(output_len);
    for i in 0..output_len {
        let centre = i as f64 * step;
        let first = (centre - half_width).ceil().max(0.0) as usize;
        let last = ((centre + half_width).floor() as usize).min(last_input);

        let mut acc = 0.0f64;
        let mut weight_sum = 0.0f64;
        for (k, &sample) in samples.iter().enumerate().take(last + 1).skip(first) {
            let pos = (centre - k as f64).abs() * cutoff * TABLE_RESOLUTION as f64;
            let idx = pos as usize;
            if idx + 1 >= table.len() {
                continue;
            }
            let frac = pos - idx as f64;
            let weight = table[idx] + (table[idx + 1] - table[idx]) * frac;
            acc += sample as f64 * weight;
            weight_sum += weight;
        }

        // Normalising by the weights keeps a constant signal constant, also
        // near the edges where the kernel is cut off.
        output.push(if weight_sum.abs() > 1e-9 {
            (acc / weight_sum) as f32
        } else {
            0.0
        });
    }

    output
}

/// One half of a Blackman-windowed sinc, sampled at `TABLE_RESOLUTION` points
/// per zero crossing from the centre out to `ZERO_CROSSINGS`.
fn kernel_table() -> Vec<f64> {
    use std::f64::consts::PI;

    let len = ZERO_CROSSINGS * TABLE_RESOLUTION + 1;
    (0..len)
        .map(|i| {
            let x = i as f64 / TABLE_RESOLUTION as f64;
            let sinc = if i == 0 {
                1.0
            } else {
                (PI * x).sin() / (PI * x)
            };
            let w = x / ZERO_CROSSINGS as f64;
            let window = 0.42 + 0.5 * (PI * w).cos() + 0.08 * (2.0 * PI * w).cos();
            sinc * window
        })
        .collect()
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

    /// Amplitude of a sine at `freq` Hz in `samples` (single-bin DFT).
    fn tone_amplitude(samples: &[f32], rate: u32, freq: f64) -> f64 {
        let (mut re, mut im) = (0.0f64, 0.0f64);
        for (n, &s) in samples.iter().enumerate() {
            let phase = 2.0 * std::f64::consts::PI * freq * n as f64 / rate as f64;
            re += s as f64 * phase.cos();
            im += s as f64 * phase.sin();
        }
        2.0 * (re * re + im * im).sqrt() / samples.len() as f64
    }

    fn sine(freq: f64, rate: u32, secs: f64) -> Vec<f32> {
        let n = (rate as f64 * secs) as usize;
        (0..n)
            .map(|i| (2.0 * std::f64::consts::PI * freq * i as f64 / rate as f64).sin() as f32)
            .collect()
    }

    #[test]
    fn downsample_keeps_speech_band_tone() {
        let out = resample(&sine(1000.0, 48000, 1.0), 48000, 16000);
        let amp = tone_amplitude(&out[2000..14000], 16000, 1000.0);
        assert!((amp - 1.0).abs() < 0.02, "1 kHz amplitude {amp}");
    }

    #[test]
    fn downsample_removes_content_above_new_nyquist() {
        // A 12 kHz tone cannot exist at 16 kHz. Without a low-pass filter it
        // aliases to 4 kHz.
        let out = resample(&sine(12000.0, 48000, 1.0), 48000, 16000);
        let alias = tone_amplitude(&out[2000..14000], 16000, 4000.0);
        assert!(alias < 0.01, "aliased 4 kHz amplitude {alias}");
    }

    #[test]
    fn non_integer_ratio_keeps_tone() {
        let out = resample(&sine(440.0, 44100, 1.0), 44100, 16000);
        let amp = tone_amplitude(&out[2000..14000], 16000, 440.0);
        assert!((amp - 1.0).abs() < 0.02, "440 Hz amplitude {amp}");
    }

    #[test]
    fn constant_signal_stays_constant() {
        let out = resample(&vec![0.5f32; 4410], 44100, 16000);
        assert!(out.iter().all(|&s| (s - 0.5).abs() < 1e-4));
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
