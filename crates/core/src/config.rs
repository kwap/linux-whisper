use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::format::FormatOptions;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors that can occur when serializing or deserializing configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Serialization error: {0}")]
    SerializeError(String),

    #[error("Deserialization error: {0}")]
    DeserializeError(String),
}

// ---------------------------------------------------------------------------
// Theme
// ---------------------------------------------------------------------------

/// Visual theme for the application UI.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

// ---------------------------------------------------------------------------
// AppConfig
// ---------------------------------------------------------------------------

/// Top-level application configuration.
///
/// Missing fields in a TOML file are filled in with sensible defaults thanks
/// to `#[serde(default)]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AppConfig {
    /// Global hotkey used to trigger recording (e.g. "Super+Shift+Space").
    pub hotkey: String,

    /// Whether to automatically paste transcribed text into the focused window.
    pub auto_paste: bool,

    /// Language code passed to the Whisper model (e.g. "en", "de", "auto").
    pub language: String,

    /// Whisper model size to use (e.g. "tiny", "base", "small", "medium", "large").
    pub model: String,

    /// Visual theme for the application.
    pub theme: Theme,

    /// Whether closing the main window minimizes to the system tray instead of
    /// quitting.
    pub minimize_to_tray: bool,

    /// Whether to display per-segment confidence scores after transcription.
    pub show_confidence: bool,

    /// Text formatting options for transcribed output.
    pub format: FormatOptions,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            hotkey: "Super+Shift+Space".to_string(),
            auto_paste: true,
            language: "auto".to_string(),
            model: "base".to_string(),
            theme: Theme::default(),
            minimize_to_tray: true,
            show_confidence: false,
            format: FormatOptions::default(),
        }
    }
}

impl AppConfig {
    // -- Load / Save --------------------------------------------------------

    /// Load configuration from the TOML file at [`config_path()`].
    ///
    /// If the file does not exist or cannot be read, returns sensible defaults.
    pub fn load() -> Self {
        let path = Self::config_path();
        match std::fs::read_to_string(&path) {
            Ok(contents) => match Self::from_toml(&contents) {
                Ok(cfg) => {
                    tracing::info!("Loaded config from {}", path.display());
                    cfg
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to parse config at {}: {e}; using defaults",
                        path.display()
                    );
                    Self::default()
                }
            },
            Err(_) => {
                tracing::info!("No config file at {}; using defaults", path.display());
                Self::default()
            }
        }
    }

    /// Save this configuration to the TOML file at [`config_path()`].
    ///
    /// Creates the parent directory if it does not exist.
    pub fn save(&self) -> Result<(), ConfigError> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ConfigError::SerializeError(format!(
                    "failed to create config dir {}: {e}",
                    parent.display()
                ))
            })?;
        }
        let toml_str = self.to_toml()?;
        std::fs::write(&path, &toml_str).map_err(|e| {
            ConfigError::SerializeError(format!(
                "failed to write config to {}: {e}",
                path.display()
            ))
        })?;
        tracing::info!("Saved config to {}", path.display());
        Ok(())
    }

    // -- XDG-compliant directory helpers -------------------------------------

    /// Returns the configuration directory for the application.
    ///
    /// Resolves to `$XDG_CONFIG_HOME/linux-whisper` when the environment
    /// variable is set, otherwise falls back to `~/.config/linux-whisper`.
    pub fn config_dir() -> PathBuf {
        let base = std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let mut home = dirs_fallback_home();
                home.push(".config");
                home
            });
        base.join("linux-whisper")
    }

    /// Returns the data directory for the application.
    ///
    /// Resolves to `$XDG_DATA_HOME/linux-whisper` when the environment
    /// variable is set, otherwise falls back to `~/.local/share/linux-whisper`.
    pub fn data_dir() -> PathBuf {
        let base = std::env::var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let mut home = dirs_fallback_home();
                home.push(".local");
                home.push("share");
                home
            });
        base.join("linux-whisper")
    }

    /// Returns the directory where downloaded Whisper models are stored.
    ///
    /// This is `data_dir()/models`.
    pub fn models_dir() -> PathBuf {
        Self::data_dir().join("models")
    }

    /// Returns the directory where downloaded LLM models are stored.
    ///
    /// This is `data_dir()/llm-models`.
    pub fn llm_models_dir() -> PathBuf {
        Self::data_dir().join("llm-models")
    }

    /// Returns the path to the main configuration file (`config.toml`).
    pub fn config_path() -> PathBuf {
        Self::config_dir().join("config.toml")
    }

    // -- TOML serialization -------------------------------------------------

    /// Serialize this configuration to a TOML string.
    pub fn to_toml(&self) -> Result<String, ConfigError> {
        toml::to_string_pretty(self).map_err(|e| ConfigError::SerializeError(e.to_string()))
    }

    /// Deserialize an `AppConfig` from a TOML string.
    ///
    /// Fields missing from the input will be filled with their default values.
    pub fn from_toml(s: &str) -> Result<Self, ConfigError> {
        toml::from_str(s).map_err(|e| ConfigError::DeserializeError(e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Helpers (private)
// ---------------------------------------------------------------------------

/// Return the user's home directory via the `$HOME` environment variable.
fn dirs_fallback_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Tests that mutate XDG env vars must hold this lock to avoid races.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn default_config_has_expected_values() {
        let cfg = AppConfig::default();

        assert_eq!(cfg.hotkey, "Super+Shift+Space");
        assert!(cfg.auto_paste);
        assert_eq!(cfg.language, "auto");
        assert_eq!(cfg.model, "base");
        assert_eq!(cfg.theme, Theme::System);
        assert!(cfg.minimize_to_tray);
        assert!(!cfg.show_confidence);
    }

    #[test]
    fn to_toml_from_toml_round_trip() {
        let original = AppConfig {
            hotkey: "Ctrl+Alt+R".to_string(),
            auto_paste: false,
            language: "de".to_string(),
            model: "large".to_string(),
            theme: Theme::Dark,
            minimize_to_tray: false,
            show_confidence: true,
            format: FormatOptions {
                enabled: false,
                llm_enabled: true,
            },
        };

        let toml_str = original.to_toml().expect("serialization should succeed");
        let restored = AppConfig::from_toml(&toml_str).expect("deserialization should succeed");

        assert_eq!(original, restored);
    }

    #[test]
    fn from_toml_partial_config_uses_defaults() {
        // Only specify two fields — the rest should take their defaults.
        let partial = r#"
            language = "fr"
            show_confidence = true
        "#;

        let cfg = AppConfig::from_toml(partial).expect("partial config should parse");

        assert_eq!(cfg.language, "fr");
        assert!(cfg.show_confidence);

        // Everything else should match Default.
        let defaults = AppConfig::default();
        assert_eq!(cfg.hotkey, defaults.hotkey);
        assert_eq!(cfg.auto_paste, defaults.auto_paste);
        assert_eq!(cfg.model, defaults.model);
        assert_eq!(cfg.theme, defaults.theme);
        assert_eq!(cfg.minimize_to_tray, defaults.minimize_to_tray);
    }

    #[test]
    fn from_toml_empty_string_uses_all_defaults() {
        let cfg = AppConfig::from_toml("").expect("empty string should parse");
        assert_eq!(cfg, AppConfig::default());
    }

    #[test]
    fn from_toml_invalid_toml_returns_error() {
        let bad = "this is [[[not valid toml";
        let result = AppConfig::from_toml(bad);
        assert!(result.is_err());

        match result.unwrap_err() {
            ConfigError::DeserializeError(msg) => {
                assert!(!msg.is_empty(), "error message should be non-empty");
            }
            other => panic!("expected DeserializeError, got: {other:?}"),
        }
    }

    #[test]
    fn config_dir_returns_reasonable_path() {
        let _lock = ENV_LOCK.lock().unwrap();
        let dir = AppConfig::config_dir();
        let dir_str = dir.to_string_lossy();

        // Must end with the application directory name.
        assert!(
            dir_str.ends_with("linux-whisper"),
            "config_dir should end with 'linux-whisper', got: {dir_str}"
        );
    }

    #[test]
    fn data_dir_returns_reasonable_path() {
        let _lock = ENV_LOCK.lock().unwrap();
        let dir = AppConfig::data_dir();
        let dir_str = dir.to_string_lossy();

        assert!(
            dir_str.ends_with("linux-whisper"),
            "data_dir should end with 'linux-whisper', got: {dir_str}"
        );
    }

    #[test]
    fn models_dir_is_inside_data_dir() {
        let _lock = ENV_LOCK.lock().unwrap();
        let models = AppConfig::models_dir();
        let data = AppConfig::data_dir();

        assert!(
            models.starts_with(&data),
            "models_dir ({models:?}) should be inside data_dir ({data:?})"
        );
        assert!(
            models.ends_with("models"),
            "models_dir should end with 'models'"
        );
    }

    #[test]
    fn config_path_is_inside_config_dir() {
        let _lock = ENV_LOCK.lock().unwrap();
        let path = AppConfig::config_path();
        let dir = AppConfig::config_dir();

        assert!(
            path.starts_with(&dir),
            "config_path ({path:?}) should be inside config_dir ({dir:?})"
        );
        assert_eq!(
            path.file_name().and_then(|f| f.to_str()),
            Some("config.toml")
        );
    }

    #[test]
    fn config_dir_respects_xdg_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        let key = "XDG_CONFIG_HOME";
        let prev = std::env::var(key).ok();

        std::env::set_var(key, "/tmp/xdg_test_config");
        let dir = AppConfig::config_dir();
        assert_eq!(dir, PathBuf::from("/tmp/xdg_test_config/linux-whisper"));

        // Restore.
        match prev {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }

    #[test]
    fn data_dir_respects_xdg_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        let key = "XDG_DATA_HOME";
        let prev = std::env::var(key).ok();

        std::env::set_var(key, "/tmp/xdg_test_data");
        let dir = AppConfig::data_dir();
        assert_eq!(dir, PathBuf::from("/tmp/xdg_test_data/linux-whisper"));

        match prev {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }

    #[test]
    fn theme_default_is_system() {
        assert_eq!(Theme::default(), Theme::System);
    }

    #[test]
    fn theme_serialization_round_trip() {
        for theme in [Theme::System, Theme::Light, Theme::Dark] {
            let cfg = AppConfig {
                theme,
                ..AppConfig::default()
            };
            let toml_str = cfg.to_toml().expect("serialize");
            let restored = AppConfig::from_toml(&toml_str).expect("deserialize");
            assert_eq!(restored.theme, theme, "round-trip failed for {theme:?}");
        }
    }

    #[test]
    fn load_returns_defaults_when_no_file() {
        let _lock = ENV_LOCK.lock().unwrap();
        let key = "XDG_CONFIG_HOME";
        let prev = std::env::var(key).ok();

        let tmp = std::env::temp_dir().join("lw_test_load_nofile");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var(key, &tmp);

        let cfg = AppConfig::load();
        assert_eq!(cfg, AppConfig::default());

        // Restore.
        match prev {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }

    #[test]
    fn save_and_load_round_trip() {
        let _lock = ENV_LOCK.lock().unwrap();
        let key = "XDG_CONFIG_HOME";
        let prev = std::env::var(key).ok();

        let tmp = std::env::temp_dir().join("lw_test_save_load");
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var(key, &tmp);

        let cfg = AppConfig {
            model: "small".to_string(),
            language: "de".to_string(),
            ..AppConfig::default()
        };

        cfg.save().expect("save should succeed");
        let loaded = AppConfig::load();
        assert_eq!(loaded.model, "small");
        assert_eq!(loaded.language, "de");

        // Cleanup.
        let _ = std::fs::remove_dir_all(&tmp);
        match prev {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }

    #[test]
    fn theme_serializes_to_lowercase() {
        let cfg = AppConfig {
            theme: Theme::Dark,
            ..AppConfig::default()
        };
        let toml_str = cfg.to_toml().expect("serialize");
        assert!(
            toml_str.contains("\"dark\""),
            "expected lowercase 'dark' in TOML output, got:\n{toml_str}"
        );
    }

    #[test]
    fn default_config_has_format_enabled() {
        let cfg = AppConfig::default();
        assert!(cfg.format.enabled);
        assert!(!cfg.format.llm_enabled);
    }

    #[test]
    fn llm_models_dir_is_inside_data_dir() {
        let _lock = ENV_LOCK.lock().unwrap();
        let llm = AppConfig::llm_models_dir();
        let data = AppConfig::data_dir();
        assert!(llm.starts_with(&data));
        assert!(llm.ends_with("llm-models"));
    }
}
