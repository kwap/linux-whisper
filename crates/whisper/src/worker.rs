//! Worker thread for asynchronous whisper inference.
//!
//! [`WhisperWorker`] spawns a dedicated OS thread for running synchronous
//! whisper-rs inference and exposes an async interface via tokio channels.
//! This keeps the tokio runtime free from blocking compute work.
//!
//! Until whisper-rs is integrated the worker returns stub transcription
//! results for testing purposes.

use std::path::PathBuf;

use linux_whisper_audio::capture::AudioBuffer;
use linux_whisper_core::model::{Segment, Transcript, TranscriptSource};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info};

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
    /// Currently returns stub results; real whisper-rs inference will be
    /// plugged in here later.
    fn worker_loop(mut rx: mpsc::Receiver<WorkerCommand>) {
        // We need a small tokio runtime *only* to drive the mpsc::Receiver
        // (which is async). An alternative would be std::sync::mpsc, but we
        // use tokio::sync::mpsc for consistency with the rest of the project.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build worker-local tokio runtime");

        let mut _model_loaded = false;
        let mut _model_path: Option<PathBuf> = None;

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

                        if !_model_loaded {
                            let _ = reply.send(Err(TranscribeError::ModelNotLoaded));
                            continue;
                        }

                        // Stub: return a placeholder transcript.
                        let duration = audio.samples.len() as f64
                            / audio.sample_rate as f64;

                        let mut transcript = Transcript::new(
                            "Transcription",
                            options.language.clone(),
                            "stub",
                            TranscriptSource::Dictation,
                        );
                        transcript.duration = duration;
                        transcript.add_segment(Segment::new(
                            0.0,
                            duration,
                            "[stub transcription - whisper-rs not yet integrated]",
                        ));

                        let _ = reply.send(Ok(transcript));
                    }

                    WorkerCommand::LoadModel { path, reply } => {
                        info!("Worker: loading model from {}", path.display());

                        // Stub: just record that a model is loaded.
                        if path.exists() {
                            _model_loaded = true;
                            _model_path = Some(path);
                            let _ = reply.send(Ok(()));
                        } else {
                            let _ = reply.send(Err(TranscribeError::TranscriptionFailed(
                                format!("model file not found: {}", path.display()),
                            )));
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
        let dir = tempfile::tempdir().unwrap();
        let model_path = dir.path().join("fake-model.bin");
        std::fs::write(&model_path, b"fake model data").unwrap();

        let worker = WhisperWorker::new();

        // Load the fake model.
        let load_result = worker.load_model(model_path).await;
        assert!(load_result.is_ok());

        // Transcribe with the stub engine.
        let audio = AudioBuffer {
            samples: vec![0.0; 16_000],
            sample_rate: 16_000,
        };
        let opts = TranscribeOptions::default();
        let result = worker.transcribe(audio, opts).await;
        assert!(result.is_ok());

        let transcript = result.unwrap();
        assert_eq!(transcript.segment_count(), 1);
        assert!((transcript.duration - 1.0).abs() < f64::EPSILON);

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
            panic!(
                "Tiny model not found at {}. Download it first.",
                model_path.display()
            );
        }

        let worker = WhisperWorker::new();

        // Load the real model file.
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
        assert_eq!(transcript.segment_count(), 1);
        assert!((transcript.duration - 1.0).abs() < f64::EPSILON);

        worker.shutdown().await;
    }
}
