//! LLM model metadata registry.
//!
//! Contains static metadata for files required by the local LLM (Qwen2.5-0.5B-Instruct),
//! including download URLs, sizes, and integrity hashes.

/// Metadata describing a single downloadable file.
#[derive(Debug, Clone)]
pub struct LlmModelFile {
    /// Short identifier for this file (e.g. "qwen2.5-0.5b-instruct", "tokenizer").
    pub name: &'static str,
    /// Filename on disk.
    pub filename: &'static str,
    /// Full download URL (HuggingFace).
    pub url: &'static str,
    /// Approximate download size in bytes.
    pub size_bytes: u64,
    /// SHA-256 hash for integrity verification.
    pub sha256: &'static str,
}

/// Returns a static slice of all files needed for the LLM.
pub fn all_files() -> &'static [LlmModelFile] {
    static FILES: &[LlmModelFile] = &[
        LlmModelFile {
            name: "qwen2.5-0.5b-instruct",
            filename: "qwen2.5-0.5b-instruct-q4_k_m.gguf",
            url: "https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF/resolve/main/qwen2.5-0.5b-instruct-q4_k_m.gguf",
            size_bytes: 386_392_064,
            sha256: "f0ca28e0567a83e0e9e5dda16d19d013c0f5c870b1a9a31847cf0e61b5e0fa4d",
        },
        LlmModelFile {
            name: "tokenizer",
            filename: "tokenizer.json",
            url: "https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct/resolve/main/tokenizer.json",
            size_bytes: 7_031_645,
            sha256: "e72fde30522243110a5e74a8afd77cb9f0d1b14b55dba87cb8070cbc10bff7e4",
        },
    ];

    FILES
}

/// Finds a file by its short name.
pub fn find_file(name: &str) -> Option<&'static LlmModelFile> {
    all_files().iter().find(|f| f.name == name)
}

/// Returns the main model GGUF file entry.
pub fn model_file() -> &'static LlmModelFile {
    find_file("qwen2.5-0.5b-instruct").expect("model file must exist in registry")
}

/// Returns the tokenizer file entry.
pub fn tokenizer_file() -> &'static LlmModelFile {
    find_file("tokenizer").expect("tokenizer file must exist in registry")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_files_has_two_entries() {
        assert_eq!(all_files().len(), 2);
    }

    #[test]
    fn find_model_file() {
        let f = find_file("qwen2.5-0.5b-instruct").expect("should find model");
        assert!(f.filename.ends_with(".gguf"));
    }

    #[test]
    fn find_tokenizer_file() {
        let f = find_file("tokenizer").expect("should find tokenizer");
        assert_eq!(f.filename, "tokenizer.json");
    }

    #[test]
    fn find_nonexistent_returns_none() {
        assert!(find_file("nonexistent").is_none());
    }

    #[test]
    fn model_file_helper() {
        let f = model_file();
        assert_eq!(f.name, "qwen2.5-0.5b-instruct");
    }

    #[test]
    fn tokenizer_file_helper() {
        let f = tokenizer_file();
        assert_eq!(f.name, "tokenizer");
    }

    #[test]
    fn all_files_have_nonempty_filenames() {
        for f in all_files() {
            assert!(!f.filename.is_empty(), "file {} has empty filename", f.name);
        }
    }

    #[test]
    fn all_files_have_positive_size() {
        for f in all_files() {
            assert!(f.size_bytes > 0, "file {} has zero size", f.name);
        }
    }

    #[test]
    fn all_files_have_valid_sha256_length() {
        for f in all_files() {
            assert_eq!(
                f.sha256.len(),
                64,
                "file {} sha256 has wrong length: {}",
                f.name,
                f.sha256.len()
            );
        }
    }

    #[test]
    fn all_files_have_valid_urls() {
        for f in all_files() {
            assert!(
                f.url.starts_with("https://huggingface.co/"),
                "file {} has unexpected URL prefix: {}",
                f.name,
                f.url
            );
        }
    }
}
