use std::process::Command;

use crate::display::DisplayServer;

/// Errors that can occur during text injection.
#[derive(Debug, thiserror::Error)]
pub enum InjectError {
    #[error("tool not found: {0}")]
    ToolNotFound(String),

    #[error("injection failed: {0}")]
    InjectFailed(String),
}

/// Trait for injecting text into the currently focused window.
#[cfg_attr(test, mockall::automock)]
pub trait TextInjector: Send {
    /// Type the given text into the active window.
    fn inject_text(&self, text: &str) -> Result<(), InjectError>;

    /// Check whether the required tool is available on the system.
    fn is_available(&self) -> bool;
}

/// Text injector using `xdotool` (X11).
pub struct XdotoolInjector {
    available: bool,
}

impl XdotoolInjector {
    pub fn new() -> Self {
        let available = which("xdotool");
        Self { available }
    }
}

impl TextInjector for XdotoolInjector {
    fn inject_text(&self, text: &str) -> Result<(), InjectError> {
        if !self.available {
            return Err(InjectError::ToolNotFound(
                "xdotool is not installed".to_string(),
            ));
        }

        let output = Command::new("xdotool")
            .arg("type")
            .arg("--clearmodifiers")
            .arg("--")
            .arg(text)
            .output()
            .map_err(|e| InjectError::InjectFailed(format!("failed to run xdotool: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(InjectError::InjectFailed(format!(
                "xdotool exited with {}: {}",
                output.status, stderr
            )));
        }

        Ok(())
    }

    fn is_available(&self) -> bool {
        self.available
    }
}

/// Text injector using `wtype` (Wayland).
pub struct WtypeInjector {
    available: bool,
}

impl WtypeInjector {
    pub fn new() -> Self {
        let available = which("wtype");
        Self { available }
    }
}

impl TextInjector for WtypeInjector {
    fn inject_text(&self, text: &str) -> Result<(), InjectError> {
        if !self.available {
            return Err(InjectError::ToolNotFound(
                "wtype is not installed".to_string(),
            ));
        }

        let output = Command::new("wtype")
            .arg("--")
            .arg(text)
            .output()
            .map_err(|e| InjectError::InjectFailed(format!("failed to run wtype: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(InjectError::InjectFailed(format!(
                "wtype exited with {}: {}",
                output.status, stderr
            )));
        }

        Ok(())
    }

    fn is_available(&self) -> bool {
        self.available
    }
}

/// Check if a binary is available on `$PATH`.
fn which(binary: &str) -> bool {
    Command::new("which")
        .arg(binary)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Create the appropriate text injector for the detected display server.
pub fn create_injector(display: &DisplayServer) -> Box<dyn TextInjector + Send> {
    match display {
        DisplayServer::Wayland => Box::new(WtypeInjector::new()),
        DisplayServer::X11 => Box::new(XdotoolInjector::new()),
        DisplayServer::Unknown => {
            // Fall back to xdotool — it is the more common tool.
            tracing::warn!("unknown display server; falling back to xdotool");
            Box::new(XdotoolInjector::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_injector_inject_text() {
        let mut mock = MockTextInjector::new();

        mock.expect_inject_text()
            .withf(|text: &str| text == "hello world")
            .times(1)
            .returning(|_| Ok(()));

        assert!(mock.inject_text("hello world").is_ok());
    }

    #[test]
    fn mock_injector_tool_not_found() {
        let mut mock = MockTextInjector::new();

        mock.expect_inject_text()
            .times(1)
            .returning(|_| {
                Err(InjectError::ToolNotFound("xdotool not found".to_string()))
            });

        let err = mock.inject_text("text").unwrap_err();
        assert!(matches!(err, InjectError::ToolNotFound(_)));
    }

    #[test]
    fn mock_injector_inject_failed() {
        let mut mock = MockTextInjector::new();

        mock.expect_inject_text()
            .times(1)
            .returning(|_| {
                Err(InjectError::InjectFailed("process crashed".to_string()))
            });

        let err = mock.inject_text("text").unwrap_err();
        assert!(matches!(err, InjectError::InjectFailed(_)));
    }

    #[test]
    fn mock_injector_is_available() {
        let mut mock = MockTextInjector::new();

        mock.expect_is_available().times(1).returning(|| true);

        assert!(mock.is_available());
    }

    #[test]
    fn inject_error_display() {
        let err = InjectError::ToolNotFound("xdotool".into());
        assert_eq!(err.to_string(), "tool not found: xdotool");

        let err = InjectError::InjectFailed("exit code 1".into());
        assert_eq!(err.to_string(), "injection failed: exit code 1");
    }

    #[test]
    fn create_injector_x11() {
        let injector = create_injector(&DisplayServer::X11);
        // We can only verify it was created; actual availability depends on
        // whether xdotool is installed in the test environment.
        let _ = injector.is_available();
    }

    #[test]
    fn create_injector_wayland() {
        let injector = create_injector(&DisplayServer::Wayland);
        let _ = injector.is_available();
    }

    #[test]
    fn create_injector_unknown_falls_back() {
        let injector = create_injector(&DisplayServer::Unknown);
        let _ = injector.is_available();
    }

    #[test]
    fn xdotool_injector_not_available_returns_error() {
        // Force unavailable by constructing directly.
        let injector = XdotoolInjector { available: false };
        assert!(!injector.is_available());
        let err = injector.inject_text("test").unwrap_err();
        assert!(matches!(err, InjectError::ToolNotFound(_)));
    }

    #[test]
    fn wtype_injector_not_available_returns_error() {
        let injector = WtypeInjector { available: false };
        assert!(!injector.is_available());
        let err = injector.inject_text("test").unwrap_err();
        assert!(matches!(err, InjectError::ToolNotFound(_)));
    }
}
