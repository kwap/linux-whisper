use std::path::Path;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use thiserror::Error;
use tracing::{debug, info};

use crate::capture::AudioBuffer;

// ---------------------------------------------------------------------------
// DecodeError
// ---------------------------------------------------------------------------

/// Errors that can occur while decoding an audio file.
#[derive(Debug, Error)]
pub enum DecodeError {
    /// The file format is not supported by the enabled symphonia codecs.
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),

    /// An I/O error occurred while reading the file.
    #[error(transparent)]
    IoError(#[from] std::io::Error),

    /// An error occurred during audio decoding.
    #[error("decode error: {0}")]
    DecodeError(String),
}

// ---------------------------------------------------------------------------
// DecodedAudio
// ---------------------------------------------------------------------------

/// Raw decoded audio data from a file.
#[derive(Debug, Clone)]
pub struct DecodedAudio {
    /// Interleaved f32 samples (multi-channel if `channels > 1`).
    pub samples: Vec<f32>,
    /// The sample rate of the decoded audio.
    pub sample_rate: u32,
    /// Number of audio channels.
    pub channels: u16,
    /// Duration of the audio in seconds.
    pub duration_secs: f64,
}

// ---------------------------------------------------------------------------
// decode_file
// ---------------------------------------------------------------------------

/// Decode an audio file at `path` into raw f32 samples.
///
/// Supports any format/codec enabled in symphonia (mp3, wav, ogg/vorbis, aac/m4a).
pub fn decode_file(path: &Path) -> Result<DecodedAudio, DecodeError> {
    let file = std::fs::File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    // Give symphonia a hint about the file extension.
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let format_opts = FormatOptions::default();
    let metadata_opts = MetadataOptions::default();

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &format_opts, &metadata_opts)
        .map_err(|e| DecodeError::UnsupportedFormat(e.to_string()))?;

    let mut format_reader = probed.format;

    // Select the first audio track.
    let track = format_reader
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .ok_or_else(|| DecodeError::UnsupportedFormat("no audio track found".into()))?;

    let codec_params = track.codec_params.clone();
    let track_id = track.id;

    let sample_rate = codec_params
        .sample_rate
        .ok_or_else(|| DecodeError::DecodeError("unknown sample rate".into()))?;

    let channels = codec_params
        .channels
        .map(|ch| ch.count() as u16)
        .unwrap_or(1);

    debug!(
        "Decoding {:?}: {} Hz, {} ch",
        path.file_name().unwrap_or_default(),
        sample_rate,
        channels,
    );

    let decoder_opts = DecoderOptions::default();
    let mut decoder = symphonia::default::get_codecs()
        .make(&codec_params, &decoder_opts)
        .map_err(|e| DecodeError::UnsupportedFormat(e.to_string()))?;

    let mut all_samples: Vec<f32> = Vec::new();

    loop {
        let packet = match format_reader.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                // End of stream.
                break;
            }
            Err(e) => return Err(DecodeError::DecodeError(e.to_string())),
        };

        // Skip packets that don't belong to our track.
        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(symphonia::core::errors::Error::DecodeError(_)) => {
                // Skip corrupt packets.
                continue;
            }
            Err(e) => return Err(DecodeError::DecodeError(e.to_string())),
        };

        let spec = *decoded.spec();
        let num_frames = decoded.capacity();

        let mut sample_buf = SampleBuffer::<f32>::new(num_frames as u64, spec);
        sample_buf.copy_interleaved_ref(decoded);
        all_samples.extend_from_slice(sample_buf.samples());
    }

    let total_frames = if channels > 0 {
        all_samples.len() / channels as usize
    } else {
        all_samples.len()
    };
    let duration_secs = total_frames as f64 / sample_rate as f64;

    info!(
        "Decoded {} samples ({:.2}s) from {:?}",
        all_samples.len(),
        duration_secs,
        path.file_name().unwrap_or_default(),
    );

    Ok(DecodedAudio {
        samples: all_samples,
        sample_rate,
        channels,
        duration_secs,
    })
}

// ---------------------------------------------------------------------------
// decode_to_mono_16khz
// ---------------------------------------------------------------------------

/// Convenience function: decode a file and convert to mono 16 kHz [`AudioBuffer`].
pub fn decode_to_mono_16khz(path: &Path) -> Result<AudioBuffer, DecodeError> {
    let decoded = decode_file(path)?;

    let mono = crate::resample::to_mono(&decoded.samples, decoded.channels);
    let resampled = crate::resample::resample(&mono, decoded.sample_rate, AudioBuffer::TARGET_SAMPLE_RATE);

    info!(
        "Converted to mono 16 kHz: {} samples",
        resampled.len()
    );

    Ok(AudioBuffer {
        samples: resampled,
        sample_rate: AudioBuffer::TARGET_SAMPLE_RATE,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn decode_nonexistent_file_returns_io_error() {
        let path = PathBuf::from("/tmp/does_not_exist_linux_whisper_test.wav");
        let result = decode_file(&path);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DecodeError::IoError(_)));
    }

    #[test]
    fn decode_non_audio_file_returns_unsupported() {
        // Create a temporary text file and try to decode it.
        let path = PathBuf::from("/tmp/linux_whisper_test_not_audio.txt");
        std::fs::write(&path, "this is not audio data").unwrap();

        let result = decode_file(&path);
        assert!(result.is_err());
        match result.unwrap_err() {
            DecodeError::UnsupportedFormat(_) => {} // expected
            other => panic!("expected UnsupportedFormat, got: {other:?}"),
        }

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn decoded_audio_struct_creation() {
        let audio = DecodedAudio {
            samples: vec![0.0; 16000],
            sample_rate: 16000,
            channels: 1,
            duration_secs: 1.0,
        };
        assert_eq!(audio.sample_rate, 16000);
        assert_eq!(audio.channels, 1);
        assert!((audio.duration_secs - 1.0).abs() < f64::EPSILON);
        assert_eq!(audio.samples.len(), 16000);
    }

    /// Returns the path to the test WAV fixture (2-second 440 Hz sine wave).
    fn test_wav_path() -> PathBuf {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        manifest_dir.join("test_fixtures").join("test.wav")
    }

    #[test]
    fn decode_real_wav_file() {
        let path = test_wav_path();
        let result = decode_file(&path);
        assert!(result.is_ok(), "failed to decode: {:?}", result.err());
        let decoded = result.unwrap();
        assert_eq!(decoded.sample_rate, 44100);
        assert_eq!(decoded.channels, 1);
        assert!(!decoded.samples.is_empty());
        // 2 seconds at 44100 Hz mono = ~88200 samples
        assert!(decoded.samples.len() > 80_000);
        assert!((decoded.duration_secs - 2.0).abs() < 0.1);
    }

    #[test]
    fn decode_to_mono_16khz_real_file() {
        let path = test_wav_path();
        let result = decode_to_mono_16khz(&path);
        assert!(result.is_ok(), "failed to decode: {:?}", result.err());
        let buf = result.unwrap();
        assert_eq!(buf.sample_rate, 16000);
        assert!(!buf.samples.is_empty());
        // 2 seconds at 16000 Hz = ~32000 samples
        assert!(buf.samples.len() > 30_000);
        assert!(buf.samples.len() < 34_000);
    }
}
