//! LLM inference engine using candle + Qwen2.5.
//!
//! Wraps candle's quantized Qwen2 model for text formatting inference.

use std::path::Path;

use candle_core::{Device, Tensor};
use candle_transformers::models::quantized_qwen2::ModelWeights;
use thiserror::Error;
use tokenizers::Tokenizer;
use tracing::{debug, info};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during LLM inference.
#[derive(Debug, Error)]
pub enum LlmError {
    /// No model has been loaded.
    #[error("no model loaded")]
    ModelNotLoaded,

    /// Model loading failed.
    #[error("model load failed: {0}")]
    LoadError(String),

    /// Inference failed.
    #[error("inference failed: {0}")]
    InferenceError(String),
}

// ---------------------------------------------------------------------------
// LlmEngine
// ---------------------------------------------------------------------------

/// Engine for running local LLM inference using candle.
pub struct LlmEngine {
    model: Option<ModelWeights>,
    tokenizer: Option<Tokenizer>,
    device: Device,
}

impl Default for LlmEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl LlmEngine {
    /// Creates a new engine without a loaded model.
    pub fn new() -> Self {
        let device = match Device::new_cuda(0) {
            Ok(d) => {
                info!("LLM engine: using CUDA device");
                d
            }
            Err(_) => {
                info!("LLM engine: using CPU device");
                Device::Cpu
            }
        };

        Self {
            model: None,
            tokenizer: None,
            device,
        }
    }

    /// Returns `true` if a model has been loaded and is ready for inference.
    pub fn is_loaded(&self) -> bool {
        self.model.is_some() && self.tokenizer.is_some()
    }

    /// Loads a quantized GGUF model and tokenizer from disk.
    pub fn load(&mut self, model_path: &Path, tokenizer_path: &Path) -> Result<(), LlmError> {
        info!(
            "Loading LLM model from {} and tokenizer from {}",
            model_path.display(),
            tokenizer_path.display()
        );

        // Load GGUF model.
        let mut file = std::fs::File::open(model_path)
            .map_err(|e| LlmError::LoadError(format!("open model: {e}")))?;

        let model_data = candle_core::quantized::gguf_file::Content::read(&mut file)
            .map_err(|e| LlmError::LoadError(format!("read GGUF: {e}")))?;

        let weights = ModelWeights::from_gguf(model_data, &mut file, &self.device)
            .map_err(|e| LlmError::LoadError(format!("build model: {e}")))?;

        // Load tokenizer.
        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| LlmError::LoadError(format!("load tokenizer: {e}")))?;

        self.model = Some(weights);
        self.tokenizer = Some(tokenizer);

        info!("LLM model loaded successfully");
        Ok(())
    }

    /// Formats raw speech-to-text output using the loaded LLM.
    pub fn format_text(&mut self, raw_text: &str) -> Result<String, LlmError> {
        let model = self.model.as_mut().ok_or(LlmError::ModelNotLoaded)?;
        let tokenizer = self.tokenizer.as_ref().ok_or(LlmError::ModelNotLoaded)?;

        // Build ChatML prompt.
        let prompt = format!(
            "<|im_start|>system\n\
             You are a text formatter. You receive raw speech-to-text output and return \
             clean, well-formatted text. Preserve the original meaning exactly. Fix \
             punctuation, capitalization, and paragraph breaks. Convert spoken lists \
             into numbered lists. Do not add, remove, or rephrase any content. Return \
             only the formatted text with no explanation.<|im_end|>\n\
             <|im_start|>user\n\
             {raw_text}<|im_end|>\n\
             <|im_start|>assistant\n"
        );

        let encoding = tokenizer
            .encode(prompt.as_str(), true)
            .map_err(|e| LlmError::InferenceError(format!("tokenize: {e}")))?;
        let input_ids = encoding.get_ids();

        let max_tokens = (raw_text.len() * 2).clamp(256, 2048);
        debug!(
            "LLM inference: {} input tokens, max_tokens={}",
            input_ids.len(),
            max_tokens
        );

        let mut tokens: Vec<u32> = input_ids.to_vec();
        let mut generated = Vec::new();

        let eos_token = tokenizer.token_to_id("<|im_end|>").unwrap_or(u32::MAX);

        for _ in 0..max_tokens {
            let input = Tensor::new(&tokens[..], &self.device)
                .map_err(|e| LlmError::InferenceError(format!("tensor: {e}")))?
                .unsqueeze(0)
                .map_err(|e| LlmError::InferenceError(format!("unsqueeze: {e}")))?;

            let logits = model
                .forward(&input, tokens.len())
                .map_err(|e| LlmError::InferenceError(format!("forward: {e}")))?;

            let next_token = sample_logits(&logits)
                .map_err(|e| LlmError::InferenceError(format!("sample: {e}")))?;

            if next_token == eos_token {
                break;
            }

            generated.push(next_token);
            tokens = vec![next_token];
        }

        let output = tokenizer
            .decode(&generated, true)
            .map_err(|e| LlmError::InferenceError(format!("decode: {e}")))?;

        let trimmed = output.trim().to_string();
        debug!("LLM output: {} chars", trimmed.len());

        Ok(trimmed)
    }
}

/// Sample the next token from logits using near-greedy (temperature ~ 0).
fn sample_logits(logits: &Tensor) -> candle_core::Result<u32> {
    let logits = logits.squeeze(0)?;
    let last_logits = logits.get(logits.dim(0)? - 1)?;

    // Apply temperature = 0.1 for near-deterministic output.
    let scaled = (&last_logits / 0.1f64)?;
    let probs = candle_nn::ops::softmax_last_dim(&scaled)?;

    let probs_vec: Vec<f32> = probs.to_vec1()?;
    let next_token = probs_vec
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(idx, _)| idx as u32)
        .unwrap_or(0);

    Ok(next_token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_new_not_loaded() {
        let engine = LlmEngine::new();
        assert!(!engine.is_loaded());
    }

    #[test]
    fn engine_default_not_loaded() {
        let engine = LlmEngine::default();
        assert!(!engine.is_loaded());
    }

    #[test]
    fn format_text_without_model_errors() {
        let mut engine = LlmEngine::new();
        let result = engine.format_text("hello world");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), LlmError::ModelNotLoaded));
    }
}
