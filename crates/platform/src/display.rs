use std::env;
use std::fmt;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;
use std::path::PathBuf;
use std::process::Command;

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

/// Detect the running display server by inspecting environment variables,
/// then falling back to runtime socket scanning and loginctl.
///
/// Detection order:
/// 1. `WAYLAND_DISPLAY` is set -> Wayland
/// 2. `XDG_SESSION_TYPE` is "wayland" -> Wayland; "x11" -> X11
/// 3. `DISPLAY` is set -> X11
/// 4. Scan `XDG_RUNTIME_DIR` for `wayland-*` socket files -> Wayland
///    (also sets `WAYLAND_DISPLAY` so child processes inherit it)
/// 5. `loginctl show-session self -p Type --value` -> "wayland" / "x11"
/// 6. Otherwise -> Unknown
pub fn detect() -> DisplayServer {
    let result = detect_with_env(
        env::var("WAYLAND_DISPLAY").ok(),
        env::var("XDG_SESSION_TYPE").ok(),
        env::var("DISPLAY").ok(),
    );

    if result != DisplayServer::Unknown {
        return result;
    }

    // Fallback: scan XDG_RUNTIME_DIR for wayland-* sockets.
    if let Some((server, socket_name)) = scan_wayland_socket() {
        // Set WAYLAND_DISPLAY so child processes (wl-copy, wtype, etc.) work.
        env::set_var("WAYLAND_DISPLAY", &socket_name);
        tracing::info!(
            "Detected {server} via socket scan; set WAYLAND_DISPLAY={socket_name}"
        );
        return server;
    }

    // Fallback: ask loginctl.
    if let Some(server) = detect_via_loginctl() {
        tracing::info!("Detected {server} via loginctl");
        return server;
    }

    DisplayServer::Unknown
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

/// Scan the XDG_RUNTIME_DIR for `wayland-*` socket files.
///
/// Returns `Some((DisplayServer::Wayland, socket_name))` if found.
fn scan_wayland_socket() -> Option<(DisplayServer, String)> {
    let runtime_dir = runtime_dir()?;

    let entries = fs::read_dir(&runtime_dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        // Look for wayland-N sockets (not .lock files).
        if name_str.starts_with("wayland-") && !name_str.ends_with(".lock") {
            // Verify it's actually a socket.
            let file_type = entry.file_type().ok()?;
            if file_type.is_socket() || file_type.is_file() {
                return Some((DisplayServer::Wayland, name_str.into_owned()));
            }
        }
    }
    None
}

/// Get the XDG runtime directory, falling back to `/run/user/<uid>`.
fn runtime_dir() -> Option<PathBuf> {
    if let Ok(dir) = env::var("XDG_RUNTIME_DIR") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }

    // Fall back to /run/user/<uid>.
    let output = Command::new("id").arg("-u").output().ok()?;
    let uid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if uid.is_empty() {
        return None;
    }
    let path = PathBuf::from(format!("/run/user/{uid}"));
    if path.is_dir() {
        Some(path)
    } else {
        None
    }
}

/// Ask loginctl for the session type.
fn detect_via_loginctl() -> Option<DisplayServer> {
    let output = Command::new("loginctl")
        .args(["show-session", "self", "-p", "Type", "--value"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let session_type = String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_lowercase();

    match session_type.as_str() {
        "wayland" => Some(DisplayServer::Wayland),
        "x11" => Some(DisplayServer::X11),
        _ => None,
    }
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

    #[test]
    fn runtime_dir_returns_some_on_linux() {
        // On a typical Linux system XDG_RUNTIME_DIR is set.
        // We just verify it doesn't panic.
        let _ = runtime_dir();
    }

    #[test]
    fn detect_via_loginctl_does_not_panic() {
        // loginctl may or may not be available — just verify no panic.
        let _ = detect_via_loginctl();
    }

    #[test]
    fn scan_wayland_socket_does_not_panic() {
        let _ = scan_wayland_socket();
    }
}
