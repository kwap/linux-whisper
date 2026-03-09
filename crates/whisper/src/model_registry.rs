//! Whisper model metadata registry.
//!
//! Contains static metadata for all supported whisper GGML models, including
//! names, download URLs, approximate sizes, and integrity hashes. Models are
//! sourced from the official ggerganov/whisper.cpp HuggingFace repository.

/// Placeholder SHA-256 hash used until real hashes are verified.
const PLACEHOLDER_SHA256: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// Metadata describing a single whisper GGML model.
#[derive(Debug, Clone)]
pub struct WhisperModel {
    /// Short identifier for the model (e.g. "tiny", "base", "large-v3").
    pub name: &'static str,
    /// Filename of the GGML binary on disk.
    pub filename: &'static str,
    /// Full download URL (HuggingFace).
    pub url: &'static str,
    /// Approximate download size in bytes.
    pub size_bytes: u64,
    /// SHA-256 hash of the model file for integrity verification.
    pub sha256: &'static str,
}

/// Returns a static slice of all available whisper GGML models.
///
/// Models are listed from smallest to largest. URLs point to the official
/// ggerganov/whisper.cpp HuggingFace repository.
pub fn all_models() -> &'static [WhisperModel] {
    static MODELS: &[WhisperModel] = &[
        WhisperModel {
            name: "tiny",
            filename: "ggml-tiny.bin",
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
            size_bytes: 77_704_715,
            sha256: PLACEHOLDER_SHA256,
        },
        WhisperModel {
            name: "tiny.en",
            filename: "ggml-tiny.en.bin",
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin",
            size_bytes: 77_704_715,
            sha256: PLACEHOLDER_SHA256,
        },
        WhisperModel {
            name: "base",
            filename: "ggml-base.bin",
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
            size_bytes: 147_964_211,
            sha256: PLACEHOLDER_SHA256,
        },
        WhisperModel {
            name: "base.en",
            filename: "ggml-base.en.bin",
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin",
            size_bytes: 147_964_211,
            sha256: PLACEHOLDER_SHA256,
        },
        WhisperModel {
            name: "small",
            filename: "ggml-small.bin",
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
            size_bytes: 487_601_967,
            sha256: PLACEHOLDER_SHA256,
        },
        WhisperModel {
            name: "small.en",
            filename: "ggml-small.en.bin",
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin",
            size_bytes: 487_601_967,
            sha256: PLACEHOLDER_SHA256,
        },
        WhisperModel {
            name: "medium",
            filename: "ggml-medium.bin",
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin",
            size_bytes: 1_533_774_781,
            sha256: PLACEHOLDER_SHA256,
        },
        WhisperModel {
            name: "medium.en",
            filename: "ggml-medium.en.bin",
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.en.bin",
            size_bytes: 1_533_774_781,
            sha256: PLACEHOLDER_SHA256,
        },
        WhisperModel {
            name: "large-v3",
            filename: "ggml-large-v3.bin",
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin",
            size_bytes: 3_094_623_691,
            sha256: PLACEHOLDER_SHA256,
        },
    ];

    MODELS
}

/// Finds a model by its short name (e.g. "base", "tiny.en", "large-v3").
///
/// Returns `None` if no model with the given name exists in the registry.
pub fn find_model(name: &str) -> Option<&'static WhisperModel> {
    all_models().iter().find(|m| m.name == name)
}

/// Returns the default model ("base").
///
/// # Panics
///
/// Panics if the "base" model is not present in the registry. This should
/// never happen as it is always included in [`all_models`].
pub fn default_model() -> &'static WhisperModel {
    find_model("base").expect("default model 'base' must exist in registry")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_models_is_not_empty() {
        assert!(!all_models().is_empty());
    }

    #[test]
    fn all_models_contains_at_least_nine() {
        assert!(all_models().len() >= 9);
    }

    #[test]
    fn find_model_tiny() {
        let model = find_model("tiny").expect("tiny should exist");
        assert_eq!(model.name, "tiny");
        assert_eq!(model.filename, "ggml-tiny.bin");
    }

    #[test]
    fn find_model_base() {
        let model = find_model("base").expect("base should exist");
        assert_eq!(model.name, "base");
        assert_eq!(model.filename, "ggml-base.bin");
    }

    #[test]
    fn find_model_base_en() {
        let model = find_model("base.en").expect("base.en should exist");
        assert_eq!(model.name, "base.en");
        assert_eq!(model.filename, "ggml-base.en.bin");
    }

    #[test]
    fn find_model_small() {
        let model = find_model("small").expect("small should exist");
        assert_eq!(model.name, "small");
    }

    #[test]
    fn find_model_small_en() {
        let model = find_model("small.en").expect("small.en should exist");
        assert_eq!(model.name, "small.en");
    }

    #[test]
    fn find_model_medium() {
        let model = find_model("medium").expect("medium should exist");
        assert_eq!(model.name, "medium");
    }

    #[test]
    fn find_model_medium_en() {
        let model = find_model("medium.en").expect("medium.en should exist");
        assert_eq!(model.name, "medium.en");
    }

    #[test]
    fn find_model_large_v3() {
        let model = find_model("large-v3").expect("large-v3 should exist");
        assert_eq!(model.name, "large-v3");
        assert_eq!(model.filename, "ggml-large-v3.bin");
    }

    #[test]
    fn find_model_invalid_returns_none() {
        assert!(find_model("nonexistent").is_none());
        assert!(find_model("").is_none());
        assert!(find_model("huge-v9").is_none());
    }

    #[test]
    fn default_model_is_base() {
        let model = default_model();
        assert_eq!(model.name, "base");
    }

    #[test]
    fn all_models_have_nonempty_filenames() {
        for model in all_models() {
            assert!(
                !model.filename.is_empty(),
                "model {} has empty filename",
                model.name
            );
        }
    }

    #[test]
    fn all_models_have_positive_size() {
        for model in all_models() {
            assert!(
                model.size_bytes > 0,
                "model {} has zero size",
                model.name
            );
        }
    }

    #[test]
    fn all_models_have_valid_sha256_length() {
        for model in all_models() {
            assert_eq!(
                model.sha256.len(),
                64,
                "model {} sha256 has wrong length: {}",
                model.name,
                model.sha256.len()
            );
        }
    }

    #[test]
    fn all_model_names_are_unique() {
        let models = all_models();
        for (i, a) in models.iter().enumerate() {
            for (j, b) in models.iter().enumerate() {
                if i != j {
                    assert_ne!(a.name, b.name, "duplicate model name: {}", a.name);
                }
            }
        }
    }

    #[test]
    fn all_models_have_valid_urls() {
        for model in all_models() {
            assert!(
                model.url.starts_with("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/"),
                "model {} has unexpected URL prefix: {}",
                model.name,
                model.url
            );
            assert!(
                model.url.ends_with(model.filename),
                "model {} URL does not end with filename",
                model.name
            );
        }
    }
}
