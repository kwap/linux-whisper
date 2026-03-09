use std::fs;
use std::process::Command;

/// Events emitted by the hotkey subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    Pressed,
    Released,
}

/// Errors that can occur during hotkey management.
#[derive(Debug, thiserror::Error)]
pub enum HotkeyError {
    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("bind error: {0}")]
    BindError(String),

    #[error("unsupported platform")]
    UnsupportedPlatform,
}

/// Trait for managing global hotkey bindings.
///
/// Implementations should use a channel or other mechanism to deliver
/// [`HotkeyEvent`]s to the caller. The `set_event_sender` method wires up
/// a `tokio::sync::mpsc::Sender` for that purpose.
#[cfg_attr(test, mockall::automock)]
pub trait HotkeyManager: Send {
    /// Bind a hotkey described by the given string (e.g. "ctrl+shift+space").
    fn bind(&mut self, hotkey: &str) -> Result<(), HotkeyError>;

    /// Unbind the currently bound hotkey.
    fn unbind(&mut self) -> Result<(), HotkeyError>;

    /// Returns `true` if a hotkey is currently bound.
    fn is_bound(&self) -> bool;

    /// Set the channel sender used to deliver hotkey events.
    fn set_event_sender(&mut self, sender: tokio::sync::mpsc::Sender<HotkeyEvent>);
}

/// Check whether the current user belongs to the `input` group.
///
/// This is required on most Linux distributions so that the application can
/// read from `/dev/input/*` devices for global hotkey support.
pub fn check_input_group_membership() -> bool {
    // First, try reading /etc/group directly.
    if let Ok(contents) = fs::read_to_string("/etc/group") {
        let username = whoami();
        for line in contents.lines() {
            // Format: group_name:password:GID:user_list
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() >= 4 && parts[0] == "input" {
                let members: Vec<&str> = parts[3].split(',').collect();
                if members.iter().any(|m| *m == username) {
                    return true;
                }
            }
        }
        return false;
    }

    // Fallback: use the `groups` command.
    if let Ok(output) = Command::new("groups").output() {
        if let Ok(groups_str) = String::from_utf8(output.stdout) {
            return groups_str.split_whitespace().any(|g| g == "input");
        }
    }

    false
}

/// Get the current username.
fn whoami() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_hotkey_manager_bind_and_unbind() {
        let mut mock = MockHotkeyManager::new();

        mock.expect_bind()
            .withf(|key: &str| key == "ctrl+shift+space")
            .times(1)
            .returning(|_| Ok(()));

        mock.expect_is_bound()
            .times(1)
            .returning(|| true);

        mock.expect_unbind()
            .times(1)
            .returning(|| Ok(()));

        assert!(mock.bind("ctrl+shift+space").is_ok());
        assert!(mock.is_bound());
        assert!(mock.unbind().is_ok());
    }

    #[test]
    fn mock_hotkey_manager_bind_error() {
        let mut mock = MockHotkeyManager::new();

        mock.expect_bind()
            .times(1)
            .returning(|key| {
                Err(HotkeyError::BindError(format!("unknown key: {key}")))
            });

        let err = mock.bind("invalid_key").unwrap_err();
        assert!(matches!(err, HotkeyError::BindError(_)));
    }

    #[test]
    fn mock_hotkey_manager_permission_denied() {
        let mut mock = MockHotkeyManager::new();

        mock.expect_bind()
            .times(1)
            .returning(|_| {
                Err(HotkeyError::PermissionDenied(
                    "user not in input group".to_string(),
                ))
            });

        let err = mock.bind("ctrl+space").unwrap_err();
        assert!(matches!(err, HotkeyError::PermissionDenied(_)));
    }

    #[test]
    fn mock_hotkey_manager_unsupported_platform() {
        let mut mock = MockHotkeyManager::new();

        mock.expect_bind()
            .times(1)
            .returning(|_| Err(HotkeyError::UnsupportedPlatform));

        let err = mock.bind("ctrl+space").unwrap_err();
        assert!(matches!(err, HotkeyError::UnsupportedPlatform));
    }

    #[test]
    fn mock_hotkey_manager_event_sender() {
        let mut mock = MockHotkeyManager::new();

        mock.expect_set_event_sender()
            .times(1)
            .return_const(());

        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        mock.set_event_sender(tx);
    }

    #[test]
    fn hotkey_event_debug() {
        assert_eq!(format!("{:?}", HotkeyEvent::Pressed), "Pressed");
        assert_eq!(format!("{:?}", HotkeyEvent::Released), "Released");
    }

    #[test]
    fn hotkey_error_display() {
        let err = HotkeyError::PermissionDenied("not in input group".into());
        assert_eq!(err.to_string(), "permission denied: not in input group");

        let err = HotkeyError::BindError("key conflict".into());
        assert_eq!(err.to_string(), "bind error: key conflict");

        let err = HotkeyError::UnsupportedPlatform;
        assert_eq!(err.to_string(), "unsupported platform");
    }

    #[test]
    fn check_input_group_membership_runs_without_panic() {
        // We cannot guarantee the result, but it must not panic.
        let _result = check_input_group_membership();
    }
}
