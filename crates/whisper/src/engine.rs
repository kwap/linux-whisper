//! Whisper engine trait and transcription types.
//!
//! Defines the [`WhisperEngine`] trait that abstracts over the actual
//! whisper-rs inference backend. Concrete implementations will be provided
//! once whisper-rs is integrated; for now a mock implementation is available
//! in tests via `mockall`.

use std::path::Path;

use linux_whisper_audio::capture::AudioBuffer;
use linux_whisper_core::model::Transcript;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during whisper transcription.
#[derive(Debug, Error)]
pub enum TranscribeError {
    /// No model has been loaded into the engine.
    #[error("no model loaded")]
    ModelNotLoaded,

    /// The transcription process failed.
    #[error("transcription failed: {0}")]
    TranscriptionFailed(String),

    /// The supplied audio data is invalid or unsupported.
    #[error("invalid audio: {0}")]
    InvalidAudio(String),
}

// ---------------------------------------------------------------------------
// TranscribeOptions
// ---------------------------------------------------------------------------

/// Options controlling how transcription is performed.
#[derive(Debug, Clone)]
pub struct TranscribeOptions {
    /// ISO 639-1 language code (e.g. "en", "de"). `None` means auto-detect.
    pub language: Option<String>,
    /// If `true`, translate non-English speech to English.
    pub translate: bool,
}

impl Default for TranscribeOptions {
    fn default() -> Self {
        Self {
            language: None,
            translate: false,
        }
    }
}

// ---------------------------------------------------------------------------
// WhisperEngine trait
// ---------------------------------------------------------------------------

/// Trait abstracting the whisper inference engine.
///
/// Implementations wrap the underlying whisper-rs (or equivalent) library
/// and provide model loading and audio transcription.
#[cfg_attr(test, mockall::automock)]
pub trait WhisperEngine: Send {
    /// Loads a GGML model file from the given path.
    ///
    /// This must be called before [`transcribe`](WhisperEngine::transcribe).
    fn load_model(&mut self, model_path: &Path) -> Result<(), TranscribeError>;

    /// Transcribes the given audio buffer using the loaded model.
    ///
    /// Returns a [`Transcript`] containing timed segments on success.
    fn transcribe(
        &self,
        audio: &AudioBuffer,
        options: &TranscribeOptions,
    ) -> Result<Transcript, TranscribeError>;

    /// Returns `true` if a model has been loaded and is ready for inference.
    fn is_model_loaded(&self) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;
    use linux_whisper_core::model::{Segment, TranscriptSource};
    use std::path::PathBuf;

    #[test]
    fn transcribe_options_default() {
        let opts = TranscribeOptions::default();
        assert_eq!(opts.language, None);
        assert!(!opts.translate);
    }

    #[test]
    fn transcribe_options_with_language() {
        let opts = TranscribeOptions {
            language: Some("de".to_string()),
            translate: true,
        };
        assert_eq!(opts.language, Some("de".to_string()));
        assert!(opts.translate);
    }

    #[test]
    fn mock_engine_load_model_success() {
        let mut mock = MockWhisperEngine::new();
        mock.expect_load_model().returning(|_| Ok(()));
        mock.expect_is_model_loaded().returning(|| true);

        let path = PathBuf::from("/tmp/models/ggml-base.bin");
        assert!(mock.load_model(&path).is_ok());
        assert!(mock.is_model_loaded());
    }

    #[test]
    fn mock_engine_load_model_failure() {
        let mut mock = MockWhisperEngine::new();
        mock.expect_load_model()
            .returning(|_| Err(TranscribeError::TranscriptionFailed("bad model".into())));
        mock.expect_is_model_loaded().returning(|| false);

        let path = PathBuf::from("/tmp/models/bad.bin");
        assert!(mock.load_model(&path).is_err());
        assert!(!mock.is_model_loaded());
    }

    #[test]
    fn mock_engine_transcribe_success() {
        let mut mock = MockWhisperEngine::new();
        mock.expect_transcribe().returning(|_audio, _opts| {
            let mut transcript = Transcript::new(
                "test",
                Some("en".into()),
                "base",
                TranscriptSource::Dictation,
            );
            transcript.add_segment(Segment::new(0.0, 1.5, "Hello world"));
            transcript.duration = 1.5;
            Ok(transcript)
        });

        let audio = AudioBuffer {
            samples: vec![0.0; 16_000],
            sample_rate: 16_000,
        };
        let opts = TranscribeOptions::default();
        let result = mock.transcribe(&audio, &opts).unwrap();

        assert_eq!(result.segment_count(), 1);
        assert_eq!(result.full_text(), "Hello world");
        assert_eq!(result.model_name, "base");
    }

    #[test]
    fn mock_engine_transcribe_no_model_loaded() {
        let mut mock = MockWhisperEngine::new();
        mock.expect_transcribe()
            .returning(|_, _| Err(TranscribeError::ModelNotLoaded));
        mock.expect_is_model_loaded().returning(|| false);

        let audio = AudioBuffer {
            samples: vec![0.0; 16_000],
            sample_rate: 16_000,
        };
        let opts = TranscribeOptions::default();
        let result = mock.transcribe(&audio, &opts);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TranscribeError::ModelNotLoaded
        ));
        assert!(!mock.is_model_loaded());
    }

    #[test]
    fn mock_engine_transcribe_invalid_audio() {
        let mut mock = MockWhisperEngine::new();
        mock.expect_transcribe()
            .returning(|_, _| Err(TranscribeError::InvalidAudio("empty buffer".into())));

        let audio = AudioBuffer {
            samples: vec![],
            sample_rate: 16_000,
        };
        let opts = TranscribeOptions::default();
        let result = mock.transcribe(&audio, &opts);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TranscribeError::InvalidAudio(_)
        ));
    }
}
