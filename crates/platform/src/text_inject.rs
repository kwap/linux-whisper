use std::process::{Command, Stdio};

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

/// Text injector using `ydotool` (display-server-agnostic, works on both
/// X11 and Wayland via the ydotoold daemon and /dev/uinput).
pub struct YdotoolInjector {
    available: bool,
}

impl YdotoolInjector {
    pub fn new() -> Self {
        let available = which("ydotool");
        Self { available }
    }
}

impl TextInjector for YdotoolInjector {
    fn inject_text(&self, text: &str) -> Result<(), InjectError> {
        if !self.available {
            return Err(InjectError::ToolNotFound(
                "ydotool is not installed".to_string(),
            ));
        }

        // ydotool type reads from stdin via --file -
        // --key-delay 12 (default) avoids dropped keystrokes
        let mut child = Command::new("ydotool")
            .args(["type", "--file", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| InjectError::InjectFailed(format!("failed to spawn ydotool: {e}")))?;

        if let Some(ref mut stdin) = child.stdin {
            use std::io::Write;
            stdin
                .write_all(text.as_bytes())
                .map_err(|e| InjectError::InjectFailed(format!("failed to write to ydotool: {e}")))?;
        }
        // Close stdin so ydotool knows input is done.
        drop(child.stdin.take());

        let output = child
            .wait_with_output()
            .map_err(|e| InjectError::InjectFailed(format!("ydotool wait error: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(InjectError::InjectFailed(format!(
                "ydotool exited with {}: {}",
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
///
/// On Wayland: tries wtype, then ydotool.
/// On X11: tries xdotool.
/// On Unknown: tries ydotool, then xdotool.
pub fn create_injector(display: &DisplayServer) -> Box<dyn TextInjector + Send> {
    match display {
        DisplayServer::Wayland => {
            let wtype = WtypeInjector::new();
            if wtype.is_available() {
                return Box::new(wtype);
            }
            let ydotool = YdotoolInjector::new();
            if ydotool.is_available() {
                tracing::info!("wtype not found; using ydotool for Wayland text injection");
                return Box::new(ydotool);
            }
            tracing::warn!("no Wayland text injection tool found (tried wtype, ydotool)");
            Box::new(wtype) // return unavailable wtype so caller gets a clear error
        }
        DisplayServer::X11 => Box::new(XdotoolInjector::new()),
        DisplayServer::Unknown => {
            let ydotool = YdotoolInjector::new();
            if ydotool.is_available() {
                tracing::info!("unknown display server; using ydotool");
                return Box::new(ydotool);
            }
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

    #[test]
    fn ydotool_injector_not_available_returns_error() {
        let injector = YdotoolInjector { available: false };
        assert!(!injector.is_available());
        let err = injector.inject_text("test").unwrap_err();
        assert!(matches!(err, InjectError::ToolNotFound(_)));
    }
}
