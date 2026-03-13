//! Worker thread for asynchronous LLM inference.
//!
//! [`LlmWorker`] spawns a dedicated OS thread for running synchronous candle
//! inference and exposes an async interface via tokio channels.

use std::path::PathBuf;

use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info};

use crate::engine::{LlmEngine, LlmError};

// ---------------------------------------------------------------------------
// WorkerCommand
// ---------------------------------------------------------------------------

/// Commands sent from the async world to the LLM worker thread.
pub enum LlmWorkerCommand {
    /// Format text using the loaded LLM.
    FormatText {
        text: String,
        reply: oneshot::Sender<Result<String, LlmError>>,
    },
    /// Load LLM model and tokenizer from disk.
    LoadModel {
        model_path: PathBuf,
        tokenizer_path: PathBuf,
        reply: oneshot::Sender<Result<(), LlmError>>,
    },
    /// Gracefully shut down the worker thread.
    Shutdown,
}

// ---------------------------------------------------------------------------
// LlmWorker
// ---------------------------------------------------------------------------

/// Async handle to a background worker thread that performs LLM inference.
#[derive(Clone)]
pub struct LlmWorker {
    sender: mpsc::Sender<LlmWorkerCommand>,
}

impl Default for LlmWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl LlmWorker {
    /// Spawns a new worker thread and returns a handle for communicating with it.
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<LlmWorkerCommand>(32);

        std::thread::Builder::new()
            .name("llm-worker".to_string())
            .spawn(move || {
                Self::worker_loop(rx);
            })
            .expect("failed to spawn LLM worker thread");

        info!("LLM worker thread spawned");

        Self { sender: tx }
    }

    /// Sends text to the worker for LLM formatting and awaits the result.
    pub async fn format_text(&self, text: String) -> Result<String, LlmError> {
        let (tx, rx) = oneshot::channel();

        self.sender
            .send(LlmWorkerCommand::FormatText { text, reply: tx })
            .await
            .map_err(|_| LlmError::InferenceError("worker channel closed".to_string()))?;

        rx.await
            .map_err(|_| LlmError::InferenceError("worker dropped reply channel".to_string()))?
    }

    /// Sends a model-load command to the worker and awaits the result.
    pub async fn load_model(
        &self,
        model_path: PathBuf,
        tokenizer_path: PathBuf,
    ) -> Result<(), LlmError> {
        let (tx, rx) = oneshot::channel();

        self.sender
            .send(LlmWorkerCommand::LoadModel {
                model_path,
                tokenizer_path,
                reply: tx,
            })
            .await
            .map_err(|_| LlmError::InferenceError("worker channel closed".to_string()))?;

        rx.await
            .map_err(|_| LlmError::InferenceError("worker dropped reply channel".to_string()))?
    }

    /// Sends a shutdown command to the worker thread.
    pub async fn shutdown(&self) {
        let _ = self.sender.send(LlmWorkerCommand::Shutdown).await;
        debug!("Shutdown command sent to LLM worker");
    }

    // -----------------------------------------------------------------------
    // Internal worker loop
    // -----------------------------------------------------------------------

    fn worker_loop(mut rx: mpsc::Receiver<LlmWorkerCommand>) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build worker-local tokio runtime");

        let mut engine = LlmEngine::new();

        rt.block_on(async {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    LlmWorkerCommand::FormatText { text, reply } => {
                        debug!("LLM worker: format_text request ({} chars)", text.len());
                        let result = engine.format_text(&text);
                        let _ = reply.send(result);
                    }

                    LlmWorkerCommand::LoadModel {
                        model_path,
                        tokenizer_path,
                        reply,
                    } => {
                        info!("LLM worker: loading model from {}", model_path.display());
                        let result = engine.load(&model_path, &tokenizer_path);
                        let _ = reply.send(result);
                    }

                    LlmWorkerCommand::Shutdown => {
                        info!("LLM worker: shutting down");
                        break;
                    }
                }
            }
        });

        debug!("LLM worker thread exiting");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn worker_creation_and_shutdown() {
        let worker = LlmWorker::new();
        worker.shutdown().await;
    }

    #[tokio::test]
    async fn worker_format_text_without_model_returns_error() {
        let worker = LlmWorker::new();
        let result = worker.format_text("hello world".to_string()).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), LlmError::ModelNotLoaded));
        worker.shutdown().await;
    }

    #[tokio::test]
    async fn worker_load_model_nonexistent_path() {
        let worker = LlmWorker::new();
        let result = worker
            .load_model(
                PathBuf::from("/nonexistent/model.gguf"),
                PathBuf::from("/nonexistent/tokenizer.json"),
            )
            .await;
        assert!(result.is_err());
        worker.shutdown().await;
    }
}
