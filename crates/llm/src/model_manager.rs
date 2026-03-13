//! LLM model download and lifecycle management.
//!
//! [`LlmModelManager`] handles downloading model files from HuggingFace,
//! verifying their integrity, and managing them on disk.

use std::path::PathBuf;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use thiserror::Error;
use tracing::{debug, info, warn};

use crate::model_registry::LlmModelFile;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during LLM model management operations.
#[derive(Debug, Error)]
pub enum LlmModelManagerError {
    /// A network or HTTP error occurred while downloading.
    #[error("download error: {0}")]
    DownloadError(String),

    /// An I/O error occurred.
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    /// The downloaded file's SHA-256 hash did not match the expected value.
    #[error("integrity check failed: expected {expected}, got {actual}")]
    IntegrityError { expected: String, actual: String },

    /// File not found.
    #[error("file not found: {0}")]
    FileNotFound(String),
}

// ---------------------------------------------------------------------------
// LlmModelManager
// ---------------------------------------------------------------------------

/// Manages LLM model files on the local filesystem.
pub struct LlmModelManager {
    /// Directory where model files are stored.
    models_dir: PathBuf,
}

impl LlmModelManager {
    /// Creates a new `LlmModelManager` that stores files under the given directory.
    pub fn new(models_dir: PathBuf) -> Self {
        Self { models_dir }
    }

    /// Returns the full path where the given file would be stored on disk.
    pub fn file_path(&self, file: &LlmModelFile) -> PathBuf {
        self.models_dir.join(file.filename)
    }

    /// Returns `true` if the file already exists on disk.
    pub fn is_downloaded(&self, file: &LlmModelFile) -> bool {
        self.file_path(file).is_file()
    }

    /// Returns `true` if all required files are downloaded.
    pub fn is_ready(&self) -> bool {
        crate::model_registry::all_files()
            .iter()
            .all(|f| self.is_downloaded(f))
    }

    /// Downloads a single file from its HuggingFace URL.
    pub async fn download(
        &self,
        file: &LlmModelFile,
        progress: Option<Arc<dyn Fn(u64, u64) + Send + Sync>>,
    ) -> Result<PathBuf, LlmModelManagerError> {
        std::fs::create_dir_all(&self.models_dir)?;

        let dest = self.file_path(file);
        info!("Downloading '{}' to {}", file.name, dest.display());

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(3600))
            .build()
            .map_err(|e| LlmModelManagerError::DownloadError(e.to_string()))?;

        let response = client
            .get(file.url)
            .send()
            .await
            .map_err(|e| LlmModelManagerError::DownloadError(e.to_string()))?;

        if !response.status().is_success() {
            return Err(LlmModelManagerError::DownloadError(format!(
                "HTTP {} for {}",
                response.status(),
                file.url
            )));
        }

        let total_size = response.content_length().unwrap_or(0);
        debug!("Content-Length: {} bytes", total_size);

        use futures_util::StreamExt;
        let mut stream = response.bytes_stream();
        let mut out_file = tokio::fs::File::create(&dest)
            .await
            .map_err(LlmModelManagerError::IoError)?;

        let mut hasher = Sha256::new();
        let mut downloaded: u64 = 0;

        use tokio::io::AsyncWriteExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| LlmModelManagerError::DownloadError(e.to_string()))?;
            out_file
                .write_all(&chunk)
                .await
                .map_err(LlmModelManagerError::IoError)?;
            hasher.update(&chunk);
            downloaded += chunk.len() as u64;

            if let Some(ref cb) = progress {
                cb(downloaded, total_size);
            }
        }

        out_file
            .flush()
            .await
            .map_err(LlmModelManagerError::IoError)?;
        drop(out_file);

        info!(
            "Download complete: {} bytes written to {}",
            downloaded,
            dest.display()
        );

        let actual_hash = hex::encode(hasher.finalize());
        if actual_hash != file.sha256 {
            warn!(
                "Integrity check failed for '{}': expected {}, got {}",
                file.name, file.sha256, actual_hash
            );
            let _ = std::fs::remove_file(&dest);
            return Err(LlmModelManagerError::IntegrityError {
                expected: file.sha256.to_string(),
                actual: actual_hash,
            });
        }

        Ok(dest)
    }

    /// Downloads all required files. Progress callback receives aggregate bytes.
    pub async fn download_all(
        &self,
        progress: Option<Arc<dyn Fn(u64, u64) + Send + Sync>>,
    ) -> Result<(), LlmModelManagerError> {
        let files = crate::model_registry::all_files();
        let total_bytes: u64 = files.iter().map(|f| f.size_bytes).sum();
        let mut cumulative: u64 = 0;

        for file in files {
            if self.is_downloaded(file) {
                cumulative += file.size_bytes;
                if let Some(ref cb) = progress {
                    cb(cumulative, total_bytes);
                }
                continue;
            }

            let offset = cumulative;
            let cb_clone = progress.clone();
            let per_file_cb: Option<Arc<dyn Fn(u64, u64) + Send + Sync>> =
                cb_clone.map(|cb| -> Arc<dyn Fn(u64, u64) + Send + Sync> {
                    Arc::new(move |downloaded, _total| {
                        cb(offset + downloaded, total_bytes);
                    })
                });

            self.download(file, per_file_cb).await?;
            cumulative += file.size_bytes;
        }

        Ok(())
    }

    /// Deletes a file from disk.
    pub fn delete(&self, file: &LlmModelFile) -> Result<(), LlmModelManagerError> {
        let path = self.file_path(file);
        if !path.exists() {
            return Err(LlmModelManagerError::FileNotFound(format!(
                "file not found: {}",
                path.display()
            )));
        }
        std::fs::remove_file(&path)?;
        info!("Deleted '{}' from {}", file.name, path.display());
        Ok(())
    }

    /// Deletes all model files from disk.
    pub fn delete_all(&self) -> Result<(), LlmModelManagerError> {
        for file in crate::model_registry::all_files() {
            if self.is_downloaded(file) {
                self.delete(file)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_registry;

    #[test]
    fn file_path_construction() {
        let mgr = LlmModelManager::new(PathBuf::from("/tmp/llm-models"));
        let file = model_registry::model_file();
        let path = mgr.file_path(file);
        assert_eq!(
            path,
            PathBuf::from("/tmp/llm-models/qwen2.5-0.5b-instruct-q4_k_m.gguf")
        );
    }

    #[test]
    fn is_downloaded_false_when_not_present() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = LlmModelManager::new(dir.path().to_path_buf());
        let file = model_registry::model_file();
        assert!(!mgr.is_downloaded(file));
    }

    #[test]
    fn is_downloaded_true_when_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = LlmModelManager::new(dir.path().to_path_buf());
        let file = model_registry::tokenizer_file();
        std::fs::write(mgr.file_path(file), b"fake data").unwrap();
        assert!(mgr.is_downloaded(file));
    }

    #[test]
    fn is_ready_false_when_incomplete() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = LlmModelManager::new(dir.path().to_path_buf());
        assert!(!mgr.is_ready());
    }

    #[test]
    fn is_ready_true_when_all_present() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = LlmModelManager::new(dir.path().to_path_buf());
        for file in model_registry::all_files() {
            std::fs::write(mgr.file_path(file), b"fake").unwrap();
        }
        assert!(mgr.is_ready());
    }

    #[test]
    fn delete_removes_file() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = LlmModelManager::new(dir.path().to_path_buf());
        let file = model_registry::tokenizer_file();
        std::fs::write(mgr.file_path(file), b"fake").unwrap();
        assert!(mgr.is_downloaded(file));
        mgr.delete(file).unwrap();
        assert!(!mgr.is_downloaded(file));
    }

    #[test]
    fn delete_nonexistent_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = LlmModelManager::new(dir.path().to_path_buf());
        let file = model_registry::model_file();
        let result = mgr.delete(file);
        assert!(result.is_err());
    }

    #[test]
    fn delete_all_clears_everything() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = LlmModelManager::new(dir.path().to_path_buf());
        for file in model_registry::all_files() {
            std::fs::write(mgr.file_path(file), b"fake").unwrap();
        }
        assert!(mgr.is_ready());
        mgr.delete_all().unwrap();
        assert!(!mgr.is_ready());
    }
}
