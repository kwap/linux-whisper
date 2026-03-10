//! Transcription service — orchestrates file-based transcription.
//!
//! The typical lifecycle is: user selects an audio file -> [`transcribe_file`]
//! decodes and transcribes it -> [`export_transcript`] formats the result ->
//! [`save_export`] writes it to disk.

use std::path::Path;
use std::sync::Arc;

use tracing::{error, info};

use linux_whisper_audio::decode::decode_to_mono_16khz;
use linux_whisper_core::export::{export, ExportFormat};
use linux_whisper_core::model::Transcript;
use linux_whisper_whisper::engine::TranscribeOptions;
use linux_whisper_whisper::worker::WhisperWorker;

// ---------------------------------------------------------------------------
// TranscriptionService
// ---------------------------------------------------------------------------

/// Orchestrates the file transcription flow: decode an audio file, send it
/// to the whisper worker for transcription, and optionally export the result
/// to various formats.
pub struct TranscriptionService {
    /// Shared handle to the background whisper inference worker.
    worker: Arc<WhisperWorker>,
}

impl TranscriptionService {
    /// Create a new `TranscriptionService` backed by the given whisper worker.
    pub fn new(worker: Arc<WhisperWorker>) -> Self {
        Self { worker }
    }

    /// Decode an audio file to mono 16 kHz and transcribe it.
    ///
    /// # Arguments
    ///
    /// * `path`     - Path to the source audio file (wav, mp3, ogg, m4a, etc.).
    /// * `language` - Optional ISO 639-1 language code. `None` means
    ///                auto-detect.
    ///
    /// Returns a [`Transcript`] containing timed segments on success.
    pub async fn transcribe_file(
        &self,
        path: &Path,
        language: Option<String>,
    ) -> Result<Transcript, Box<dyn std::error::Error>> {
        info!("Transcribing file: {}", path.display());

        // Decode the file into a mono 16 kHz AudioBuffer suitable for whisper.
        let audio = decode_to_mono_16khz(path).map_err(|e| {
            error!("Failed to decode audio file {}: {e}", path.display());
            e
        })?;

        info!(
            "Decoded {} to {} samples at {} Hz",
            path.display(),
            audio.samples.len(),
            audio.sample_rate,
        );

        let options = TranscribeOptions {
            language,
            translate: false,
        };

        let transcript = self.worker.transcribe(audio, options).await.map_err(|e| {
            error!("Transcription failed for {}: {e}", path.display());
            e
        })?;

        info!(
            "Transcription complete — {} segment(s), {:.1}s",
            transcript.segment_count(),
            transcript.duration,
        );

        Ok(transcript)
    }

    /// Export a transcript to the requested text format (TXT, SRT, VTT, CSV).
    ///
    /// Returns the formatted string on success.
    pub fn export_transcript(
        transcript: &Transcript,
        format: ExportFormat,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let content = export(transcript, format)?;
        info!(
            "Exported transcript ({} segment(s)) to {:?} format ({} bytes)",
            transcript.segment_count(),
            format,
            content.len(),
        );
        Ok(content)
    }

    /// Write an exported string to a file on disk.
    ///
    /// Creates the file (and any missing parent directories) if it does not
    /// already exist; overwrites the file if it does.
    pub fn save_export(content: &str, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        // Ensure the parent directory exists.
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(path, content)?;
        info!(
            "Saved export to {} ({} bytes)",
            path.display(),
            content.len(),
        );
        Ok(())
    }
}
