use std::io::Write;
use std::process::{Command, Stdio};

use crate::display::DisplayServer;

/// Errors that can occur during clipboard operations.
#[derive(Debug, thiserror::Error)]
pub enum ClipboardError {
    #[error("clipboard access error: {0}")]
    AccessError(String),

    #[error("clipboard set error: {0}")]
    SetError(String),
}

/// Trait for clipboard read/write operations.
#[cfg_attr(test, mockall::automock)]
pub trait ClipboardManager: Send {
    /// Read the current text content from the clipboard.
    fn get_text(&self) -> Result<String, ClipboardError>;

    /// Write text content to the clipboard.
    fn set_text(&self, text: &str) -> Result<(), ClipboardError>;
}

/// Clipboard manager backed by the `arboard` crate.
///
/// Each operation opens a fresh clipboard handle, which is cheap on Linux and
/// avoids lifetime issues with the underlying platform handle.
pub struct ArboardClipboard {
    _private: (),
}

impl ArboardClipboard {
    /// Create a new `ArboardClipboard` instance.
    ///
    /// This will fail if the system clipboard cannot be accessed (e.g. no
    /// display server is running).
    pub fn new() -> Result<Self, ClipboardError> {
        // Verify that we can open a clipboard handle at construction time.
        let _clipboard = arboard::Clipboard::new().map_err(|e| {
            ClipboardError::AccessError(format!("failed to open clipboard: {e}"))
        })?;
        Ok(Self { _private: () })
    }
}

impl ClipboardManager for ArboardClipboard {
    fn get_text(&self) -> Result<String, ClipboardError> {
        // arboard requires &mut self for get_text, so we use an interior
        // mutability workaround via unsafe pointer cast. However, the simpler
        // approach is to just create a new clipboard handle each time. This is
        // cheap on Linux.
        let mut cb = arboard::Clipboard::new().map_err(|e| {
            ClipboardError::AccessError(format!("failed to open clipboard: {e}"))
        })?;
        cb.get_text().map_err(|e| {
            ClipboardError::AccessError(format!("failed to read clipboard: {e}"))
        })
    }

    fn set_text(&self, text: &str) -> Result<(), ClipboardError> {
        let mut cb = arboard::Clipboard::new().map_err(|e| {
            ClipboardError::AccessError(format!("failed to open clipboard: {e}"))
        })?;
        cb.set_text(text.to_string()).map_err(|e| {
            ClipboardError::SetError(format!("failed to write clipboard: {e}"))
        })
    }
}

/// Clipboard manager using `wl-copy` / `wl-paste` from the `wl-clipboard`
/// package. This avoids the Wayland issue where arboard's clipboard content
/// disappears as soon as the setting process exits or drops the handle.
pub struct WlClipboard {
    _private: (),
}

impl WlClipboard {
    /// Create a new `WlClipboard`. Returns `Err` if `wl-copy` is not on `$PATH`.
    pub fn new() -> Result<Self, ClipboardError> {
        let available = Command::new("which")
            .arg("wl-copy")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if !available {
            return Err(ClipboardError::AccessError(
                "wl-copy not found — install wl-clipboard".to_string(),
            ));
        }
        Ok(Self { _private: () })
    }
}

impl ClipboardManager for WlClipboard {
    fn get_text(&self) -> Result<String, ClipboardError> {
        let output = Command::new("wl-paste")
            .arg("--no-newline")
            .output()
            .map_err(|e| ClipboardError::AccessError(format!("failed to run wl-paste: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ClipboardError::AccessError(format!(
                "wl-paste failed: {stderr}"
            )));
        }

        String::from_utf8(output.stdout)
            .map_err(|e| ClipboardError::AccessError(format!("invalid UTF-8 from wl-paste: {e}")))
    }

    fn set_text(&self, text: &str) -> Result<(), ClipboardError> {
        let mut child = Command::new("wl-copy")
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|e| ClipboardError::SetError(format!("failed to spawn wl-copy: {e}")))?;

        if let Some(ref mut stdin) = child.stdin {
            stdin
                .write_all(text.as_bytes())
                .map_err(|e| ClipboardError::SetError(format!("failed to write to wl-copy: {e}")))?;
        }

        let status = child
            .wait()
            .map_err(|e| ClipboardError::SetError(format!("wl-copy wait error: {e}")))?;

        if !status.success() {
            return Err(ClipboardError::SetError(format!(
                "wl-copy exited with {status}"
            )));
        }

        Ok(())
    }
}

/// Create the best available clipboard backend for the given display server.
///
/// On Wayland: prefers `WlClipboard` (wl-copy/wl-paste), falls back to arboard.
/// On X11 / Unknown: uses arboard.
pub fn create_clipboard(display: &DisplayServer) -> Box<dyn ClipboardManager + Send> {
    match display {
        DisplayServer::Wayland => {
            if let Ok(wl) = WlClipboard::new() {
                tracing::info!("Using wl-clipboard for Wayland clipboard");
                return Box::new(wl);
            }
            tracing::warn!(
                "wl-clipboard not available; falling back to arboard (clipboard may not persist)"
            );
            match ArboardClipboard::new() {
                Ok(ab) => Box::new(ab),
                Err(e) => {
                    tracing::error!("arboard fallback also failed: {e}");
                    // Return a WlClipboard that will fail on use — better error message.
                    Box::new(WlClipboard { _private: () })
                }
            }
        }
        _ => match ArboardClipboard::new() {
            Ok(ab) => Box::new(ab),
            Err(e) => {
                tracing::error!("arboard clipboard failed: {e}");
                Box::new(WlClipboard { _private: () })
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_clipboard_get_text() {
        let mut mock = MockClipboardManager::new();

        mock.expect_get_text()
            .times(1)
            .returning(|| Ok("hello from clipboard".to_string()));

        let text = mock.get_text().unwrap();
        assert_eq!(text, "hello from clipboard");
    }

    #[test]
    fn mock_clipboard_set_text() {
        let mut mock = MockClipboardManager::new();

        mock.expect_set_text()
            .withf(|text: &str| text == "new content")
            .times(1)
            .returning(|_| Ok(()));

        assert!(mock.set_text("new content").is_ok());
    }

    #[test]
    fn mock_clipboard_get_text_error() {
        let mut mock = MockClipboardManager::new();

        mock.expect_get_text()
            .times(1)
            .returning(|| Err(ClipboardError::AccessError("no display".to_string())));

        let err = mock.get_text().unwrap_err();
        assert!(matches!(err, ClipboardError::AccessError(_)));
    }

    #[test]
    fn mock_clipboard_set_text_error() {
        let mut mock = MockClipboardManager::new();

        mock.expect_set_text()
            .times(1)
            .returning(|_| Err(ClipboardError::SetError("write failed".to_string())));

        let err = mock.set_text("data").unwrap_err();
        assert!(matches!(err, ClipboardError::SetError(_)));
    }

    #[test]
    fn clipboard_error_display() {
        let err = ClipboardError::AccessError("no display".into());
        assert_eq!(err.to_string(), "clipboard access error: no display");

        let err = ClipboardError::SetError("write failed".into());
        assert_eq!(err.to_string(), "clipboard set error: write failed");
    }

    #[test]
    fn create_clipboard_does_not_panic() {
        // Smoke test — the factory should never panic regardless of display.
        let _ = create_clipboard(&DisplayServer::Wayland);
        let _ = create_clipboard(&DisplayServer::X11);
        let _ = create_clipboard(&DisplayServer::Unknown);
    }
}
