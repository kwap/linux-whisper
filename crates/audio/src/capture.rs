use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use thiserror::Error;
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// AudioBuffer
// ---------------------------------------------------------------------------

/// A buffer of mono audio samples at 16 kHz.
#[derive(Debug, Clone)]
pub struct AudioBuffer {
    /// Mono f32 samples at 16 kHz.
    pub samples: Vec<f32>,
    /// Sample rate (always 16 000 after processing).
    pub sample_rate: u32,
}

impl AudioBuffer {
    /// The standard output sample rate used throughout the application.
    pub const TARGET_SAMPLE_RATE: u32 = 16_000;
}

// ---------------------------------------------------------------------------
// CaptureError
// ---------------------------------------------------------------------------

/// Errors that can occur during audio capture.
#[derive(Debug, Error)]
pub enum CaptureError {
    /// No default input device was found on the system.
    #[error("no default input device found")]
    NoInputDevice,

    /// An error originating from the audio device / host.
    #[error("device error: {0}")]
    DeviceError(String),

    /// An error that occurred while recording audio.
    #[error("stream error: {0}")]
    StreamError(String),
}

// ---------------------------------------------------------------------------
// AudioCapture trait
// ---------------------------------------------------------------------------

/// Trait abstracting audio capture so that implementations can be swapped
/// (e.g. for testing with [`MockAudioCapture`]).
#[cfg_attr(test, mockall::automock)]
pub trait AudioCapture {
    /// List the names of all available input devices.
    fn list_devices(&self) -> Result<Vec<String>, CaptureError>;

    /// Begin recording from the default input device.
    fn start_recording(&mut self) -> Result<(), CaptureError>;

    /// Stop recording and return the captured audio resampled to 16 kHz mono.
    fn stop_recording(&mut self) -> Result<AudioBuffer, CaptureError>;

    /// Whether the capture is currently recording.
    fn is_recording(&self) -> bool;
}

// ---------------------------------------------------------------------------
// CpalCapture
// ---------------------------------------------------------------------------

/// Real audio capture implementation backed by CPAL.
pub struct CpalCapture {
    host: cpal::Host,
    recording: bool,
    /// Shared buffer that the input stream callback writes into.
    buffer: Arc<Mutex<Vec<f32>>>,
    /// The active input stream (present only while recording).
    stream: Option<cpal::Stream>,
    /// Sample rate reported by the input device config.
    device_sample_rate: u32,
    /// Number of channels on the input device.
    device_channels: u16,
}

impl CpalCapture {
    /// Create a new `CpalCapture` using the default CPAL host.
    pub fn new() -> Result<Self, CaptureError> {
        let host = cpal::default_host();
        info!("CPAL host: {}", host.id().name());
        Ok(Self {
            host,
            recording: false,
            buffer: Arc::new(Mutex::new(Vec::new())),
            stream: None,
            device_sample_rate: 0,
            device_channels: 0,
        })
    }
}

impl AudioCapture for CpalCapture {
    fn list_devices(&self) -> Result<Vec<String>, CaptureError> {
        let devices = self
            .host
            .input_devices()
            .map_err(|e| CaptureError::DeviceError(e.to_string()))?;

        let names: Vec<String> = devices
            .map(|d| d.name().unwrap_or_else(|_| "<unknown>".to_string()))
            .collect();

        debug!("Found {} input device(s)", names.len());
        Ok(names)
    }

    fn start_recording(&mut self) -> Result<(), CaptureError> {
        if self.recording {
            warn!("start_recording called while already recording");
            return Ok(());
        }

        let device = self
            .host
            .default_input_device()
            .ok_or(CaptureError::NoInputDevice)?;

        let config = device
            .default_input_config()
            .map_err(|e| CaptureError::DeviceError(e.to_string()))?;

        self.device_sample_rate = config.sample_rate().0;
        self.device_channels = config.channels();

        info!(
            "Recording from \"{}\" at {} Hz, {} ch",
            device.name().unwrap_or_else(|_| "<unknown>".to_string()),
            self.device_sample_rate,
            self.device_channels,
        );

        // Clear any previous samples.
        {
            let mut buf = self.buffer.lock().expect("buffer lock poisoned");
            buf.clear();
        }

        let buffer = Arc::clone(&self.buffer);
        let channels = self.device_channels;

        let err_fn = |err: cpal::StreamError| {
            warn!("CPAL stream error: {err}");
        };

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &config.into(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let mono = crate::resample::to_mono(data, channels);
                    let mut buf = buffer.lock().expect("buffer lock poisoned");
                    buf.extend_from_slice(&mono);
                },
                err_fn,
                None,
            ),
            cpal::SampleFormat::I16 => {
                let buffer = Arc::clone(&self.buffer);
                let channels = self.device_channels;
                device.build_input_stream(
                    &config.into(),
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        let float_data: Vec<f32> = data
                            .iter()
                            .map(|&s| s as f32 / i16::MAX as f32)
                            .collect();
                        let mono = crate::resample::to_mono(&float_data, channels);
                        let mut buf = buffer.lock().expect("buffer lock poisoned");
                        buf.extend_from_slice(&mono);
                    },
                    err_fn,
                    None,
                )
            }
            cpal::SampleFormat::U16 => {
                let buffer = Arc::clone(&self.buffer);
                let channels = self.device_channels;
                device.build_input_stream(
                    &config.into(),
                    move |data: &[u16], _: &cpal::InputCallbackInfo| {
                        let float_data: Vec<f32> = data
                            .iter()
                            .map(|&s| (s as f32 / u16::MAX as f32) * 2.0 - 1.0)
                            .collect();
                        let mono = crate::resample::to_mono(&float_data, channels);
                        let mut buf = buffer.lock().expect("buffer lock poisoned");
                        buf.extend_from_slice(&mono);
                    },
                    err_fn,
                    None,
                )
            }
            fmt => {
                return Err(CaptureError::DeviceError(format!(
                    "unsupported sample format: {fmt:?}"
                )));
            }
        }
        .map_err(|e| CaptureError::StreamError(e.to_string()))?;

        stream
            .play()
            .map_err(|e| CaptureError::StreamError(e.to_string()))?;

        self.stream = Some(stream);
        self.recording = true;

        Ok(())
    }

    fn stop_recording(&mut self) -> Result<AudioBuffer, CaptureError> {
        if !self.recording {
            return Err(CaptureError::StreamError(
                "not currently recording".to_string(),
            ));
        }

        // Drop the stream to stop recording.
        if let Some(stream) = self.stream.take() {
            let _ = stream.pause();
            drop(stream);
        }
        self.recording = false;

        let raw_samples = {
            let mut buf = self.buffer.lock().expect("buffer lock poisoned");
            std::mem::take(&mut *buf)
        };

        info!(
            "Captured {} mono samples at {} Hz",
            raw_samples.len(),
            self.device_sample_rate,
        );

        // Resample to 16 kHz.
        let samples = crate::resample::resample(
            &raw_samples,
            self.device_sample_rate,
            AudioBuffer::TARGET_SAMPLE_RATE,
        );

        info!("Resampled to {} samples at 16 kHz", samples.len());

        Ok(AudioBuffer {
            samples,
            sample_rate: AudioBuffer::TARGET_SAMPLE_RATE,
        })
    }

    fn is_recording(&self) -> bool {
        self.recording
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_list_devices() {
        let mut mock = MockAudioCapture::new();
        mock.expect_list_devices()
            .returning(|| Ok(vec!["Device A".into(), "Device B".into()]));

        let devices = mock.list_devices().unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0], "Device A");
    }

    #[test]
    fn mock_recording_lifecycle() {
        let mut mock = MockAudioCapture::new();

        mock.expect_is_recording().returning(|| false);
        assert!(!mock.is_recording());

        mock.expect_start_recording().returning(|| Ok(()));
        assert!(mock.start_recording().is_ok());

        mock.expect_stop_recording().returning(|| {
            Ok(AudioBuffer {
                samples: vec![0.0; 16000],
                sample_rate: 16000,
            })
        });
        let buffer = mock.stop_recording().unwrap();
        assert_eq!(buffer.sample_rate, 16000);
        assert_eq!(buffer.samples.len(), 16000);
    }

    #[test]
    fn mock_no_input_device() {
        let mut mock = MockAudioCapture::new();
        mock.expect_start_recording()
            .returning(|| Err(CaptureError::NoInputDevice));

        let result = mock.start_recording();
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), CaptureError::NoInputDevice));
    }

    #[test]
    fn audio_buffer_target_rate() {
        assert_eq!(AudioBuffer::TARGET_SAMPLE_RATE, 16_000);
    }
}
