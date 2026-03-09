use std::env;
use std::fmt;

/// Detected display server protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayServer {
    X11,
    Wayland,
    Unknown,
}

impl fmt::Display for DisplayServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DisplayServer::X11 => write!(f, "X11"),
            DisplayServer::Wayland => write!(f, "Wayland"),
            DisplayServer::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Detect the running display server by inspecting environment variables.
///
/// Detection order:
/// 1. `WAYLAND_DISPLAY` is set -> Wayland
/// 2. `XDG_SESSION_TYPE` is "wayland" -> Wayland; "x11" -> X11
/// 3. `DISPLAY` is set -> X11
/// 4. Otherwise -> Unknown
pub fn detect() -> DisplayServer {
    detect_with_env(
        env::var("WAYLAND_DISPLAY").ok(),
        env::var("XDG_SESSION_TYPE").ok(),
        env::var("DISPLAY").ok(),
    )
}

/// Internal helper that accepts pre-read env values for testability.
fn detect_with_env(
    wayland_display: Option<String>,
    xdg_session_type: Option<String>,
    display: Option<String>,
) -> DisplayServer {
    // 1. WAYLAND_DISPLAY being set is a strong indicator of Wayland.
    if wayland_display.as_deref().is_some_and(|v| !v.is_empty()) {
        return DisplayServer::Wayland;
    }

    // 2. XDG_SESSION_TYPE is the canonical way to query the session type.
    if let Some(session_type) = xdg_session_type.as_deref() {
        match session_type {
            "wayland" => return DisplayServer::Wayland,
            "x11" => return DisplayServer::X11,
            _ => {}
        }
    }

    // 3. DISPLAY being set is a reasonable X11 indicator.
    if display.as_deref().is_some_and(|v| !v.is_empty()) {
        return DisplayServer::X11;
    }

    DisplayServer::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wayland_display_set_returns_wayland() {
        let result = detect_with_env(
            Some("wayland-0".to_string()),
            None,
            None,
        );
        assert_eq!(result, DisplayServer::Wayland);
    }

    #[test]
    fn wayland_display_empty_falls_through() {
        let result = detect_with_env(
            Some(String::new()),
            None,
            None,
        );
        assert_eq!(result, DisplayServer::Unknown);
    }

    #[test]
    fn xdg_session_type_wayland() {
        let result = detect_with_env(
            None,
            Some("wayland".to_string()),
            None,
        );
        assert_eq!(result, DisplayServer::Wayland);
    }

    #[test]
    fn xdg_session_type_x11() {
        let result = detect_with_env(
            None,
            Some("x11".to_string()),
            None,
        );
        assert_eq!(result, DisplayServer::X11);
    }

    #[test]
    fn xdg_session_type_unknown_value() {
        let result = detect_with_env(
            None,
            Some("tty".to_string()),
            None,
        );
        assert_eq!(result, DisplayServer::Unknown);
    }

    #[test]
    fn display_set_returns_x11() {
        let result = detect_with_env(
            None,
            None,
            Some(":0".to_string()),
        );
        assert_eq!(result, DisplayServer::X11);
    }

    #[test]
    fn display_empty_returns_unknown() {
        let result = detect_with_env(
            None,
            None,
            Some(String::new()),
        );
        assert_eq!(result, DisplayServer::Unknown);
    }

    #[test]
    fn nothing_set_returns_unknown() {
        let result = detect_with_env(None, None, None);
        assert_eq!(result, DisplayServer::Unknown);
    }

    #[test]
    fn wayland_display_takes_priority_over_xdg() {
        let result = detect_with_env(
            Some("wayland-0".to_string()),
            Some("x11".to_string()),
            Some(":0".to_string()),
        );
        assert_eq!(result, DisplayServer::Wayland);
    }

    #[test]
    fn xdg_x11_takes_priority_over_display() {
        let result = detect_with_env(
            None,
            Some("x11".to_string()),
            Some(":0".to_string()),
        );
        assert_eq!(result, DisplayServer::X11);
    }

    #[test]
    fn display_format() {
        assert_eq!(format!("{}", DisplayServer::X11), "X11");
        assert_eq!(format!("{}", DisplayServer::Wayland), "Wayland");
        assert_eq!(format!("{}", DisplayServer::Unknown), "Unknown");
    }
}
