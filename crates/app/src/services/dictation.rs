//! Dictation service — orchestrates the live dictation flow.
//!
//! The typical lifecycle is: hotkey press -> [`start_recording`] -> user speaks
//! -> hotkey release -> [`stop_and_transcribe`] -> [`auto_paste`] or
//! [`copy_to_clipboard`].

use std::sync::Arc;

use tracing::{error, info};

use linux_whisper_audio::capture::{AudioCapture, CpalCapture};
use linux_whisper_core::config::AppConfig;
use linux_whisper_core::model::Transcript;
use linux_whisper_platform::clipboard::create_clipboard;
use linux_whisper_platform::display;
use linux_whisper_platform::text_inject::create_injector;
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
    /// (e.g. via [`auto_paste`] or [`copy_to_clipboard`]).
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

        // Build transcription options from the current config.
        let language = match self.config.language.as_str() {
            "auto" => None,
            lang => Some(lang.to_string()),
        };

        let options = TranscribeOptions {
            language,
            translate: false,
        };

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

    /// Deliver transcribed text to the user.
    ///
    /// If `config.auto_paste` is enabled the text is typed into the currently
    /// focused window via the display-appropriate text injector (xdotool on
    /// X11, wtype on Wayland).  Otherwise, the text is simply copied to the
    /// system clipboard.
    /// Deliver transcribed text to the user.
    ///
    /// Strategy:
    /// 1. Always copy to clipboard first (so user always has the text).
    /// 2. If auto_paste is enabled, also try to type it into the focused window.
    pub fn auto_paste(config: &AppConfig, text: &str) -> Result<(), Box<dyn std::error::Error>> {
        let display_server = display::detect();
        info!("Display server detected: {display_server}");

        // Always copy to clipboard first — this guarantees the text is available.
        if let Err(e) = Self::copy_to_clipboard_inner(&display_server, text) {
            error!("Clipboard copy failed: {e}");
            // Don't return — still try paste if enabled.
        }

        if config.auto_paste {
            info!("Attempting text injection (display: {display_server})");
            let injector = create_injector(&display_server);

            if injector.is_available() {
                match injector.inject_text(text) {
                    Ok(()) => info!("Text injected successfully ({} chars)", text.len()),
                    Err(e) => error!("Text injection failed: {e} (text is in clipboard)"),
                }
            } else {
                info!("No text injection tool available; text is in clipboard");
            }
        } else {
            info!(
                "Auto-paste disabled; text copied to clipboard ({} chars)",
                text.len()
            );
        }

        Ok(())
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
