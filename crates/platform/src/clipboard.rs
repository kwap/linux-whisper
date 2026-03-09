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
}
