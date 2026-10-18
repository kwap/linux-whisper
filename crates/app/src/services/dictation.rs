//! Dictation service — orchestrates the live dictation flow.
//!
//! The typical lifecycle is: hotkey press -> [`start_recording`] -> user speaks
//! -> hotkey press -> [`stop_and_transcribe`] -> [`deliver`].

use std::sync::Arc;

use tracing::info;

use linux_whisper_audio::capture::{AudioCapture, CpalCapture};
use linux_whisper_core::config::AppConfig;
use linux_whisper_core::model::Transcript;
use linux_whisper_platform::clipboard::create_clipboard;
use linux_whisper_platform::display;
use linux_whisper_platform::text_inject::{deliver_text, Delivery};
use linux_whisper_whisper::engine::TranscribeOptions;
use linux_whisper_whisper::worker::WhisperWorker;

// ---------------------------------------------------------------------------
// DictationService
// ---------------------------------------------------------------------------

/// Orchestrates the live dictation flow: record from microphone, transcribe
/// via the whisper worker, and deliver the result to the user (paste or
/// clipboard).
pub struct DictationService {
    /// Audio capture backend.
    capture: CpalCapture,
    /// Shared handle to the background whisper inference worker.
    worker: Arc<WhisperWorker>,
    /// Application configuration snapshot (language, auto_paste, etc.).
    config: AppConfig,
    /// Whether we are currently recording audio.
    recording: bool,
}

impl DictationService {
    /// Create a new `DictationService`.
    ///
    /// Initialises a [`CpalCapture`] for microphone access and stores
    /// references to the whisper worker and application config.
    pub fn new(
        worker: Arc<WhisperWorker>,
        config: AppConfig,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let capture = CpalCapture::new()?;
        info!("DictationService initialised");

        Ok(Self {
            capture,
            worker,
            config,
            recording: false,
        })
    }

    /// Begin recording audio from the default input device.
    ///
    /// This is a non-blocking call — audio samples are accumulated in the
    /// background by the CPAL stream callback until [`stop_and_transcribe`] is
    /// called.
    pub fn start_recording(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if self.recording {
            info!("start_recording called while already recording; ignoring");
            return Ok(());
        }

        self.capture.start_recording()?;
        self.recording = true;
        info!("Dictation recording started");
        Ok(())
    }

    /// Stop recording, send the captured audio to the whisper worker for
    /// transcription, and return the resulting [`Transcript`].
    ///
    /// The caller is responsible for delivering the transcript text to the user
    /// (e.g. via [`deliver`]).
    pub async fn stop_and_transcribe(&mut self) -> Result<Transcript, Box<dyn std::error::Error>> {
        if !self.recording {
            return Err("not currently recording".into());
        }

        let audio = self.capture.stop_recording()?;
        self.recording = false;

        info!(
            "Dictation recording stopped — {} samples captured",
            audio.samples.len(),
        );

        let options = TranscribeOptions::from_config(&self.config);

        let transcript = self.worker.transcribe(audio, options).await?;

        info!(
            "Dictation transcription complete — {} segment(s), {:.1}s",
            transcript.segment_count(),
            transcript.duration,
        );

        Ok(transcript)
    }

    /// Returns `true` if the service is currently recording audio.
    pub fn is_recording(&self) -> bool {
        self.recording
    }

    /// Deliver transcribed text to the user: copy it to the clipboard and, if
    /// auto-paste is on, insert it into the focused window. Spawns helper
    /// programs and may wait briefly, so call it off the GTK thread.
    pub fn deliver(config: &AppConfig, text: &str) -> Delivery {
        let display_server = display::detect();
        let delivery = deliver_text(
            &display_server,
            text,
            config.auto_paste,
            config.paste_shortcut,
        );
        info!(
            "Delivered {} chars ({display_server}): {delivery:?}",
            text.len()
        );
        delivery
    }

    /// Copy the given text to the system clipboard using the best available
    /// backend for the current display server.
    pub fn copy_to_clipboard(text: &str) -> Result<(), Box<dyn std::error::Error>> {
        let display_server = display::detect();
        Self::copy_to_clipboard_inner(&display_server, text)
    }

    fn copy_to_clipboard_inner(
        display_server: &display::DisplayServer,
        text: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let clipboard = create_clipboard(display_server);
        clipboard.set_text(text)?;
        info!(
            "Copied {} chars to clipboard (display: {display_server})",
            text.len()
        );
        Ok(())
    }
}
