//! Global hotkey: reads the keyboards in `/dev/input` (evdev), so it works on
//! X11 and on every Wayland desktop, and can keep the hotkey's keys from the
//! focused app (see [`EvdevHotkeyManager`]).

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use evdev::uinput::VirtualDevice;
use evdev::{
    AttributeSet, Device, EventType, InputEvent, KeyCode, RelativeAxisCode, SynchronizationCode,
};
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
    let lower = name.to_lowercase();
    let key = match lower.as_str() {
        // Modifiers (return both L/R so either physical key matches).
        "super" | "meta" | "logo" | "win" => {
            return Ok(vec![KeyCode::KEY_LEFTMETA, KeyCode::KEY_RIGHTMETA])
        }
        "shift" => return Ok(vec![KeyCode::KEY_LEFTSHIFT, KeyCode::KEY_RIGHTSHIFT]),
        "ctrl" | "control" => return Ok(vec![KeyCode::KEY_LEFTCTRL, KeyCode::KEY_RIGHTCTRL]),
        // Left Alt only: on many layouts Right Alt is AltGr, which types
        // letters such as ś, and must not trigger (and eat) an Alt shortcut.
        "alt" => KeyCode::KEY_LEFTALT,
        "altgr" => KeyCode::KEY_RIGHTALT,

        // Common keys.
        "space" => KeyCode::KEY_SPACE,
        "enter" | "return" => KeyCode::KEY_ENTER,
        "tab" => KeyCode::KEY_TAB,
        "escape" | "esc" => KeyCode::KEY_ESC,
        "backspace" => KeyCode::KEY_BACKSPACE,
        "insert" | "ins" => KeyCode::KEY_INSERT,
        "delete" | "del" => KeyCode::KEY_DELETE,
        "home" => KeyCode::KEY_HOME,
        "end" => KeyCode::KEY_END,
        "pageup" | "pgup" => KeyCode::KEY_PAGEUP,
        "pagedown" | "pgdn" => KeyCode::KEY_PAGEDOWN,
        "up" => KeyCode::KEY_UP,
        "down" => KeyCode::KEY_DOWN,
        "left" => KeyCode::KEY_LEFT,
        "right" => KeyCode::KEY_RIGHT,
        "pause" | "break" => KeyCode::KEY_PAUSE,
        "scrolllock" => KeyCode::KEY_SCROLLLOCK,
        "printscreen" | "print" | "sysrq" => KeyCode::KEY_SYSRQ,
        "menu" | "compose" => KeyCode::KEY_COMPOSE,
        "grave" | "`" => KeyCode::KEY_GRAVE,
        "minus" | "-" => KeyCode::KEY_MINUS,
        "equal" | "=" => KeyCode::KEY_EQUAL,
        "comma" | "," => KeyCode::KEY_COMMA,
        "period" | "." => KeyCode::KEY_DOT,
        "slash" | "/" => KeyCode::KEY_SLASH,
        "backslash" | "\\" => KeyCode::KEY_BACKSLASH,
        "semicolon" | ";" => KeyCode::KEY_SEMICOLON,
        "apostrophe" | "'" => KeyCode::KEY_APOSTROPHE,
        "bracketleft" | "[" => KeyCode::KEY_LEFTBRACE,
        "bracketright" | "]" => KeyCode::KEY_RIGHTBRACE,

        // Letters. Evdev codes follow the physical QWERTY layout, not the
        // alphabet (KEY_A = 30, KEY_B = 48, ...), so they need a table.
        s if s.len() == 1 && s.as_bytes()[0].is_ascii_alphabetic() => {
            const LETTERS: [KeyCode; 26] = [
                KeyCode::KEY_A,
                KeyCode::KEY_B,
                KeyCode::KEY_C,
                KeyCode::KEY_D,
                KeyCode::KEY_E,
                KeyCode::KEY_F,
                KeyCode::KEY_G,
                KeyCode::KEY_H,
                KeyCode::KEY_I,
                KeyCode::KEY_J,
                KeyCode::KEY_K,
                KeyCode::KEY_L,
                KeyCode::KEY_M,
                KeyCode::KEY_N,
                KeyCode::KEY_O,
                KeyCode::KEY_P,
                KeyCode::KEY_Q,
                KeyCode::KEY_R,
                KeyCode::KEY_S,
                KeyCode::KEY_T,
                KeyCode::KEY_U,
                KeyCode::KEY_V,
                KeyCode::KEY_W,
                KeyCode::KEY_X,
                KeyCode::KEY_Y,
                KeyCode::KEY_Z,
            ];
            LETTERS[(s.as_bytes()[0] - b'a') as usize]
        }

        // Numbers. KEY_1..KEY_9 are contiguous, KEY_0 follows KEY_9.
        s if s.len() == 1 && s.as_bytes()[0].is_ascii_digit() => match s.as_bytes()[0] {
            b'0' => KeyCode::KEY_0,
            d => KeyCode::new(KeyCode::KEY_1.code() + u16::from(d - b'1')),
        },

        // Function keys F1–F24.
        s if s.starts_with('f') && s.len() > 1 => {
            let num: u16 = s[1..]
                .parse()
                .map_err(|_| HotkeyError::BindError(format!("unknown key: {name}")))?;
            match num {
                1..=10 => KeyCode::new(KeyCode::KEY_F1.code() + num - 1),
                11 => KeyCode::KEY_F11,
                12 => KeyCode::KEY_F12,
                13..=24 => KeyCode::new(KeyCode::KEY_F13.code() + num - 13),
                _ => {
                    return Err(HotkeyError::BindError(format!(
                        "function key out of range: {name}"
                    )));
                }
            }
        }

        _ => return Err(HotkeyError::BindError(format!("unknown key: {name}"))),
    };
    Ok(vec![key])
}

// ---------------------------------------------------------------------------
// Key names for the shortcut editor
// ---------------------------------------------------------------------------

/// The modifier part of a hotkey for an evdev key code ("Ctrl", "Shift",
/// "Alt", "AltGr", "Super"), or `None` if the key is not a modifier.
pub fn modifier_name(code: u16) -> Option<&'static str> {
    match KeyCode::new(code) {
        KeyCode::KEY_LEFTCTRL | KeyCode::KEY_RIGHTCTRL => Some("Ctrl"),
        KeyCode::KEY_LEFTSHIFT | KeyCode::KEY_RIGHTSHIFT => Some("Shift"),
        KeyCode::KEY_LEFTALT => Some("Alt"),
        KeyCode::KEY_RIGHTALT => Some("AltGr"),
        KeyCode::KEY_LEFTMETA | KeyCode::KEY_RIGHTMETA => Some("Super"),
        _ => None,
    }
}

/// The name a hotkey uses for a (non-modifier) evdev key code, e.g. "Space",
/// "D", "F13"; `None` for keys a hotkey cannot use.
pub fn key_name(code: u16) -> Option<String> {
    const NAMED: [(KeyCode, &str); 33] = [
        (KeyCode::KEY_SPACE, "Space"),
        (KeyCode::KEY_ENTER, "Enter"),
        (KeyCode::KEY_TAB, "Tab"),
        (KeyCode::KEY_ESC, "Esc"),
        (KeyCode::KEY_BACKSPACE, "Backspace"),
        (KeyCode::KEY_INSERT, "Insert"),
        (KeyCode::KEY_DELETE, "Delete"),
        (KeyCode::KEY_HOME, "Home"),
        (KeyCode::KEY_END, "End"),
        (KeyCode::KEY_PAGEUP, "PageUp"),
        (KeyCode::KEY_PAGEDOWN, "PageDown"),
        (KeyCode::KEY_UP, "Up"),
        (KeyCode::KEY_DOWN, "Down"),
        (KeyCode::KEY_LEFT, "Left"),
        (KeyCode::KEY_RIGHT, "Right"),
        (KeyCode::KEY_PAUSE, "Pause"),
        (KeyCode::KEY_SCROLLLOCK, "ScrollLock"),
        (KeyCode::KEY_SYSRQ, "PrintScreen"),
        (KeyCode::KEY_COMPOSE, "Menu"),
        (KeyCode::KEY_GRAVE, "Grave"),
        (KeyCode::KEY_MINUS, "Minus"),
        (KeyCode::KEY_EQUAL, "Equal"),
        (KeyCode::KEY_COMMA, "Comma"),
        (KeyCode::KEY_DOT, "Period"),
        (KeyCode::KEY_SLASH, "Slash"),
        (KeyCode::KEY_BACKSLASH, "Backslash"),
        (KeyCode::KEY_SEMICOLON, "Semicolon"),
        (KeyCode::KEY_APOSTROPHE, "Apostrophe"),
        (KeyCode::KEY_LEFTBRACE, "BracketLeft"),
        (KeyCode::KEY_RIGHTBRACE, "BracketRight"),
        (KeyCode::KEY_0, "0"),
        (KeyCode::KEY_F11, "F11"),
        (KeyCode::KEY_F12, "F12"),
    ];
    let key = KeyCode::new(code);
    if let Some((_, name)) = NAMED.iter().find(|(k, _)| *k == key) {
        return Some(name.to_string());
    }
    // Letters, digits and function keys: whatever name maps back to this key.
    let candidates = ('A'..='Z')
        .map(String::from)
        .chain(('1'..='9').map(String::from))
        .chain((1..=24).map(|n| format!("F{n}")));
    candidates
        .into_iter()
        .find(|name| map_key_name(name).is_ok_and(|keys| keys == [key]))
}

/// Whether a hotkey without Ctrl, Alt or Super would take a key away from
/// typing: a plain key (or Shift or AltGr + key) that types a character or
/// edits text. Function keys and keys like Pause or Insert are fine on their
/// own.
pub fn hotkey_blocks_typing(hotkey: &str) -> bool {
    let parts: Vec<String> = hotkey.split('+').map(|p| p.trim().to_lowercase()).collect();
    let has_modifier = parts.iter().any(|p| {
        matches!(
            p.as_str(),
            "ctrl" | "control" | "alt" | "super" | "meta" | "logo" | "win"
        )
    });
    let Some(key) = parts.last() else {
        return false;
    };
    let standalone = (key.starts_with('f') && key.len() > 1 && key[1..].parse::<u8>().is_ok())
        || matches!(
            key.as_str(),
            "insert"
                | "ins"
                | "pause"
                | "break"
                | "scrolllock"
                | "printscreen"
                | "print"
                | "sysrq"
                | "menu"
        );
    !has_modifier && !standalone
}

// ---------------------------------------------------------------------------
// EvdevHotkeyManager
// ---------------------------------------------------------------------------

/// Global hotkey manager using Linux evdev input devices.
///
/// Requires the current user to be in the `input` group (or have read access
/// to `/dev/input/event*` devices).
///
/// Reading the keyboards alone would leave the hotkey's keys to the focused
/// app as well: Super+Shift+Space would also type a space. So, with
/// [`keep_keys_from_apps`](Self::keep_keys_from_apps) on and `/dev/uinput`
/// writable, each keyboard is grabbed and its events are passed on through a
/// virtual copy of it, all except the key press that triggers the hotkey.
/// This is what keyd and xremap do, and it works on X11 and every Wayland
/// desktop.
pub struct EvdevHotkeyManager {
    event_sender: Option<tokio::sync::mpsc::Sender<HotkeyEvent>>,
    shutdown: Option<Arc<AtomicBool>>,
    thread: Option<thread::JoinHandle<()>>,
    bound: bool,
    keep_keys: bool,
    /// Set by the listener while at least one keyboard is grabbed.
    keeping_keys: Arc<AtomicBool>,
    /// Only use the keyboard with this name (tests).
    only: Option<String>,
}

impl Default for EvdevHotkeyManager {
    fn default() -> Self {
        Self::new()
    }
}

impl EvdevHotkeyManager {
    pub fn new() -> Self {
        Self {
            event_sender: None,
            shutdown: None,
            thread: None,
            bound: false,
            keep_keys: false,
            keeping_keys: Arc::new(AtomicBool::new(false)),
            only: None,
        }
    }

    /// Keep the key that triggers the hotkey from the focused app. Takes
    /// effect on the next [`bind`](HotkeyManager::bind).
    pub fn keep_keys_from_apps(&mut self, keep: bool) {
        self.keep_keys = keep;
    }

    /// Whether the hotkey's keys are being kept from other apps right now.
    pub fn is_keeping_keys(&self) -> bool {
        self.keeping_keys.load(Ordering::Relaxed)
    }
}

impl HotkeyManager for EvdevHotkeyManager {
    fn bind(&mut self, hotkey: &str) -> Result<(), HotkeyError> {
        let sender = self.event_sender.clone().ok_or_else(|| {
            HotkeyError::BindError("event sender not set — call set_event_sender() first".into())
        })?;

        let combo = Combo::parse(hotkey)?;
        info!("Binding hotkey \"{hotkey}\"");

        // Open all keyboard devices.
        let mut seen = HashSet::new();
        let devices = open_new_keyboards(&mut seen, self.only.as_deref())?;
        if devices.is_empty() {
            return Err(HotkeyError::PermissionDenied(
                if check_input_group_membership() {
                    "cannot open the keyboards in /dev/input. You are in the input group, \
                     but it only takes effect after you log out and back in"
                } else {
                    "no access to the keyboards in /dev/input. Run: \
                     sudo usermod -aG input $USER, then log out and back in"
                }
                .into(),
            ));
        }
        info!("Monitoring {} keyboard device(s)", devices.len());

        let keep_keys = self.keep_keys && uinput_writable();
        if self.keep_keys && !keep_keys {
            warn!(
                "Cannot write to /dev/uinput, so the hotkey's keys also reach the focused app. \
                 The udev rule installed with Linux Whisper grants access"
            );
        }
        let keyboards: Vec<Keyboard> = devices
            .into_iter()
            .map(|(path, device)| Keyboard::new(path, device, keep_keys, &combo))
            .collect();

        let shutdown = Arc::new(AtomicBool::new(false));
        let listener = Listener {
            keyboards,
            seen,
            combo,
            keep_keys,
            sender,
            shutdown: shutdown.clone(),
            keeping_keys: self.keeping_keys.clone(),
            only: self.only.clone(),
            held: HashSet::new(),
            swallowed: HashSet::new(),
            fired: false,
        };

        let handle = thread::Builder::new()
            .name("evdev-hotkey".into())
            .spawn(move || listener.run())
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
        self.keeping_keys.store(false, Ordering::Relaxed);
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
// Combo
// ---------------------------------------------------------------------------

/// The modifier keys. A hotkey fires only with exactly its own modifiers
/// held, so Ctrl+Space does not also fire (and hide) Ctrl+Shift+Space.
const MODIFIERS: [KeyCode; 8] = [
    KeyCode::KEY_LEFTCTRL,
    KeyCode::KEY_RIGHTCTRL,
    KeyCode::KEY_LEFTSHIFT,
    KeyCode::KEY_RIGHTSHIFT,
    KeyCode::KEY_LEFTALT,
    KeyCode::KEY_RIGHTALT,
    KeyCode::KEY_LEFTMETA,
    KeyCode::KEY_RIGHTMETA,
];

/// A parsed hotkey: pressing the last part while the others are held fires
/// it. Each part is a group of keys, any of which will do (left or right
/// Shift, for example).
#[derive(Debug, Clone)]
struct Combo {
    held: Vec<Vec<KeyCode>>,
    trigger: Vec<KeyCode>,
}

impl Combo {
    fn parse(hotkey: &str) -> Result<Self, HotkeyError> {
        let mut parts = hotkey
            .split('+')
            .map(|part| map_key_name(part.trim()))
            .collect::<Result<Vec<_>, _>>()?;
        let trigger = parts
            .pop()
            .ok_or_else(|| HotkeyError::BindError("empty hotkey string".into()))?;
        Ok(Self {
            held: parts,
            trigger,
        })
    }

    /// Whether pressing `key` while `held` are down fires the hotkey.
    fn fires(&self, key: KeyCode, held: &HashSet<KeyCode>) -> bool {
        let in_combo = |k: &KeyCode| self.held.iter().any(|group| group.contains(k));
        self.trigger.contains(&key)
            && self
                .held
                .iter()
                .all(|group| group.iter().any(|k| held.contains(k)))
            && held
                .iter()
                .all(|k| !MODIFIERS.contains(k) || in_combo(k) || self.trigger.contains(k))
    }

    /// Whether the trigger can be kept from other apps. A modifier cannot:
    /// the desktop would never see it released.
    fn trigger_can_be_kept(&self) -> bool {
        !self.trigger.iter().any(|k| MODIFIERS.contains(k))
    }
}

// ---------------------------------------------------------------------------
// Keyboards
// ---------------------------------------------------------------------------

/// Part of the names of the virtual keyboards this app creates. Devices with
/// it are never read: they only repeat what the app itself sends.
const VIRTUAL_DEVICE_TAG: &str = "Linux Whisper";

/// A key the desktop sees but no app does (it types nothing). Tapped after
/// a kept hotkey, so the desktop does not take Super+(kept key) for a lone
/// Super press, which opens the application menu on KDE and GNOME.
const NO_OP_KEY: KeyCode = KeyCode::KEY_UNKNOWN;

/// How long the desktop gets to pick up a virtual keyboard before keys are
/// sent through it; keys sent earlier would be lost.
const MIRROR_SETTLE: Duration = Duration::from_millis(400);

struct Keyboard {
    path: PathBuf,
    name: String,
    // Declared before `mirror`, so on drop the keyboard is released before its
    // copy disappears.
    device: Device,
    /// The virtual copy that the desktop reads while `device` is grabbed.
    mirror: Option<VirtualDevice>,
    /// When the keyboard can be grabbed (its copy has been picked up by then).
    grab_at: Option<Instant>,
    /// Events of the current report, passed on together at its end.
    frame: Vec<InputEvent>,
    /// Tap `NO_OP_KEY` at the end of the current report.
    tap_no_op: bool,
}

impl Keyboard {
    fn new(path: PathBuf, device: Device, keep_keys: bool, combo: &Combo) -> Self {
        let name = device.name().unwrap_or("keyboard").to_string();
        if let Err(e) = device.set_nonblocking(true) {
            warn!("Failed to set {name} non-blocking: {e}");
        }
        let mut keyboard = Self {
            path,
            name,
            device,
            mirror: None,
            grab_at: None,
            frame: Vec::new(),
            tap_no_op: false,
        };
        if keep_keys {
            keyboard.start_keeping(combo);
        }
        keyboard
    }

    /// Create the keyboard's copy; the keyboard is grabbed once the desktop
    /// has picked the copy up.
    fn start_keeping(&mut self, combo: &Combo) {
        if !combo.trigger_can_be_kept() || !can_keep_keys(&self.device, combo) {
            return;
        }
        match mirror_of(&self.device, &self.name) {
            Ok(mirror) => {
                self.mirror = Some(mirror);
                self.grab_at = Some(Instant::now() + MIRROR_SETTLE);
            }
            Err(e) => warn!("Cannot create a virtual copy of {}: {e}", self.name),
        }
    }

    fn is_grabbed(&self) -> bool {
        self.device.is_grabbed()
    }

    /// Grab the keyboard once its copy is ready and no key is down: the
    /// desktop saw those presses from this keyboard and must see their
    /// releases from it too, or the keys would stay pressed.
    fn grab_when_ready(&mut self) {
        let Some(at) = self.grab_at else {
            return;
        };
        if Instant::now() < at || self.is_grabbed() || keys_down(&self.device) {
            return;
        }
        if let Err(e) = self.device.grab() {
            warn!("Cannot grab {} ({e}); its keys are only read", self.name);
            self.give_up_keeping();
            return;
        }
        // A key pressed just before the grab took effect: back off and retry.
        if keys_down(&self.device) {
            let _ = self.device.ungrab();
            return;
        }
        self.grab_at = None;
        info!(
            "Keeping the hotkey from other apps on {} ({})",
            self.name,
            self.path.display()
        );
    }

    /// Stop grabbing: the keyboard goes back to being read by everyone.
    fn give_up_keeping(&mut self) {
        if self.is_grabbed() {
            let _ = self.device.ungrab();
        }
        self.mirror = None;
        self.grab_at = None;
        self.frame.clear();
    }

    /// Pass the current report on through the copy. A report left with only
    /// its scan code (its key was kept) is dropped.
    fn pass_on_frame(&mut self) {
        let frame = std::mem::take(&mut self.frame);
        let tap = std::mem::take(&mut self.tap_no_op);
        let Some(mirror) = self.mirror.as_mut() else {
            return;
        };
        let mut result = Ok(());
        if frame.iter().any(|e| e.event_type() != EventType::MISC) {
            result = mirror.emit(&frame);
        }
        if tap && result.is_ok() {
            result = mirror
                .emit(&[InputEvent::new(EventType::KEY.0, NO_OP_KEY.code(), 1)])
                .and_then(|()| {
                    mirror.emit(&[InputEvent::new(EventType::KEY.0, NO_OP_KEY.code(), 0)])
                });
        }
        if let Err(e) = result {
            // Never leave a keyboard grabbed that nobody else can read.
            error!("Cannot pass keys on from {}: {e}", self.name);
            self.give_up_keeping();
        }
    }
}

/// Whether any key of `device` is down (or its state cannot be read).
fn keys_down(device: &Device) -> bool {
    device
        .get_key_state()
        .map_or(true, |keys| keys.iter().next().is_some())
}

/// Whether `device` is a keyboard that can be grabbed to keep the hotkey from
/// other apps: it has the trigger key and letters, and no pointer (keyboards
/// whose USB receiver also serves a mouse are only read).
fn can_keep_keys(device: &Device, combo: &Combo) -> bool {
    let Some(keys) = device.supported_keys() else {
        return false;
    };
    let pointer = keys.contains(KeyCode::BTN_LEFT)
        || keys.contains(KeyCode::BTN_TOUCH)
        || device.supported_absolute_axes().is_some()
        || device
            .supported_relative_axes()
            .is_some_and(|axes| axes.contains(RelativeAxisCode::REL_X));
    combo.trigger.iter().any(|&k| keys.contains(k))
        && keys.contains(KeyCode::KEY_A)
        && keys.contains(KeyCode::KEY_Z)
        && !pointer
}

/// The name of the virtual copy of keyboard `name`: the name with the tag
/// added, shortened to what uinput accepts (78 bytes) without losing the tag.
fn mirror_name(name: &str) -> String {
    let tag = format!(" ({VIRTUAL_DEVICE_TAG})");
    let mut base = name.to_string();
    while base.len() + tag.len() > 78 {
        base.pop();
    }
    base + &tag
}

/// A virtual keyboard with the same keys and identity as `device`.
fn mirror_of(device: &Device, name: &str) -> std::io::Result<VirtualDevice> {
    let mut keys = AttributeSet::<KeyCode>::new();
    for key in device.supported_keys().into_iter().flatten() {
        keys.insert(key);
    }
    keys.insert(NO_OP_KEY);
    let mirror_name = mirror_name(name);
    let mut builder = VirtualDevice::builder()?
        .name(&mirror_name)
        .input_id(device.input_id())
        .with_keys(&keys)?;
    if let Some(axes) = device.supported_relative_axes() {
        builder = builder.with_relative_axes(axes)?;
    }
    if let Some(misc) = device.misc_properties() {
        builder = builder.with_msc(misc)?;
    }
    builder.build()
}

fn uinput_writable() -> bool {
    fs::OpenOptions::new()
        .write(true)
        .open("/dev/uinput")
        .is_ok()
}

// ---------------------------------------------------------------------------
// Listener thread
// ---------------------------------------------------------------------------

/// How often to look for keyboards connected after the hotkey was bound
/// (e.g. a Bluetooth keyboard that connects after login, or one that
/// reconnects after sleep).
const HOTPLUG_SCAN_INTERVAL: Duration = Duration::from_secs(3);

struct Listener {
    keyboards: Vec<Keyboard>,
    /// Device paths already looked at.
    seen: HashSet<PathBuf>,
    combo: Combo,
    keep_keys: bool,
    sender: tokio::sync::mpsc::Sender<HotkeyEvent>,
    shutdown: Arc<AtomicBool>,
    keeping_keys: Arc<AtomicBool>,
    only: Option<String>,
    /// Keys held down, on any keyboard.
    held: HashSet<KeyCode>,
    /// Trigger keys kept from other apps until they are released.
    swallowed: HashSet<KeyCode>,
    /// The hotkey fired and its trigger is still down.
    fired: bool,
}

impl Listener {
    fn run(mut self) {
        use std::os::fd::AsFd;

        let mut last_scan = Instant::now();
        let mut paused = false;
        loop {
            if self.shutdown.load(Ordering::Relaxed) {
                debug!("Hotkey listener shutting down");
                // Dropping the keyboards releases them and removes their copies.
                return;
            }

            // The process is about to stop (see `release_keyboards_on_stop`).
            match PAUSE.load(Ordering::SeqCst) {
                PAUSE_REQUESTED if !paused => {
                    for keyboard in &mut self.keyboards {
                        keyboard.give_up_keeping();
                    }
                    paused = true;
                    PAUSE.store(PAUSED, Ordering::SeqCst);
                }
                RUNNING if paused => {
                    paused = false;
                    if self.keep_keys {
                        for keyboard in &mut self.keyboards {
                            keyboard.start_keeping(&self.combo);
                        }
                    }
                }
                _ => {}
            }

            if last_scan.elapsed() >= HOTPLUG_SCAN_INTERVAL {
                last_scan = Instant::now();
                self.add_new_keyboards();
            }

            for keyboard in &mut self.keyboards {
                keyboard.grab_when_ready();
            }
            self.keeping_keys.store(
                self.keyboards.iter().any(Keyboard::is_grabbed),
                Ordering::Relaxed,
            );

            // Wait for input, waking up regularly for the checks above.
            let ready: Vec<usize> = {
                let mut pollfds: Vec<PollFd> = self
                    .keyboards
                    .iter()
                    .map(|k| PollFd::new(k.device.as_fd(), PollFlags::POLLIN))
                    .collect();
                match poll(&mut pollfds, PollTimeout::from(100u8)) {
                    Ok(0) | Err(nix::errno::Errno::EINTR) => continue,
                    Err(e) => {
                        warn!("poll() error: {e}");
                        thread::sleep(Duration::from_millis(100));
                        continue;
                    }
                    Ok(_) => pollfds
                        .iter()
                        .enumerate()
                        .filter(|(_, pfd)| pfd.revents().is_some_and(|r| !r.is_empty()))
                        .map(|(i, _)| i)
                        .collect(),
                }
            };

            let mut gone = Vec::new();
            for i in ready {
                if let Err(e) = self.read_keyboard(i) {
                    if e.kind() != std::io::ErrorKind::WouldBlock {
                        info!("Keyboard {} is gone ({e})", self.keyboards[i].name);
                        gone.push(i);
                    }
                }
            }
            // Unplugged (or asleep): forget it; the scan picks it up again.
            for i in gone.into_iter().rev() {
                let keyboard = self.keyboards.remove(i);
                self.seen.remove(&keyboard.path);
                self.held.clear();
                self.swallowed.clear();
                self.fired = false;
            }
        }
    }

    fn add_new_keyboards(&mut self) {
        // Devices that are gone can come back under the same path.
        self.seen.retain(|path| path.exists());
        match open_new_keyboards(&mut self.seen, self.only.as_deref()) {
            Ok(devices) if !devices.is_empty() => {
                info!("Monitoring {} newly connected keyboard(s)", devices.len());
                for (path, device) in devices {
                    let keyboard = Keyboard::new(path, device, self.keep_keys, &self.combo);
                    self.keyboards.push(keyboard);
                }
            }
            Ok(_) => {}
            Err(e) => debug!("Keyboard scan failed: {e}"),
        }
    }

    /// Handle the events waiting on keyboard `i`: watch for the hotkey and,
    /// if the keyboard is grabbed, pass every other event on.
    fn read_keyboard(&mut self, i: usize) -> std::io::Result<()> {
        let keyboard = &mut self.keyboards[i];
        let grabbed = keyboard.is_grabbed();
        let events: Vec<InputEvent> = keyboard.device.fetch_events()?.collect();
        for event in events {
            let keep = match event.event_type() {
                EventType::KEY => {
                    let key = KeyCode::new(event.code());
                    match event.value() {
                        1 => {
                            let fires = self.combo.fires(key, &self.held);
                            self.held.insert(key);
                            if fires {
                                debug!("Hotkey pressed on {}", keyboard.name);
                                self.fired = true;
                                let _ = self.sender.try_send(HotkeyEvent::Pressed);
                            }
                            let keep = fires && grabbed && self.combo.trigger_can_be_kept();
                            if keep {
                                self.swallowed.insert(key);
                                keyboard.tap_no_op = !self.combo.held.is_empty();
                            }
                            keep
                        }
                        0 => {
                            self.held.remove(&key);
                            if self.fired && self.combo.trigger.contains(&key) {
                                self.fired = false;
                                let _ = self.sender.try_send(HotkeyEvent::Released);
                            }
                            self.swallowed.remove(&key)
                        }
                        // Auto-repeat.
                        _ => self.swallowed.contains(&key),
                    }
                }
                _ => false,
            };

            if !grabbed {
                continue;
            }
            match event.event_type() {
                EventType::SYNCHRONIZATION if event.code() == SynchronizationCode::SYN_REPORT.0 => {
                    keyboard.pass_on_frame();
                }
                // SYN_DROPPED: the event reader already put in what was lost.
                EventType::SYNCHRONIZATION => {}
                _ if !keep => keyboard.frame.push(event),
                _ => {}
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Stopping the process
// ---------------------------------------------------------------------------

/// Whether the process is about to stop: the listener lets go of the
/// keyboards (PAUSE_REQUESTED → PAUSED) and takes them again after.
static PAUSE: AtomicU8 = AtomicU8::new(RUNNING);
const RUNNING: u8 = 0;
const PAUSE_REQUESTED: u8 = 1;
const PAUSED: u8 = 2;

/// Make Ctrl+Z safe in the terminal Linux Whisper was started from. A stopped
/// process cannot pass keys on, so stopped while holding the keyboards it
/// would leave them dead, and `fg` could not even be typed. On SIGTSTP the
/// keyboards are let go first, then the process stops itself; when it
/// continues they are taken again. (A debugger stops it without warning:
/// turn keeping the shortcut from other apps off while debugging.)
pub fn release_keyboards_on_stop(rt: &tokio::runtime::Handle) {
    use nix::sys::signal::{raise, Signal};
    use tokio::signal::unix::{signal, SignalKind};

    rt.spawn(async move {
        let mut stops = match signal(SignalKind::from_raw(Signal::SIGTSTP as i32)) {
            Ok(stops) => stops,
            Err(e) => {
                warn!("Cannot watch for Ctrl+Z: {e}");
                return;
            }
        };
        while stops.recv().await.is_some() {
            PAUSE.store(PAUSE_REQUESTED, Ordering::SeqCst);
            // The listener checks at least every 100 ms; none may be running.
            for _ in 0..50 {
                if PAUSE.load(Ordering::SeqCst) == PAUSED {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            info!("Stopping (Ctrl+Z); the keyboards are released until the process continues");
            if let Err(e) = raise(Signal::SIGSTOP) {
                warn!("Cannot stop: {e}");
            }
            // Continued.
            PAUSE.store(RUNNING, Ordering::SeqCst);
        }
    });
}

// ---------------------------------------------------------------------------
// Device discovery
// ---------------------------------------------------------------------------

/// Open the `/dev/input/event*` devices not yet in `seen` that support
/// keyboard keys. Every device that could be opened is added to `seen`, so
/// later calls only look at devices connected (or made accessible) since.
/// This app's own virtual keyboards are skipped, and with `only`, all but the
/// keyboard of that name.
fn open_new_keyboards(
    seen: &mut HashSet<PathBuf>,
    only: Option<&str>,
) -> Result<Vec<(PathBuf, Device)>, HotkeyError> {
    let entries = fs::read_dir("/dev/input")
        .map_err(|e| HotkeyError::PermissionDenied(format!("cannot read /dev/input: {e}")))?;

    let mut keyboards = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        if !name.starts_with("event") || seen.contains(&path) {
            continue;
        }

        match Device::open(&path) {
            Ok(dev) => {
                // Only remember devices that could be opened: one that failed
                // (e.g. before udev set its permissions) is tried again later.
                seen.insert(path.clone());
                if dev.name().is_some_and(|n| n.contains(VIRTUAL_DEVICE_TAG))
                    || only.is_some_and(|only| dev.name() != Some(only))
                {
                    continue;
                }
                // Check if the device supports KEY events and has keyboard keys.
                if let Some(supported) = dev.supported_keys() {
                    if supported.contains(KeyCode::KEY_SPACE) {
                        debug!(
                            "Opened keyboard device: {} ({})",
                            dev.name().unwrap_or("unknown"),
                            path.display()
                        );
                        keyboards.push((path, dev));
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
                if members.iter().any(|m| m.trim() == username) {
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

        mock.expect_is_bound().times(1).returning(|| true);

        mock.expect_unbind().times(1).returning(|| Ok(()));

        assert!(mock.bind("ctrl+shift+space").is_ok());
        assert!(mock.is_bound());
        assert!(mock.unbind().is_ok());
    }

    #[test]
    fn mock_hotkey_manager_bind_error() {
        let mut mock = MockHotkeyManager::new();

        mock.expect_bind()
            .times(1)
            .returning(|key| Err(HotkeyError::BindError(format!("unknown key: {key}"))));

        let err = mock.bind("invalid_key").unwrap_err();
        assert!(matches!(err, HotkeyError::BindError(_)));
    }

    #[test]
    fn mock_hotkey_manager_permission_denied() {
        let mut mock = MockHotkeyManager::new();

        mock.expect_bind().times(1).returning(|_| {
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

        mock.expect_set_event_sender().times(1).return_const(());

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
    fn parse_hotkey_letters_use_their_own_key_codes() {
        // Evdev codes are in keyboard order, not alphabetical order.
        assert_eq!(parse_hotkey("D").unwrap(), vec![KeyCode::KEY_D]);
        assert_eq!(parse_hotkey("b").unwrap(), vec![KeyCode::KEY_B]);
        assert_eq!(parse_hotkey("Z").unwrap(), vec![KeyCode::KEY_Z]);
        let all: Vec<KeyCode> = ('a'..='z')
            .map(|c| parse_hotkey(&c.to_string()).unwrap()[0])
            .collect();
        let unique: HashSet<KeyCode> = all.iter().copied().collect();
        assert_eq!(unique.len(), 26);
    }

    #[test]
    fn parse_hotkey_extended_keys() {
        assert_eq!(parse_hotkey("F13").unwrap(), vec![KeyCode::KEY_F13]);
        assert_eq!(parse_hotkey("F24").unwrap(), vec![KeyCode::KEY_F24]);
        assert_eq!(parse_hotkey("Pause").unwrap(), vec![KeyCode::KEY_PAUSE]);
        assert_eq!(parse_hotkey("Insert").unwrap(), vec![KeyCode::KEY_INSERT]);
        assert_eq!(parse_hotkey("`").unwrap(), vec![KeyCode::KEY_GRAVE]);
        assert!(parse_hotkey("F25").is_err());
        assert!(parse_hotkey("Ctrl+").is_err());
    }

    #[test]
    fn parse_hotkey_digits() {
        assert_eq!(parse_hotkey("0").unwrap(), vec![KeyCode::KEY_0]);
        assert_eq!(parse_hotkey("9").unwrap(), vec![KeyCode::KEY_9]);
    }

    #[test]
    fn parse_hotkey_number_key() {
        let keys = parse_hotkey("Ctrl+1").unwrap();
        assert!(keys.contains(&KeyCode::KEY_1));
    }

    // -- Combo tests ----------------------------------------------------------

    fn held(keys: &[KeyCode]) -> HashSet<KeyCode> {
        keys.iter().copied().collect()
    }

    #[test]
    fn combo_fires_when_the_last_key_is_pressed_with_the_others_held() {
        let combo = Combo::parse("Super+Shift+Space").unwrap();
        let mods = held(&[KeyCode::KEY_LEFTMETA, KeyCode::KEY_RIGHTSHIFT]);
        assert!(combo.fires(KeyCode::KEY_SPACE, &mods));
        // Missing Shift.
        assert!(!combo.fires(KeyCode::KEY_SPACE, &held(&[KeyCode::KEY_LEFTMETA])));
        // Not the trigger.
        assert!(!combo.fires(KeyCode::KEY_A, &mods));
        assert!(!combo.fires(KeyCode::KEY_SPACE, &HashSet::new()));
    }

    #[test]
    fn combo_needs_exactly_its_own_modifiers() {
        let combo = Combo::parse("Ctrl+Space").unwrap();
        assert!(combo.fires(KeyCode::KEY_SPACE, &held(&[KeyCode::KEY_LEFTCTRL])));
        // Ctrl+Shift+Space is a different shortcut, left to the apps.
        assert!(!combo.fires(
            KeyCode::KEY_SPACE,
            &held(&[KeyCode::KEY_LEFTCTRL, KeyCode::KEY_LEFTSHIFT])
        ));
        // Other keys held (not modifiers) do not matter.
        assert!(combo.fires(
            KeyCode::KEY_SPACE,
            &held(&[KeyCode::KEY_LEFTCTRL, KeyCode::KEY_W])
        ));
    }

    #[test]
    fn a_single_key_combo_fires_alone() {
        let combo = Combo::parse("F13").unwrap();
        assert!(combo.fires(KeyCode::KEY_F13, &HashSet::new()));
        assert!(combo.trigger_can_be_kept());
        assert!(!Combo::parse("Ctrl+Shift").unwrap().trigger_can_be_kept());
    }

    #[test]
    fn key_names_map_back_to_their_keys() {
        for code in [
            KeyCode::KEY_SPACE,
            KeyCode::KEY_D,
            KeyCode::KEY_Z,
            KeyCode::KEY_1,
            KeyCode::KEY_0,
            KeyCode::KEY_F1,
            KeyCode::KEY_F12,
            KeyCode::KEY_F13,
            KeyCode::KEY_F24,
            KeyCode::KEY_PAUSE,
            KeyCode::KEY_GRAVE,
            KeyCode::KEY_INSERT,
        ] {
            let name = key_name(code.code()).unwrap_or_else(|| panic!("{code:?} has no name"));
            assert_eq!(parse_hotkey(&name).unwrap(), vec![code], "{name}");
        }
        assert_eq!(key_name(KeyCode::KEY_D.code()).as_deref(), Some("D"));
        assert_eq!(key_name(KeyCode::KEY_LEFTSHIFT.code()), None);
        assert_eq!(modifier_name(KeyCode::KEY_RIGHTMETA.code()), Some("Super"));
        assert_eq!(modifier_name(KeyCode::KEY_A.code()), None);
    }

    #[test]
    fn long_keyboard_names_keep_the_tag() {
        let long = "Logitech G915 TKL LIGHTSPEED Wireless RGB Mechanical Gaming Keyboard ünïcode";
        let name = mirror_name(long);
        assert!(name.len() <= 78, "{} bytes", name.len());
        assert!(name.ends_with(" (Linux Whisper)"));
        assert_eq!(mirror_name("AT Keyboard"), "AT Keyboard (Linux Whisper)");
    }

    #[test]
    fn alt_is_left_alt_and_altgr_is_right_alt() {
        assert_eq!(
            parse_hotkey("Alt+S").unwrap(),
            vec![KeyCode::KEY_LEFTALT, KeyCode::KEY_S]
        );
        assert_eq!(modifier_name(KeyCode::KEY_RIGHTALT.code()), Some("AltGr"));
        // AltGr+S types ś on a Polish layout: it must not fire Alt+S.
        let combo = Combo::parse("Alt+S").unwrap();
        assert!(!combo.fires(KeyCode::KEY_S, &held(&[KeyCode::KEY_RIGHTALT])));
        assert!(combo.fires(KeyCode::KEY_S, &held(&[KeyCode::KEY_LEFTALT])));
        assert!(hotkey_blocks_typing("AltGr+S"));
    }

    #[test]
    fn plain_typing_keys_are_not_good_hotkeys() {
        assert!(hotkey_blocks_typing("D"));
        assert!(hotkey_blocks_typing("Shift+D"));
        assert!(hotkey_blocks_typing("Space"));
        assert!(!hotkey_blocks_typing("Ctrl+D"));
        assert!(!hotkey_blocks_typing("Super+Shift+Space"));
        assert!(!hotkey_blocks_typing("F9"));
        assert!(!hotkey_blocks_typing("Shift+F13"));
        assert!(!hotkey_blocks_typing("Pause"));
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

    /// Against the real kernel: a fake keyboard is grabbed and passed on
    /// through its copy, all but the hotkey's Space. Needs /dev/uinput and
    /// /dev/input access; touches no other keyboard, and nothing reaches the
    /// desktop (the copy is grabbed by the test). Run with
    /// `cargo test -p linux-whisper-platform --lib -- --ignored grab`.
    #[test]
    #[ignore]
    fn grab_keeps_the_hotkey_from_other_apps() {
        use std::os::fd::AsFd;

        let name = format!("LW test keyboard {}", std::process::id());
        let mut keys = AttributeSet::<KeyCode>::new();
        for code in KeyCode::KEY_ESC.code()..=KeyCode::KEY_MICMUTE.code() {
            keys.insert(KeyCode::new(code));
        }
        let mut fake = VirtualDevice::builder()
            .and_then(|b| b.name(&name).with_keys(&keys))
            .and_then(|b| b.build())
            .expect("cannot create a fake keyboard: no /dev/uinput access");
        let fake_path = fake
            .enumerate_dev_nodes_blocking()
            .unwrap()
            .flatten()
            .find(|p| p.to_string_lossy().contains("event"))
            .expect("fake keyboard node");
        // udev gives the new node its permissions a moment after it appears.
        let deadline = Instant::now() + Duration::from_secs(3);
        while Device::open(&fake_path).is_err() {
            assert!(
                Instant::now() < deadline,
                "cannot open {}",
                fake_path.display()
            );
            thread::sleep(Duration::from_millis(50));
        }

        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let mut mgr = EvdevHotkeyManager::new();
        mgr.only = Some(name.clone());
        mgr.set_event_sender(tx);
        mgr.keep_keys_from_apps(true);
        mgr.bind("Super+Shift+Space").unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        while !mgr.is_keeping_keys() {
            assert!(
                Instant::now() < deadline,
                "the fake keyboard was never grabbed"
            );
            thread::sleep(Duration::from_millis(50));
        }
        // Grabbed by the listener: a second grab fails.
        let mut probe = Device::open(&fake_path).unwrap();
        assert!(probe.grab().is_err(), "fake keyboard not grabbed");
        drop(probe);

        // The copy: grab it so that what it sends stays with this test.
        let mirror_name = mirror_name(&name);
        let mut mirror = fs::read_dir("/dev/input")
            .unwrap()
            .flatten()
            .filter_map(|e| Device::open(e.path()).ok())
            .find(|d| d.name() == Some(mirror_name.as_str()))
            .expect("copy of the fake keyboard");
        mirror.grab().unwrap();
        mirror.set_nonblocking(true).unwrap();

        let press = |fake: &mut VirtualDevice, key: KeyCode, value: i32| {
            fake.emit(&[InputEvent::new(EventType::KEY.0, key.code(), value)])
                .unwrap();
            thread::sleep(Duration::from_millis(20));
        };
        press(&mut fake, KeyCode::KEY_A, 1);
        press(&mut fake, KeyCode::KEY_A, 0);
        press(&mut fake, KeyCode::KEY_LEFTMETA, 1);
        press(&mut fake, KeyCode::KEY_LEFTSHIFT, 1);
        press(&mut fake, KeyCode::KEY_SPACE, 1);
        press(&mut fake, KeyCode::KEY_SPACE, 0);
        press(&mut fake, KeyCode::KEY_LEFTSHIFT, 0);
        press(&mut fake, KeyCode::KEY_LEFTMETA, 0);
        thread::sleep(Duration::from_millis(200));

        let mut passed = Vec::new();
        loop {
            let ready = {
                let mut fds = [PollFd::new(mirror.as_fd(), PollFlags::POLLIN)];
                poll(&mut fds, PollTimeout::from(200u8)).unwrap_or(0) > 0
            };
            if !ready {
                break;
            }
            for e in mirror.fetch_events().unwrap() {
                if e.event_type() == EventType::KEY {
                    passed.push((KeyCode::new(e.code()), e.value()));
                }
            }
        }
        assert_eq!(
            passed,
            vec![
                (KeyCode::KEY_A, 1),
                (KeyCode::KEY_A, 0),
                (KeyCode::KEY_LEFTMETA, 1),
                (KeyCode::KEY_LEFTSHIFT, 1),
                (NO_OP_KEY, 1),
                (NO_OP_KEY, 0),
                (KeyCode::KEY_LEFTSHIFT, 0),
                (KeyCode::KEY_LEFTMETA, 0),
            ]
        );
        assert_eq!(rx.try_recv().ok(), Some(HotkeyEvent::Pressed));
        assert_eq!(rx.try_recv().ok(), Some(HotkeyEvent::Released));

        // Unbinding gives the keyboard back.
        mgr.unbind().unwrap();
        assert!(!mgr.is_keeping_keys());
        let mut probe = Device::open(&fake_path).unwrap();
        assert!(probe.grab().is_ok(), "fake keyboard still grabbed");
    }
}
