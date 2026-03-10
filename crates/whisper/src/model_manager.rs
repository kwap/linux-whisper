//! Model download and lifecycle management.
//!
//! [`ModelManager`] handles downloading whisper GGML models from HuggingFace,
//! verifying their integrity, and managing them on disk.

use std::path::PathBuf;

use sha2::{Digest, Sha256};
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::model_registry::WhisperModel;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during model management operations.
#[derive(Debug, Error)]
pub enum ModelManagerError {
    /// A network or HTTP error occurred while downloading a model.
    #[error("download error: {0}")]
    DownloadError(String),

    /// An I/O error occurred (e.g. writing the model file to disk).
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    /// The downloaded file's SHA-256 hash did not match the expected value.
    #[error("integrity check failed: expected {expected}, got {actual}")]
    IntegrityError {
        /// The expected SHA-256 hash from the model registry.
        expected: String,
        /// The actual SHA-256 hash of the downloaded file.
        actual: String,
    },

    /// No model with the given name was found in the registry.
    #[error("model not found: {0}")]
    ModelNotFound(String),
}

// ---------------------------------------------------------------------------
// Progress callback
// ---------------------------------------------------------------------------

/// Callback invoked during model downloads to report progress.
///
/// Arguments are `(bytes_downloaded, total_bytes)`. If the total size is
/// unknown the second argument will be `0`.
pub type ProgressCallback = Box<dyn Fn(u64, u64) + Send>;

// ---------------------------------------------------------------------------
// ModelManager
// ---------------------------------------------------------------------------

/// Manages whisper model files on the local filesystem.
///
/// Provides methods to download, verify, list, and delete GGML model files.
pub struct ModelManager {
    /// Directory where model files are stored.
    models_dir: PathBuf,
}

impl ModelManager {
    /// Creates a new `ModelManager` that stores models under the given directory.
    ///
    /// The directory will be created (including parents) when a download is
    /// initiated if it does not already exist.
    pub fn new(models_dir: PathBuf) -> Self {
        Self { models_dir }
    }

    /// Returns the full path where the given model would be stored on disk.
    pub fn model_path(&self, model: &WhisperModel) -> PathBuf {
        self.models_dir.join(model.filename)
    }

    /// Returns `true` if the model file already exists on disk.
    pub fn is_downloaded(&self, model: &WhisperModel) -> bool {
        self.model_path(model).is_file()
    }

    /// Returns a list of models from the registry that are present on disk.
    pub fn list_downloaded(&self) -> Vec<&'static WhisperModel> {
        crate::model_registry::all_models()
            .iter()
            .filter(|m| self.is_downloaded(m))
            .collect()
    }

    /// Downloads a model from its HuggingFace URL to the local models directory.
    ///
    /// If a `progress` callback is provided it will be called periodically with
    /// the number of bytes downloaded so far and the total expected size.
    ///
    /// Returns the path to the downloaded file on success.
    pub async fn download(
        &self,
        model: &WhisperModel,
        progress: Option<ProgressCallback>,
    ) -> Result<PathBuf, ModelManagerError> {
        // Ensure the models directory exists.
        std::fs::create_dir_all(&self.models_dir)?;

        let dest = self.model_path(model);
        info!("Downloading model '{}' to {}", model.name, dest.display());

        let response = reqwest::get(model.url)
            .await
            .map_err(|e| ModelManagerError::DownloadError(e.to_string()))?;

        if !response.status().is_success() {
            return Err(ModelManagerError::DownloadError(format!(
                "HTTP {} for {}",
                response.status(),
                model.url
            )));
        }

        let total_size = response.content_length().unwrap_or(0);
        debug!("Content-Length: {} bytes", total_size);

        // Stream the response body to disk while computing SHA-256.
        use futures_util::StreamExt;
        let mut stream = response.bytes_stream();
        let mut file = tokio::fs::File::create(&dest)
            .await
            .map_err(ModelManagerError::IoError)?;

        let mut hasher = Sha256::new();
        let mut downloaded: u64 = 0;

        use tokio::io::AsyncWriteExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| ModelManagerError::DownloadError(e.to_string()))?;
            file.write_all(&chunk)
                .await
                .map_err(ModelManagerError::IoError)?;
            hasher.update(&chunk);
            downloaded += chunk.len() as u64;

            if let Some(ref cb) = progress {
                cb(downloaded, total_size);
            }
        }

        file.flush().await.map_err(ModelManagerError::IoError)?;
        drop(file);

        info!(
            "Download complete: {} bytes written to {}",
            downloaded,
            dest.display()
        );

        // Verify integrity if a non-placeholder hash is provided.
        let actual_hash = hex::encode(hasher.finalize());
        let placeholder = "0".repeat(64);
        if model.sha256 != placeholder && actual_hash != model.sha256 {
            // Remove the corrupt file.
            warn!(
                "Integrity check failed for '{}': expected {}, got {}",
                model.name, model.sha256, actual_hash
            );
            let _ = std::fs::remove_file(&dest);
            return Err(ModelManagerError::IntegrityError {
                expected: model.sha256.to_string(),
                actual: actual_hash,
            });
        }

        Ok(dest)
    }

    /// Deletes a model file from disk.
    ///
    /// Returns an error if the file does not exist or cannot be removed.
    pub fn delete(&self, model: &WhisperModel) -> Result<(), ModelManagerError> {
        let path = self.model_path(model);
        if !path.exists() {
            return Err(ModelManagerError::ModelNotFound(format!(
                "model file not found: {}",
                path.display()
            )));
        }
        std::fs::remove_file(&path)?;
        info!("Deleted model '{}' from {}", model.name, path.display());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_registry;

    #[test]
    fn model_path_construction() {
        let mgr = ModelManager::new(PathBuf::from("/tmp/models"));
        let model = model_registry::find_model("base").unwrap();
        let path = mgr.model_path(model);
        assert_eq!(path, PathBuf::from("/tmp/models/ggml-base.bin"));
    }

    #[test]
    fn is_downloaded_false_when_not_present() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = ModelManager::new(dir.path().to_path_buf());
        let model = model_registry::find_model("tiny").unwrap();
        assert!(!mgr.is_downloaded(model));
    }

    #[test]
    fn is_downloaded_true_when_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = ModelManager::new(dir.path().to_path_buf());
        let model = model_registry::find_model("tiny").unwrap();

        // Create a dummy file.
        std::fs::write(mgr.model_path(model), b"fake model data").unwrap();
        assert!(mgr.is_downloaded(model));
    }

    #[test]
    fn list_downloaded_empty_initially() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = ModelManager::new(dir.path().to_path_buf());
        assert!(mgr.list_downloaded().is_empty());
    }

    #[test]
    fn list_downloaded_finds_present_models() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = ModelManager::new(dir.path().to_path_buf());

        // Place two fake model files.
        let tiny = model_registry::find_model("tiny").unwrap();
        let base = model_registry::find_model("base").unwrap();
        std::fs::write(mgr.model_path(tiny), b"fake").unwrap();
        std::fs::write(mgr.model_path(base), b"fake").unwrap();

        let downloaded = mgr.list_downloaded();
        assert_eq!(downloaded.len(), 2);

        let names: Vec<&str> = downloaded.iter().map(|m| m.name).collect();
        assert!(names.contains(&"tiny"));
        assert!(names.contains(&"base"));
    }

    #[test]
    fn delete_removes_model_file() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = ModelManager::new(dir.path().to_path_buf());
        let model = model_registry::find_model("small").unwrap();

        // Create a dummy file.
        std::fs::write(mgr.model_path(model), b"fake model").unwrap();
        assert!(mgr.is_downloaded(model));

        mgr.delete(model).unwrap();
        assert!(!mgr.is_downloaded(model));
    }

    #[test]
    fn delete_nonexistent_model_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = ModelManager::new(dir.path().to_path_buf());
        let model = model_registry::find_model("medium").unwrap();

        let result = mgr.delete(model);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ModelManagerError::ModelNotFound(_)
        ));
    }
}
