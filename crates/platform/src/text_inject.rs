use std::fs::OpenOptions;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, EventType, InputEvent, KeyCode};
use linux_whisper_core::config::PasteShortcut;
use tracing::{info, warn};

use crate::clipboard::create_clipboard;
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

impl Default for XdotoolInjector {
    fn default() -> Self {
        Self::new()
    }
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

impl Default for WtypeInjector {
    fn default() -> Self {
        Self::new()
    }
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

impl Default for YdotoolInjector {
    fn default() -> Self {
        Self::new()
    }
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
            stdin.write_all(text.as_bytes()).map_err(|e| {
                InjectError::InjectFailed(format!("failed to write to ydotool: {e}"))
            })?;
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

/// Pastes the clipboard into the focused window by pressing the paste
/// shortcut on a virtual keyboard (`/dev/uinput`). Unlike wtype, this works on
/// every compositor, including GNOME and KDE, and with any text.
pub struct UinputPaster {
    shortcut: PasteShortcut,
}

/// The virtual keyboard, created once and kept: the compositor needs a moment
/// to pick up a new input device, so a fresh one per paste would be too slow.
static VIRTUAL_KEYBOARD: Mutex<Option<VirtualDevice>> = Mutex::new(None);

impl UinputPaster {
    pub fn new(shortcut: PasteShortcut) -> Self {
        Self { shortcut }
    }

    /// Whether the virtual keyboard can be created (write access to
    /// `/dev/uinput`, which the udev rule installed with the app grants).
    pub fn is_permitted() -> bool {
        OpenOptions::new().write(true).open("/dev/uinput").is_ok()
    }

    /// Create the virtual keyboard ahead of the first paste.
    pub fn prepare() -> Result<(), InjectError> {
        with_virtual_keyboard(|_| Ok(()))
    }
}

impl TextInjector for UinputPaster {
    /// The text must already be on the clipboard.
    fn inject_text(&self, _text: &str) -> Result<(), InjectError> {
        let (modifiers, key) = shortcut_keys(self.shortcut);
        with_virtual_keyboard(|dev| {
            let mut send = |code: KeyCode, value: i32| {
                dev.emit(&[InputEvent::new(EventType::KEY.0, code.code(), value)])
                    .map_err(|e| InjectError::InjectFailed(format!("virtual keyboard: {e}")))
            };
            let pressed = modifiers
                .iter()
                .try_for_each(|&m| send(m, 1))
                .and_then(|()| {
                    std::thread::sleep(Duration::from_millis(20));
                    send(key, 1)
                });
            std::thread::sleep(Duration::from_millis(20));
            // Always release everything, even after a failed press: the
            // keyboard lives on, and a stuck Ctrl would affect all typing.
            let released = std::iter::once(key)
                .chain(modifiers.iter().rev().copied())
                .map(|k| send(k, 0))
                .fold(Ok(()), Result::and);
            pressed.and(released)
        })
    }

    fn is_available(&self) -> bool {
        Self::is_permitted()
    }
}

/// Modifier keys and key that make up a paste shortcut.
fn shortcut_keys(shortcut: PasteShortcut) -> (&'static [KeyCode], KeyCode) {
    match shortcut {
        PasteShortcut::CtrlV => (&[KeyCode::KEY_LEFTCTRL], KeyCode::KEY_V),
        PasteShortcut::CtrlShiftV => (
            &[KeyCode::KEY_LEFTCTRL, KeyCode::KEY_LEFTSHIFT],
            KeyCode::KEY_V,
        ),
        PasteShortcut::ShiftInsert => (&[KeyCode::KEY_LEFTSHIFT], KeyCode::KEY_INSERT),
    }
}

fn with_virtual_keyboard<T>(
    f: impl FnOnce(&mut VirtualDevice) -> Result<T, InjectError>,
) -> Result<T, InjectError> {
    let mut guard = VIRTUAL_KEYBOARD
        .lock()
        .map_err(|_| InjectError::InjectFailed("virtual keyboard lock poisoned".into()))?;
    if guard.is_none() {
        // Declare the standard keyboard keys, not just the four it presses:
        // udev only tags a device as a keyboard (ID_INPUT_KEYBOARD) when it
        // has the usual key block, and compositors treat it accordingly.
        let mut keys = AttributeSet::<KeyCode>::new();
        for code in KeyCode::KEY_ESC.code()..=KeyCode::KEY_MICMUTE.code() {
            keys.insert(KeyCode::new(code));
        }
        let device = VirtualDevice::builder()
            .and_then(|b| b.name("Linux Whisper paste keyboard").with_keys(&keys))
            .and_then(|b| b.build())
            .map_err(|e| {
                InjectError::ToolNotFound(format!("cannot create virtual keyboard: {e}"))
            })?;
        *guard = Some(device);
        // Give the compositor time to add the new keyboard.
        std::thread::sleep(Duration::from_millis(400));
    }
    let device = guard
        .as_mut()
        .ok_or_else(|| InjectError::InjectFailed("virtual keyboard missing".into()))?;
    f(device)
}

/// How dictated text reached the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
    /// Typed into the focused window by the named tool.
    Typed(&'static str),
    /// Pasted into the focused window with the paste shortcut.
    Pasted,
    /// Only copied to the clipboard; the reason says why it was not inserted.
    ClipboardOnly(String),
    /// Neither inserted nor copied.
    Failed(String),
}

/// Copy `text` to the clipboard and, when `insert` is set, put it into the
/// focused window with the first method that works on this desktop:
///
/// - Wayland: wtype (wlroots compositors), then the virtual-keyboard paste
///   (GNOME, KDE, any compositor), then ydotool.
/// - X11: xdotool, then the virtual-keyboard paste.
pub fn deliver_text(
    display: &DisplayServer,
    text: &str,
    insert: bool,
    shortcut: PasteShortcut,
) -> Delivery {
    let copied = match create_clipboard(display).set_text(text) {
        Ok(()) => true,
        Err(e) => {
            warn!("Clipboard copy failed: {e}");
            false
        }
    };

    if !insert {
        return if copied {
            Delivery::ClipboardOnly("auto-paste is turned off".into())
        } else {
            Delivery::Failed("could not copy to the clipboard".into())
        };
    }

    let mut problems: Vec<String> = Vec::new();

    // Type the text directly where the desktop allows it.
    let typist: Option<(Box<dyn TextInjector>, &'static str)> = match display {
        DisplayServer::Wayland => Some((Box::new(WtypeInjector::new()), "wtype")),
        DisplayServer::X11 => Some((Box::new(XdotoolInjector::new()), "xdotool")),
        DisplayServer::Unknown => None,
    };
    if let Some((injector, tool)) = typist {
        if try_typing(injector.as_ref(), tool, text, &mut problems) {
            return Delivery::Typed(tool);
        }
    }

    if copied {
        let paster = UinputPaster::new(shortcut);
        if paster.is_available() {
            match paster.inject_text(text) {
                Ok(()) => return Delivery::Pasted,
                Err(e) => problems.push(format!("virtual keyboard: {e}")),
            }
        } else {
            problems.push("no write access to /dev/uinput".into());
        }
    }

    if *display != DisplayServer::X11
        && try_typing(&YdotoolInjector::new(), "ydotool", text, &mut problems)
    {
        return Delivery::Typed("ydotool");
    }

    let reason = if problems.is_empty() {
        "no way to insert text was found".to_string()
    } else {
        problems.join("; ")
    };
    if copied {
        Delivery::ClipboardOnly(reason)
    } else {
        Delivery::Failed(reason)
    }
}

/// Type `text` with `injector` if its tool is installed, noting failures.
fn try_typing(
    injector: &dyn TextInjector,
    tool: &str,
    text: &str,
    problems: &mut Vec<String>,
) -> bool {
    if !injector.is_available() {
        return false;
    }
    match injector.inject_text(text) {
        Ok(()) => true,
        Err(e) => {
            info!("{tool} could not insert the text: {e}");
            problems.push(format!("{tool}: {e}"));
            false
        }
    }
}

/// Check if a binary is available on `$PATH`.
pub(crate) fn which(binary: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;

    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            Path::new(&dir)
                .join(binary)
                .metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    })
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
            .returning(|_| Err(InjectError::ToolNotFound("xdotool not found".to_string())));

        let err = mock.inject_text("text").unwrap_err();
        assert!(matches!(err, InjectError::ToolNotFound(_)));
    }

    #[test]
    fn mock_injector_inject_failed() {
        let mut mock = MockTextInjector::new();

        mock.expect_inject_text()
            .times(1)
            .returning(|_| Err(InjectError::InjectFailed("process crashed".to_string())));

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
    fn paste_shortcuts_press_the_right_keys() {
        assert_eq!(
            shortcut_keys(PasteShortcut::CtrlV),
            (&[KeyCode::KEY_LEFTCTRL][..], KeyCode::KEY_V)
        );
        assert_eq!(
            shortcut_keys(PasteShortcut::CtrlShiftV),
            (
                &[KeyCode::KEY_LEFTCTRL, KeyCode::KEY_LEFTSHIFT][..],
                KeyCode::KEY_V
            )
        );
        assert_eq!(
            shortcut_keys(PasteShortcut::ShiftInsert),
            (&[KeyCode::KEY_LEFTSHIFT][..], KeyCode::KEY_INSERT)
        );
    }

    /// Presses the paste shortcut on the real virtual keyboard and reads the
    /// key events back. Needs write access to /dev/uinput and read access to
    /// /dev/input; run with
    /// `cargo test -p linux-whisper-platform --lib -- --ignored uinput`.
    /// The test grabs the virtual keyboard before pressing anything, so no
    /// key reaches the desktop.
    #[test]
    #[ignore]
    fn uinput_paster_presses_the_shortcut() {
        UinputPaster::prepare().expect("create the virtual keyboard");
        let mut device = evdev::enumerate()
            .map(|(_, d)| d)
            .find(|d| d.name() == Some("Linux Whisper paste keyboard"))
            .expect("virtual keyboard listed in /dev/input");
        device.grab().expect("grab the virtual keyboard");

        UinputPaster::new(PasteShortcut::CtrlShiftV)
            .inject_text("")
            .expect("press the shortcut");

        let mut keys = Vec::new();
        while keys.len() < 6 {
            for ev in device.fetch_events().expect("read events") {
                if ev.event_type() == EventType::KEY {
                    keys.push((KeyCode::new(ev.code()), ev.value()));
                }
            }
        }
        let _ = device.ungrab();

        assert_eq!(
            keys,
            vec![
                (KeyCode::KEY_LEFTCTRL, 1),
                (KeyCode::KEY_LEFTSHIFT, 1),
                (KeyCode::KEY_V, 1),
                (KeyCode::KEY_V, 0),
                (KeyCode::KEY_LEFTSHIFT, 0),
                (KeyCode::KEY_LEFTCTRL, 0),
            ]
        );
    }

    #[test]
    fn which_finds_programs_on_path() {
        assert!(which("sh"));
        assert!(!which("definitely-not-a-real-program-name"));
    }

    #[test]
    fn ydotool_injector_not_available_returns_error() {
        let injector = YdotoolInjector { available: false };
        assert!(!injector.is_available());
        let err = injector.inject_text("test").unwrap_err();
        assert!(matches!(err, InjectError::ToolNotFound(_)));
    }
}
