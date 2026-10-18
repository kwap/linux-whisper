//! Typing dictated text while it is spoken, the way phone dictation does it:
//! the words appear at the cursor as they are recognised, and when
//! recognition changes its mind about the last ones, they are erased and
//! typed again.
//!
//! The text is typed on the virtual keyboard (`/dev/uinput`) with the user's
//! own keyboard layout, so it works on every desktop and keeps the clipboard
//! out of it. Accented letters the layout has no key for are typed with its
//! dead keys (on a Polish layout, AltGr+[ then a gives ä), and anything left
//! is pasted.

use std::collections::HashMap;
use std::fs;
use std::time::Duration;

use evdev::uinput::VirtualDevice;
use evdev::{EventType, InputEvent, KeyCode};
use linux_whisper_core::config::PasteShortcut;
use tracing::debug;

use crate::clipboard::create_clipboard;
use crate::display::DisplayServer;
use crate::text_inject::{press_shortcut, with_virtual_keyboard, InjectError};

/// One key press on the virtual keyboard: the key and the modifiers that
/// choose the character on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stroke {
    /// evdev key code.
    pub key: u16,
    /// Shift held (the second level, or the fourth with `level3`).
    pub shift: bool,
    /// The third level held (AltGr on most layouts).
    pub level3: bool,
}

/// How to type characters with the user's keyboard layout. The app fills it
/// from the desktop's keymap.
#[derive(Debug, Clone, Default)]
pub struct KeyboardLayout {
    keys: HashMap<char, Stroke>,
    /// Dead keys, by the combining accent they put on the next letter.
    dead_keys: HashMap<char, Stroke>,
    /// The key that selects the third level, if the layout has one.
    level3_key: Option<u16>,
}

impl KeyboardLayout {
    pub fn new(level3_key: Option<u16>) -> Self {
        Self {
            level3_key,
            ..Self::default()
        }
    }

    /// Note that `stroke` types `ch`. The first way found is kept, so add the
    /// simplest ones (lowest levels) first.
    pub fn add_key(&mut self, ch: char, stroke: Stroke) {
        if self.can_press(stroke) {
            self.keys.entry(ch).or_insert(stroke);
        }
    }

    /// Note that `stroke` is a dead key adding `accent` (a combining
    /// character, see [`dead_key_accent`]) to the next letter.
    pub fn add_dead_key(&mut self, accent: char, stroke: Stroke) {
        if self.can_press(stroke) {
            self.dead_keys.entry(accent).or_insert(stroke);
        }
    }

    /// Number of characters the layout can type directly.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// The strokes that type `ch`, if the layout can: its key, or a dead key
    /// and the plain letter.
    pub fn strokes(&self, ch: char) -> Option<Vec<Stroke>> {
        if let Some(&stroke) = self.keys.get(&ch) {
            return Some(vec![stroke]);
        }
        let (letter, accent) = decompose(ch)?;
        let dead = *self.dead_keys.get(&accent)?;
        let letter = *self.keys.get(&letter)?;
        Some(vec![dead, letter])
    }

    fn can_press(&self, stroke: Stroke) -> bool {
        !stroke.level3 || self.level3_key.is_some()
    }
}

/// The combining accent a dead key adds, by its keysym name (e.g.
/// "dead_diaeresis").
pub fn dead_key_accent(keysym_name: &str) -> Option<char> {
    Some(match keysym_name {
        "dead_grave" => '\u{300}',
        "dead_acute" => '\u{301}',
        "dead_circumflex" => '\u{302}',
        "dead_tilde" => '\u{303}',
        "dead_diaeresis" => '\u{308}',
        "dead_abovering" => '\u{30A}',
        "dead_doubleacute" => '\u{30B}',
        "dead_caron" => '\u{30C}',
        "dead_cedilla" => '\u{327}',
        "dead_ogonek" => '\u{328}',
        "dead_abovedot" => '\u{307}',
        _ => return None,
    })
}

/// Accented letters by accent: pairs of plain and accented letter.
const ACCENTED: &[(char, &str)] = &[
    ('\u{300}', "aàeèiìoòuùAÀEÈIÌOÒUÙ"),
    (
        '\u{301}',
        "aáeéiíoóuúyýcćnńsśzźlĺrŕAÁEÉIÍOÓUÚYÝCĆNŃSŚZŹLĹRŔ",
    ),
    ('\u{302}', "aâeêiîoôuûAÂEÊIÎOÔUÛ"),
    ('\u{303}', "aãoõnñAÃOÕNÑ"),
    ('\u{308}', "aäeëiïoöuüyÿAÄEËIÏOÖUÜYŸ"),
    ('\u{30A}', "aåuůAÅUŮ"),
    ('\u{30B}', "oőuűOŐUŰ"),
    ('\u{30C}', "cčdďeěnňrřsštťzžCČDĎEĚNŇRŘSŠTŤZŽ"),
    ('\u{327}', "cçsşCÇSŞ"),
    ('\u{328}', "aąeęAĄEĘ"),
    ('\u{307}', "zżeėZŻEĖ"),
];

/// The plain letter and accent of an accented letter.
fn decompose(ch: char) -> Option<(char, char)> {
    ACCENTED.iter().find_map(|&(accent, pairs)| {
        let letters: Vec<char> = pairs.chars().collect();
        letters
            .chunks(2)
            .find(|pair| pair[1] == ch)
            .map(|pair| (pair[0], accent))
    })
}

/// How to turn `typed` into `wanted`: how many characters to erase from the
/// end of `typed`, and what to type after that.
pub fn edit<'a>(typed: &str, wanted: &'a str) -> (usize, &'a str) {
    let mut same = 0;
    for ((at, a), b) in wanted.char_indices().zip(typed.chars()) {
        if a != b {
            break;
        }
        same = at + a.len_utf8();
    }
    let erase = typed.chars().count() - wanted[..same].chars().count();
    (erase, &wanted[same..])
}

/// Pause after each key event. Applications handle typing this fast fine,
/// and it keeps the events from arriving in one burst.
const KEY_GAP: Duration = Duration::from_millis(1);

/// After pasting, the application fetches the text from the clipboard in its
/// own time; keys pressed before it has would land before the text.
const PASTE_SETTLE: Duration = Duration::from_millis(150);

/// Text being typed into the focused window while it is dictated.
pub struct LiveTyping {
    layout: KeyboardLayout,
    paste: PasteShortcut,
    display: DisplayServer,
    /// The text typed so far.
    typed: String,
}

impl LiveTyping {
    pub fn new(layout: KeyboardLayout, paste: PasteShortcut, display: DisplayServer) -> Self {
        Self {
            layout,
            paste,
            display,
            typed: String::new(),
        }
    }

    /// The text typed so far.
    pub fn typed(&self) -> &str {
        &self.typed
    }

    /// Make the typed text read `wanted`: erase back to where the two differ,
    /// then type the rest. Only ever touches the text typed by this session.
    pub fn show(&mut self, wanted: &str) -> Result<(), InjectError> {
        let (erase, add) = edit(&self.typed, wanted);
        if erase == 0 && add.is_empty() {
            return Ok(());
        }
        debug!("Live typing: erase {erase}, type {:?}", add);
        let caps_lock = caps_lock_on();
        let layout = &self.layout;
        let typed = &mut self.typed;
        let (paste, display) = (self.paste, &self.display);

        with_virtual_keyboard(|dev| {
            for _ in 0..erase {
                tap(dev, &[], KeyCode::KEY_BACKSPACE.code())?;
                typed.pop();
            }

            let mut unknown = String::new();
            for ch in add.chars() {
                let Some(mut strokes) = layout.strokes(ch) else {
                    unknown.push(ch);
                    continue;
                };
                if !unknown.is_empty() {
                    paste_text(dev, display, paste, &unknown)?;
                    typed.push_str(&unknown);
                    unknown.clear();
                }
                // Caps Lock turns the case of letters around.
                if caps_lock && is_cased(ch) {
                    if let Some(letter) = strokes.last_mut() {
                        letter.shift = !letter.shift;
                    }
                }
                for stroke in strokes {
                    let mut modifiers = Vec::new();
                    if stroke.shift {
                        modifiers.push(KeyCode::KEY_LEFTSHIFT.code());
                    }
                    if stroke.level3 {
                        modifiers.extend(layout.level3_key);
                    }
                    tap(dev, &modifiers, stroke.key)?;
                }
                typed.push(ch);
            }
            if !unknown.is_empty() {
                paste_text(dev, display, paste, &unknown)?;
                typed.push_str(&unknown);
            }
            Ok(())
        })
    }
}

/// Whether Caps Lock changes `ch`: a letter with an upper and a lower case.
fn is_cased(ch: char) -> bool {
    ch.is_lowercase() || ch.is_uppercase()
}

/// Press `key` with `modifiers` held, and let go of all of them.
fn tap(dev: &mut VirtualDevice, modifiers: &[u16], key: u16) -> Result<(), InjectError> {
    let mut send = |code: u16, value: i32| {
        dev.emit(&[InputEvent::new(EventType::KEY.0, code, value)])
            .map_err(|e| InjectError::InjectFailed(format!("virtual keyboard: {e}")))?;
        std::thread::sleep(KEY_GAP);
        Ok::<(), InjectError>(())
    };
    let pressed = modifiers
        .iter()
        .try_for_each(|&m| send(m, 1))
        .and_then(|()| send(key, 1));
    // Always let go of everything, even after a failed press: the keyboard
    // lives on, and a stuck Shift would affect all typing.
    let released = std::iter::once(key)
        .chain(modifiers.iter().rev().copied())
        .map(|k| send(k, 0))
        .fold(Ok(()), Result::and);
    pressed.and(released)
}

/// Put `text` in through the clipboard, for characters the layout cannot type.
fn paste_text(
    dev: &mut VirtualDevice,
    display: &DisplayServer,
    shortcut: PasteShortcut,
    text: &str,
) -> Result<(), InjectError> {
    create_clipboard(display)
        .set_text(text)
        .map_err(|e| InjectError::InjectFailed(format!("clipboard: {e}")))?;
    press_shortcut(dev, shortcut)?;
    std::thread::sleep(PASTE_SETTLE);
    Ok(())
}

/// Whether Caps Lock is on, from the keyboard lights (readable by everyone).
fn caps_lock_on() -> bool {
    let Ok(leds) = fs::read_dir("/sys/class/leds") else {
        return false;
    };
    leds.flatten()
        .filter(|led| led.file_name().to_string_lossy().ends_with("::capslock"))
        .any(|led| fs::read_to_string(led.path().join("brightness")).is_ok_and(|b| b.trim() != "0"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: KeyCode) -> Stroke {
        Stroke {
            key: key.code(),
            shift: false,
            level3: false,
        }
    }

    fn shifted(stroke: Stroke) -> Stroke {
        Stroke {
            shift: true,
            ..stroke
        }
    }

    fn altgr(stroke: Stroke) -> Stroke {
        Stroke {
            level3: true,
            ..stroke
        }
    }

    /// A few keys of the Polish layout.
    fn polish() -> KeyboardLayout {
        let mut layout = KeyboardLayout::new(Some(KeyCode::KEY_RIGHTALT.code()));
        layout.add_key('a', key(KeyCode::KEY_A));
        layout.add_key('A', shifted(key(KeyCode::KEY_A)));
        layout.add_key('ą', altgr(key(KeyCode::KEY_A)));
        layout.add_key('o', key(KeyCode::KEY_O));
        layout.add_key('ß', altgr(key(KeyCode::KEY_T)));
        layout.add_key(' ', key(KeyCode::KEY_SPACE));
        layout.add_dead_key('\u{308}', altgr(key(KeyCode::KEY_LEFTBRACE)));
        layout
    }

    #[test]
    fn edit_erases_back_to_the_first_difference() {
        assert_eq!(edit("", "Hello"), (0, "Hello"));
        assert_eq!(edit("Hello", "Hello world"), (0, " world"));
        assert_eq!(edit("Hello word", "Hello world."), (1, "ld."));
        assert_eq!(edit("I scream", "Ice cream"), (7, "ce cream"));
        assert_eq!(edit("Hello", "Hello"), (0, ""));
        assert_eq!(edit("Hello there", "Hello"), (6, ""));
    }

    #[test]
    fn edit_counts_characters_not_bytes() {
        assert_eq!(edit("Zażółć", "Zażółw"), (1, "w"));
        assert_eq!(edit("über", "Über"), (4, "Über"));
    }

    #[test]
    fn letters_are_typed_with_their_level() {
        let layout = polish();
        assert_eq!(layout.strokes('a'), Some(vec![key(KeyCode::KEY_A)]));
        assert_eq!(
            layout.strokes('A'),
            Some(vec![shifted(key(KeyCode::KEY_A))])
        );
        assert_eq!(layout.strokes('ą'), Some(vec![altgr(key(KeyCode::KEY_A))]));
        assert_eq!(layout.strokes('ß'), Some(vec![altgr(key(KeyCode::KEY_T))]));
    }

    #[test]
    fn missing_accented_letters_use_a_dead_key() {
        let layout = polish();
        assert_eq!(
            layout.strokes('ä'),
            Some(vec![
                altgr(key(KeyCode::KEY_LEFTBRACE)),
                key(KeyCode::KEY_A)
            ])
        );
        assert_eq!(
            layout.strokes('ö'),
            Some(vec![
                altgr(key(KeyCode::KEY_LEFTBRACE)),
                key(KeyCode::KEY_O)
            ])
        );
        // No dead key for the circumflex, no key for the letter.
        assert_eq!(layout.strokes('â'), None);
        assert_eq!(layout.strokes('ü'), None);
        assert_eq!(layout.strokes('€'), None);
    }

    #[test]
    fn the_first_and_simplest_way_wins() {
        let mut layout = polish();
        layout.add_key('a', altgr(key(KeyCode::KEY_Q)));
        assert_eq!(layout.strokes('a'), Some(vec![key(KeyCode::KEY_A)]));
    }

    #[test]
    fn without_altgr_third_level_keys_are_left_out() {
        let mut layout = KeyboardLayout::new(None);
        layout.add_key('ą', altgr(key(KeyCode::KEY_A)));
        layout.add_dead_key('\u{308}', altgr(key(KeyCode::KEY_LEFTBRACE)));
        assert_eq!(layout.strokes('ą'), None);
        assert!(layout.is_empty());
    }

    #[test]
    fn dead_key_names_map_to_accents() {
        assert_eq!(dead_key_accent("dead_diaeresis"), Some('\u{308}'));
        assert_eq!(dead_key_accent("dead_ogonek"), Some('\u{328}'));
        assert_eq!(dead_key_accent("a"), None);
        assert_eq!(decompose('Ž'), Some(('Z', '\u{30C}')));
        assert_eq!(decompose('x'), None);
    }

    /// Types with the real virtual keyboard and reads the keys back. Needs
    /// write access to /dev/uinput and read access to /dev/input; run with
    /// `cargo test -p linux-whisper-platform --lib -- --ignored`. The test
    /// grabs the virtual keyboard before typing, so no key reaches the
    /// desktop.
    #[test]
    #[ignore]
    fn live_typing_corrects_what_it_typed() {
        let mut device = crate::text_inject::grab_own_virtual_keyboard();

        let mut typing = LiveTyping::new(polish(), PasteShortcut::CtrlV, DisplayServer::Unknown);
        typing.show("Aa ą").expect("type");
        typing.show("Aa ä").expect("correct");
        assert_eq!(typing.typed(), "Aa ä");

        let shift = KeyCode::KEY_LEFTSHIFT.code();
        let altgr = KeyCode::KEY_RIGHTALT.code();
        let (a, space, brace) = (
            KeyCode::KEY_A.code(),
            KeyCode::KEY_SPACE.code(),
            KeyCode::KEY_LEFTBRACE.code(),
        );
        let backspace = KeyCode::KEY_BACKSPACE.code();
        let caps = caps_lock_on();
        let mut expected = vec![];
        let mut tap = |modifiers: &[u16], key: u16| {
            expected.extend(modifiers.iter().map(|&m| (m, 1)));
            expected.extend([(key, 1), (key, 0)]);
            expected.extend(modifiers.iter().rev().map(|&m| (m, 0)));
        };
        let (upper, lower): (&[u16], &[u16]) = if caps {
            (&[], &[shift])
        } else {
            (&[shift], &[])
        };
        tap(upper, a);
        tap(lower, a);
        tap(&[], space);
        let mut altgr_lower = lower.to_vec();
        altgr_lower.push(altgr);
        tap(&altgr_lower, a);
        tap(&[], backspace);
        tap(&[altgr], brace);
        tap(lower, a);

        let mut keys = Vec::new();
        while keys.len() < expected.len() {
            for ev in device.fetch_events().expect("read events") {
                if ev.event_type() == EventType::KEY {
                    keys.push((ev.code(), ev.value()));
                }
            }
        }
        let _ = device.ungrab();
        assert_eq!(keys, expected);
    }
}
