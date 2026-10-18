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

        // `forward` takes the position of the first token it is given: 0 for
        // the prompt (which also clears the KV cache left over from the
        // previous call), then the running position of each generated token.
        let mut position = 0;
        for _ in 0..max_tokens {
            let input = Tensor::new(&tokens[..], &self.device)
                .map_err(|e| LlmError::InferenceError(format!("tensor: {e}")))?
                .unsqueeze(0)
                .map_err(|e| LlmError::InferenceError(format!("unsqueeze: {e}")))?;

            let logits = model
                .forward(&input, position)
                .map_err(|e| LlmError::InferenceError(format!("forward: {e}")))?;
            position += tokens.len();

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

        if !is_plausible_reformat(raw_text, &trimmed) {
            return Err(LlmError::InferenceError(
                "output does not look like a reformatting of the input".into(),
            ));
        }

        Ok(trimmed)
    }
}

/// Number words a spoken list turns into list numbers ("first" → "1.").
const NUMBER_WORDS: [&str; 26] = [
    "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "first",
    "second", "third", "fourth", "fifth", "sixth", "seventh", "eighth", "ninth", "tenth",
    "firstly", "secondly", "thirdly", "next", "then", "finally",
];

/// The formatter must only change punctuation, capitals and layout (and turn
/// spoken lists into numbered ones). Reject output that is empty, much
/// shorter or longer than the input, or that drops many of its words — the
/// caller then falls back to the basic formatter. Numbers and number words
/// are left out of the comparison, since list numbering replaces them.
fn is_plausible_reformat(input: &str, output: &str) -> bool {
    let words = |s: &str| -> Vec<String> {
        s.split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(|w| w.to_lowercase())
            .filter(|w| {
                !w.chars().all(|c| c.is_ascii_digit()) && !NUMBER_WORDS.contains(&w.as_str())
            })
            .collect()
    };
    let in_words = words(input);
    let out_words = words(output);
    if in_words.is_empty() {
        return out_words.is_empty();
    }
    if out_words.is_empty() {
        return false;
    }
    let ratio = out_words.len() as f64 / in_words.len() as f64;
    if !(0.8..=1.3).contains(&ratio) {
        return false;
    }
    let kept = in_words.iter().filter(|w| out_words.contains(w)).count();
    kept as f64 / in_words.len() as f64 >= 0.85
}

/// Pick the most likely next token (greedy decoding, so the same input always
/// gives the same output).
fn sample_logits(logits: &Tensor) -> candle_core::Result<u32> {
    // `forward` returns the logits of the last position only: [batch, vocab].
    logits
        .squeeze(0)?
        .to_dtype(candle_core::DType::F32)?
        .argmax(0)?
        .to_scalar::<u32>()
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
    fn reformat_check_accepts_punctuation_changes() {
        assert!(is_plausible_reformat(
            "so first buy milk second call mom",
            "So, first: buy milk.\n2. Call Mom."
        ));
    }

    #[test]
    fn reformat_check_accepts_numbered_lists() {
        assert!(is_plausible_reformat(
            "first buy milk second call mom third pay rent",
            "1. Buy milk\n2. Call mom\n3. Pay rent"
        ));
    }

    #[test]
    fn reformat_check_rejects_rewrites() {
        assert!(!is_plausible_reformat("send the report tomorrow", ""));
        assert!(!is_plausible_reformat(
            "send the report tomorrow",
            "Sure! Here is your formatted text: I will happily send the report tomorrow morning."
        ));
        assert!(!is_plausible_reformat(
            "send the report tomorrow morning please",
            "Dispatch documents."
        ));
    }

    #[test]
    fn format_text_without_model_errors() {
        let mut engine = LlmEngine::new();
        let result = engine.format_text("hello world");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), LlmError::ModelNotLoaded));
    }
}
