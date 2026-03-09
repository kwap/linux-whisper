//! Worker thread for asynchronous whisper inference.
//!
//! [`WhisperWorker`] spawns a dedicated OS thread for running synchronous
//! whisper-rs inference and exposes an async interface via tokio channels.
//! This keeps the tokio runtime free from blocking compute work.

use std::path::PathBuf;

use linux_whisper_audio::capture::AudioBuffer;
use linux_whisper_core::model::{Segment, Transcript, TranscriptSource};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, error, info};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::engine::{TranscribeError, TranscribeOptions};

// ---------------------------------------------------------------------------
// WorkerCommand
// ---------------------------------------------------------------------------

/// Commands sent from the async world to the worker thread.
pub enum WorkerCommand {
    /// Request transcription of an audio buffer.
    Transcribe {
        /// The audio to transcribe.
        audio: AudioBuffer,
        /// Transcription options (language, translate flag, etc.).
        options: TranscribeOptions,
        /// Channel to send the result back on.
        reply: oneshot::Sender<Result<Transcript, TranscribeError>>,
    },
    /// Load a whisper model from the given path.
    LoadModel {
        /// Path to the GGML model file.
        path: PathBuf,
        /// Channel to send the result back on.
        reply: oneshot::Sender<Result<(), TranscribeError>>,
    },
    /// Gracefully shut down the worker thread.
    Shutdown,
}

// ---------------------------------------------------------------------------
// WhisperWorker
// ---------------------------------------------------------------------------

/// Async handle to a background worker thread that performs whisper inference.
///
/// Commands are sent over an MPSC channel and processed sequentially on a
/// dedicated `std::thread`. Results are returned via oneshot channels.
#[derive(Clone)]
pub struct WhisperWorker {
    sender: mpsc::Sender<WorkerCommand>,
}

impl WhisperWorker {
    /// Spawns a new worker thread and returns a handle for communicating with it.
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<WorkerCommand>(32);

        std::thread::Builder::new()
            .name("whisper-worker".to_string())
            .spawn(move || {
                Self::worker_loop(rx);
            })
            .expect("failed to spawn whisper worker thread");

        info!("Whisper worker thread spawned");

        Self { sender: tx }
    }

    /// Sends an audio buffer to the worker for transcription and awaits the
    /// result.
    pub async fn transcribe(
        &self,
        audio: AudioBuffer,
        options: TranscribeOptions,
    ) -> Result<Transcript, TranscribeError> {
        let (tx, rx) = oneshot::channel();

        self.sender
            .send(WorkerCommand::Transcribe {
                audio,
                options,
                reply: tx,
            })
            .await
            .map_err(|_| {
                TranscribeError::TranscriptionFailed("worker channel closed".to_string())
            })?;

        rx.await.map_err(|_| {
            TranscribeError::TranscriptionFailed("worker dropped reply channel".to_string())
        })?
    }

    /// Sends a model-load command to the worker and awaits the result.
    pub async fn load_model(&self, path: PathBuf) -> Result<(), TranscribeError> {
        let (tx, rx) = oneshot::channel();

        self.sender
            .send(WorkerCommand::LoadModel { path, reply: tx })
            .await
            .map_err(|_| {
                TranscribeError::TranscriptionFailed("worker channel closed".to_string())
            })?;

        rx.await.map_err(|_| {
            TranscribeError::TranscriptionFailed("worker dropped reply channel".to_string())
        })?
    }

    /// Sends a shutdown command to the worker thread.
    ///
    /// The worker will finish processing the current command (if any) and then
    /// exit. This method does not block waiting for the thread to terminate.
    pub async fn shutdown(&self) {
        let _ = self.sender.send(WorkerCommand::Shutdown).await;
        debug!("Shutdown command sent to whisper worker");
    }

    // -----------------------------------------------------------------------
    // Internal worker loop
    // -----------------------------------------------------------------------

    /// The synchronous event loop that runs on the dedicated worker thread.
    ///
    /// Receives commands from the async side and processes them one at a time.
    fn worker_loop(mut rx: mpsc::Receiver<WorkerCommand>) {
        // We need a small tokio runtime *only* to drive the mpsc::Receiver
        // (which is async). An alternative would be std::sync::mpsc, but we
        // use tokio::sync::mpsc for consistency with the rest of the project.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build worker-local tokio runtime");

        let mut ctx: Option<WhisperContext> = None;
        let mut model_name: String = String::new();

        rt.block_on(async {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    WorkerCommand::Transcribe {
                        audio,
                        options,
                        reply,
                    } => {
                        debug!(
                            "Worker: transcribe request ({} samples, lang={:?})",
                            audio.samples.len(),
                            options.language,
                        );

                        let Some(ref whisper_ctx) = ctx else {
                            let _ = reply.send(Err(TranscribeError::ModelNotLoaded));
                            continue;
                        };

                        let result =
                            run_transcription(whisper_ctx, &audio, &options, &model_name);
                        let _ = reply.send(result);
                    }

                    WorkerCommand::LoadModel { path, reply } => {
                        info!("Worker: loading model from {}", path.display());

                        if !path.exists() {
                            let _ = reply.send(Err(TranscribeError::TranscriptionFailed(
                                format!("model file not found: {}", path.display()),
                            )));
                            continue;
                        }

                        let params = WhisperContextParameters::default();
                        match WhisperContext::new_with_params(
                            path.to_str().unwrap_or_default(),
                            params,
                        ) {
                            Ok(new_ctx) => {
                                // Extract model name from filename.
                                model_name = path
                                    .file_stem()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("unknown")
                                    .strip_prefix("ggml-")
                                    .unwrap_or("unknown")
                                    .to_string();

                                info!("Model loaded: {} ({})", model_name, path.display());
                                ctx = Some(new_ctx);
                                let _ = reply.send(Ok(()));
                            }
                            Err(e) => {
                                error!("Failed to load whisper model: {e}");
                                let _ = reply.send(Err(TranscribeError::TranscriptionFailed(
                                    format!("whisper-rs model load failed: {e}"),
                                )));
                            }
                        }
                    }

                    WorkerCommand::Shutdown => {
                        info!("Worker: shutting down");
                        break;
                    }
                }
            }
        });

        debug!("Whisper worker thread exiting");
    }
}

/// Run whisper-rs transcription synchronously. Called on the worker thread.
fn run_transcription(
    ctx: &WhisperContext,
    audio: &AudioBuffer,
    options: &TranscribeOptions,
    model_name: &str,
) -> Result<Transcript, TranscribeError> {
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

    // Set language if specified, otherwise auto-detect.
    if let Some(ref lang) = options.language {
        params.set_language(Some(lang));
    } else {
        params.set_language(None);
    }

    params.set_translate(options.translate);

    // Don't suppress non-speech tokens — they include language markers
    // needed for multilingual transcription.
    params.set_suppress_nst(false);

    // Allow multiple segments so each can detect its own language.
    params.set_single_segment(false);

    // Reset context between segments so language can change.
    params.set_no_context(true);

    // Don't print to stdout.
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);

    // Create a state and run inference.
    let mut state = ctx
        .create_state()
        .map_err(|e| TranscribeError::TranscriptionFailed(format!("create state: {e}")))?;

    state
        .full(params, &audio.samples)
        .map_err(|e| TranscribeError::TranscriptionFailed(format!("inference failed: {e}")))?;

    // Extract segments from the result.
    let num_segments = state.full_n_segments();

    let duration = audio.samples.len() as f64 / audio.sample_rate as f64;

    let mut transcript = Transcript::new(
        "Transcription",
        options.language.clone(),
        model_name,
        TranscriptSource::Dictation,
    );
    transcript.duration = duration;

    for i in 0..num_segments {
        let Some(seg) = state.get_segment(i) else {
            continue;
        };

        let text = seg.to_str_lossy().map_err(|e| {
            TranscribeError::TranscriptionFailed(format!("failed to get segment {i} text: {e}"))
        })?;

        let start = seg.start_timestamp();
        let end = seg.end_timestamp();

        // whisper.cpp timestamps are in centiseconds (10ms units).
        let start_sec = start as f64 / 100.0;
        let end_sec = end as f64 / 100.0;

        let trimmed = text.trim();
        if !trimmed.is_empty() {
            transcript.add_segment(Segment::new(start_sec, end_sec, trimmed));
        }
    }

    info!(
        "Whisper inference complete: {} segment(s), {:.1}s audio",
        transcript.segment_count(),
        duration,
    );

    Ok(transcript)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn worker_creation_and_shutdown() {
        let worker = WhisperWorker::new();
        worker.shutdown().await;
        // If we get here without hanging, the test passes.
    }

    #[tokio::test]
    async fn worker_transcribe_without_model_returns_error() {
        let worker = WhisperWorker::new();

        let audio = AudioBuffer {
            samples: vec![0.0; 16_000],
            sample_rate: 16_000,
        };
        let opts = TranscribeOptions::default();
        let result = worker.transcribe(audio, opts).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TranscribeError::ModelNotLoaded));

        worker.shutdown().await;
    }

    #[tokio::test]
    async fn worker_load_model_nonexistent_path() {
        let worker = WhisperWorker::new();

        let result = worker
            .load_model(PathBuf::from("/nonexistent/path/model.bin"))
            .await;
        assert!(result.is_err());

        worker.shutdown().await;
    }

    #[tokio::test]
    async fn worker_load_model_and_transcribe() {
        // This test requires a real model file. Use the tiny model if available.
        let model_path = tiny_model_path();
        if !model_path.exists() {
            // Skip test silently if model not present.
            return;
        }

        let worker = WhisperWorker::new();

        let load_result = worker.load_model(model_path).await;
        assert!(load_result.is_ok(), "load_model failed: {:?}", load_result.err());

        // Transcribe 1 second of silence.
        let audio = AudioBuffer {
            samples: vec![0.0; 16_000],
            sample_rate: 16_000,
        };
        let opts = TranscribeOptions::default();
        let result = worker.transcribe(audio, opts).await;
        assert!(result.is_ok(), "transcribe failed: {:?}", result.err());

        let transcript = result.unwrap();
        // Silence may produce 0 or more segments depending on model.
        assert!(transcript.duration > 0.0);

        worker.shutdown().await;
    }

    /// Returns the path to the downloaded tiny model, if present.
    fn tiny_model_path() -> PathBuf {
        let data_dir = std::env::var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").expect("HOME not set");
                PathBuf::from(home).join(".local/share")
            });
        data_dir.join("linux-whisper/models/ggml-tiny.bin")
    }

    #[tokio::test]
    async fn worker_integration_real_model() {
        let model_path = tiny_model_path();
        if !model_path.exists() {
            // Skip test silently if model not present.
            return;
        }

        let worker = WhisperWorker::new();

        let load_result = worker.load_model(model_path).await;
        assert!(load_result.is_ok(), "load_model failed: {:?}", load_result.err());

        // Transcribe 1 second of silence.
        let audio = AudioBuffer {
            samples: vec![0.0; 16_000],
            sample_rate: 16_000,
        };
        let opts = TranscribeOptions::default();
        let result = worker.transcribe(audio, opts).await;
        assert!(result.is_ok(), "transcribe failed: {:?}", result.err());

        worker.shutdown().await;
    }
}
