use std::collections::HashSet;
use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use evdev::{Device, EventType, KeyCode};
use nix::poll::{poll, PollFd, PollFlags, PollTimeout};
use tracing::{debug, error, info, warn};

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

// ---------------------------------------------------------------------------
// parse_hotkey
// ---------------------------------------------------------------------------

/// Parse a hotkey string like `"Super+Shift+Space"` into a set of evdev `Key`
/// codes. Returns both left and right variants for modifier keys.
pub fn parse_hotkey(hotkey: &str) -> Result<Vec<KeyCode>, HotkeyError> {
    let parts: Vec<&str> = hotkey.split('+').map(str::trim).collect();
    if parts.is_empty() {
        return Err(HotkeyError::BindError("empty hotkey string".into()));
    }

    let mut keys = Vec::new();
    for part in &parts {
        let mapped = map_key_name(part)?;
        keys.extend(mapped);
    }

    Ok(keys)
}

/// Map a single key name to one or more evdev Key codes.
/// Modifier keys return both left and right variants.
fn map_key_name(name: &str) -> Result<Vec<KeyCode>, HotkeyError> {
    match name.to_lowercase().as_str() {
        // Modifiers (return both L/R so either physical key matches).
        "super" | "meta" | "logo" | "win" => {
            Ok(vec![KeyCode::KEY_LEFTMETA, KeyCode::KEY_RIGHTMETA])
        }
        "shift" => Ok(vec![KeyCode::KEY_LEFTSHIFT, KeyCode::KEY_RIGHTSHIFT]),
        "ctrl" | "control" => Ok(vec![KeyCode::KEY_LEFTCTRL, KeyCode::KEY_RIGHTCTRL]),
        "alt" => Ok(vec![KeyCode::KEY_LEFTALT, KeyCode::KEY_RIGHTALT]),

        // Common keys.
        "space" => Ok(vec![KeyCode::KEY_SPACE]),
        "enter" | "return" => Ok(vec![KeyCode::KEY_ENTER]),
        "tab" => Ok(vec![KeyCode::KEY_TAB]),
        "escape" | "esc" => Ok(vec![KeyCode::KEY_ESC]),
        "backspace" => Ok(vec![KeyCode::KEY_BACKSPACE]),

        // Letters.
        s if s.len() == 1 && s.chars().next().unwrap().is_ascii_alphabetic() => {
            let ch = s.chars().next().unwrap().to_ascii_uppercase();
            let code = (ch as u16) - b'A' as u16 + KeyCode::KEY_A.code();
            Ok(vec![KeyCode::new(code)])
        }

        // Function keys (F1–F12). Evdev codes: F1–F10 are contiguous (59–68),
        // F11=87, F12=88.
        s if s.starts_with('f') || s.starts_with('F') => {
            let num: u16 = s[1..]
                .parse()
                .map_err(|_| HotkeyError::BindError(format!("unknown key: {name}")))?;
            let key = match num {
                1..=10 => KeyCode::new(KeyCode::KEY_F1.code() + num - 1),
                11 => KeyCode::KEY_F11,
                12 => KeyCode::KEY_F12,
                _ => {
                    return Err(HotkeyError::BindError(format!(
                        "function key out of range: {name}"
                    )));
                }
            };
            Ok(vec![key])
        }

        // Numbers.
        s if s.len() == 1 && s.chars().next().unwrap().is_ascii_digit() => {
            let ch = s.chars().next().unwrap();
            let code = if ch == '0' {
                KeyCode::KEY_0.code()
            } else {
                KeyCode::KEY_1.code() + (ch as u16 - b'1' as u16)
            };
            Ok(vec![KeyCode::new(code)])
        }

        _ => Err(HotkeyError::BindError(format!("unknown key: {name}"))),
    }
}

// ---------------------------------------------------------------------------
// EvdevHotkeyManager
// ---------------------------------------------------------------------------

/// Global hotkey manager using Linux evdev input devices.
///
/// Requires the current user to be in the `input` group (or have read access
/// to `/dev/input/event*` devices).
pub struct EvdevHotkeyManager {
    event_sender: Option<tokio::sync::mpsc::Sender<HotkeyEvent>>,
    shutdown: Option<Arc<AtomicBool>>,
    thread: Option<thread::JoinHandle<()>>,
    bound: bool,
}

impl EvdevHotkeyManager {
    pub fn new() -> Self {
        Self {
            event_sender: None,
            shutdown: None,
            thread: None,
            bound: false,
        }
    }
}

impl HotkeyManager for EvdevHotkeyManager {
    fn bind(&mut self, hotkey: &str) -> Result<(), HotkeyError> {
        // Check input group membership first.
        if !check_input_group_membership() {
            return Err(HotkeyError::PermissionDenied(
                "user is not in the 'input' group — run: sudo usermod -aG input $USER".into(),
            ));
        }

        let sender = self.event_sender.clone().ok_or_else(|| {
            HotkeyError::BindError("event sender not set — call set_event_sender() first".into())
        })?;

        let target_keys = parse_hotkey(hotkey)?;
        info!("Binding hotkey \"{hotkey}\" → {} evdev key(s)", target_keys.len());

        // Open all keyboard devices.
        let keyboards = open_keyboard_devices()?;
        if keyboards.is_empty() {
            return Err(HotkeyError::BindError(
                "no keyboard devices found in /dev/input/".into(),
            ));
        }
        info!("Monitoring {} keyboard device(s)", keyboards.len());

        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();

        // The target_keys list contains L/R variants for modifiers.
        // The combo is: for each part of the hotkey, at least one of its
        // variant keys must be held.
        //
        // We build "groups" — each original hotkey part maps to a set of
        // acceptable keys (e.g., Super → {LEFTMETA, RIGHTMETA}).
        let groups = build_key_groups(hotkey)?;

        let handle = thread::Builder::new()
            .name("evdev-hotkey".into())
            .spawn(move || {
                hotkey_listener_loop(keyboards, groups, sender, shutdown_clone);
            })
            .map_err(|e| HotkeyError::BindError(format!("failed to spawn listener thread: {e}")))?;

        self.shutdown = Some(shutdown);
        self.thread = Some(handle);
        self.bound = true;

        Ok(())
    }

    fn unbind(&mut self) -> Result<(), HotkeyError> {
        if let Some(shutdown) = self.shutdown.take() {
            shutdown.store(true, Ordering::Relaxed);
        }
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
        self.bound = false;
        Ok(())
    }

    fn is_bound(&self) -> bool {
        self.bound
    }

    fn set_event_sender(&mut self, sender: tokio::sync::mpsc::Sender<HotkeyEvent>) {
        self.event_sender = Some(sender);
    }
}

impl Drop for EvdevHotkeyManager {
    fn drop(&mut self) {
        let _ = self.unbind();
    }
}

// ---------------------------------------------------------------------------
// Key groups
// ---------------------------------------------------------------------------

/// A group represents one "part" of the hotkey combo (e.g., the "Shift" part).
/// Any of the keys in the group being held satisfies that part.
struct KeyGroup {
    keys: Vec<KeyCode>,
}

fn build_key_groups(hotkey: &str) -> Result<Vec<KeyGroup>, HotkeyError> {
    let parts: Vec<&str> = hotkey.split('+').map(str::trim).collect();
    let mut groups = Vec::new();
    for part in parts {
        let keys = map_key_name(part)?;
        groups.push(KeyGroup { keys });
    }
    Ok(groups)
}

/// Check if all groups are satisfied by the currently held keys.
fn combo_active(groups: &[KeyGroup], held: &HashSet<KeyCode>) -> bool {
    groups
        .iter()
        .all(|g| g.keys.iter().any(|k| held.contains(k)))
}

// ---------------------------------------------------------------------------
// Listener thread
// ---------------------------------------------------------------------------

fn hotkey_listener_loop(
    mut keyboards: Vec<Device>,
    groups: Vec<KeyGroup>,
    sender: tokio::sync::mpsc::Sender<HotkeyEvent>,
    shutdown: Arc<AtomicBool>,
) {
    use std::os::fd::AsFd;

    let mut held_keys: HashSet<KeyCode> = HashSet::new();
    let mut combo_was_active = false;

    // Set all devices to non-blocking.
    for dev in &keyboards {
        if let Err(e) = dev.set_nonblocking(true) {
            warn!("Failed to set device non-blocking: {e}");
        }
    }

    loop {
        if shutdown.load(Ordering::Relaxed) {
            debug!("Hotkey listener shutting down");
            return;
        }

        // Poll all device fds with a 200ms timeout.
        // We collect ready indices separately so the borrow on keyboards is released
        // before we call fetch_events().
        let ready_indices: Vec<usize> = {
            let mut pollfds: Vec<PollFd> = keyboards
                .iter()
                .map(|d| PollFd::new(d.as_fd(), PollFlags::POLLIN))
                .collect();

            let timeout = PollTimeout::try_from(200).unwrap();
            match poll(&mut pollfds, timeout) {
                Ok(0) => continue, // timeout
                Err(e) => {
                    if e == nix::errno::Errno::EINTR {
                        continue;
                    }
                    error!("poll() error: {e}");
                    return;
                }
                Ok(_) => {}
            }

            pollfds
                .iter()
                .enumerate()
                .filter(|(_, pfd)| {
                    pfd.revents()
                        .map_or(false, |r| r.contains(PollFlags::POLLIN))
                })
                .map(|(i, _)| i)
                .collect()
        };

        // Read events from all ready devices.
        for i in ready_indices {
            if let Ok(events) = keyboards[i].fetch_events() {
                for ev in events {
                    if ev.event_type() == EventType::KEY {
                        let key = KeyCode::new(ev.code());
                        match ev.value() {
                            1 => {
                                // Key press.
                                held_keys.insert(key);
                            }
                            0 => {
                                // Key release.
                                held_keys.remove(&key);
                            }
                            2 => {} // Repeat — ignore.
                            _ => {}
                        }
                    }
                }
            }
        }

        let combo_active_now = combo_active(&groups, &held_keys);

        if combo_active_now && !combo_was_active {
            debug!("Hotkey combo pressed");
            let _ = sender.try_send(HotkeyEvent::Pressed);
        } else if !combo_active_now && combo_was_active {
            debug!("Hotkey combo released");
            let _ = sender.try_send(HotkeyEvent::Released);
        }

        combo_was_active = combo_active_now;
    }
}

// ---------------------------------------------------------------------------
// Device discovery
// ---------------------------------------------------------------------------

/// Open all `/dev/input/event*` devices that support keyboard keys.
fn open_keyboard_devices() -> Result<Vec<Device>, HotkeyError> {
    let entries = fs::read_dir("/dev/input").map_err(|e| {
        HotkeyError::PermissionDenied(format!("cannot read /dev/input: {e}"))
    })?;

    let mut keyboards = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        if !name.starts_with("event") {
            continue;
        }

        match Device::open(&path) {
            Ok(dev) => {
                // Check if the device supports KEY events and has keyboard keys.
                if let Some(supported) = dev.supported_keys() {
                    if supported.contains(KeyCode::KEY_SPACE) {
                        debug!("Opened keyboard device: {} ({})",
                            dev.name().unwrap_or("unknown"), path.display());
                        keyboards.push(dev);
                    }
                }
            }
            Err(e) => {
                debug!("Cannot open {}: {e}", path.display());
            }
        }
    }

    Ok(keyboards)
}

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

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

    // -- parse_hotkey tests -------------------------------------------------

    #[test]
    fn parse_hotkey_super_shift_space() {
        let keys = parse_hotkey("Super+Shift+Space").unwrap();
        // Super → 2 keys, Shift → 2 keys, Space → 1 key = 5 total.
        assert_eq!(keys.len(), 5);
        assert!(keys.contains(&KeyCode::KEY_LEFTMETA));
        assert!(keys.contains(&KeyCode::KEY_RIGHTMETA));
        assert!(keys.contains(&KeyCode::KEY_LEFTSHIFT));
        assert!(keys.contains(&KeyCode::KEY_RIGHTSHIFT));
        assert!(keys.contains(&KeyCode::KEY_SPACE));
    }

    #[test]
    fn parse_hotkey_ctrl_alt_a() {
        let keys = parse_hotkey("Ctrl+Alt+A").unwrap();
        assert!(keys.contains(&KeyCode::KEY_LEFTCTRL));
        assert!(keys.contains(&KeyCode::KEY_LEFTALT));
        assert!(keys.contains(&KeyCode::KEY_A));
    }

    #[test]
    fn parse_hotkey_function_key() {
        let keys = parse_hotkey("F12").unwrap();
        assert_eq!(keys.len(), 1);
        assert!(keys.contains(&KeyCode::KEY_F12));
    }

    #[test]
    fn parse_hotkey_invalid_key() {
        let result = parse_hotkey("Super+InvalidKey");
        assert!(result.is_err());
    }

    #[test]
    fn parse_hotkey_number_key() {
        let keys = parse_hotkey("Ctrl+1").unwrap();
        assert!(keys.contains(&KeyCode::KEY_1));
    }

    // -- combo_active tests -------------------------------------------------

    #[test]
    fn combo_active_all_held() {
        let groups = build_key_groups("Super+Shift+Space").unwrap();
        let mut held = HashSet::new();
        held.insert(KeyCode::KEY_LEFTMETA);
        held.insert(KeyCode::KEY_RIGHTSHIFT);
        held.insert(KeyCode::KEY_SPACE);
        assert!(combo_active(&groups, &held));
    }

    #[test]
    fn combo_active_missing_key() {
        let groups = build_key_groups("Super+Shift+Space").unwrap();
        let mut held = HashSet::new();
        held.insert(KeyCode::KEY_LEFTMETA);
        held.insert(KeyCode::KEY_RIGHTSHIFT);
        // Missing Space.
        assert!(!combo_active(&groups, &held));
    }

    #[test]
    fn combo_active_empty_held() {
        let groups = build_key_groups("Super+Space").unwrap();
        let held = HashSet::new();
        assert!(!combo_active(&groups, &held));
    }

    // -- EvdevHotkeyManager unit tests --------------------------------------

    #[test]
    fn evdev_manager_not_bound_initially() {
        let mgr = EvdevHotkeyManager::new();
        assert!(!mgr.is_bound());
    }

    #[test]
    fn evdev_manager_bind_without_sender_fails() {
        let mut mgr = EvdevHotkeyManager::new();
        // Don't set event sender — bind should fail.
        // Note: this may also fail due to input group, which is fine.
        let result = mgr.bind("Super+Space");
        assert!(result.is_err());
    }
}
