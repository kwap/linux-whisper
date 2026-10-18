// SPDX-License-Identifier: GPL-3.0-or-later
//! The dictation shortcut: showing it as key caps, and a dialog that records
//! a new one from the keys the user presses.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, glib};

use linux_whisper_core::config::AppConfig;
use linux_whisper_platform::hotkey::{hotkey_blocks_typing, key_name, modifier_name, parse_hotkey};

/// A hotkey shown as key caps: [Super] + [Shift] + [Space].
#[derive(Clone)]
pub struct Keycaps {
    pub widget: gtk::Box,
}

impl Keycaps {
    pub fn new(large: bool) -> Self {
        let widget = gtk::Box::builder()
            .spacing(6)
            .valign(gtk::Align::Center)
            .css_classes(if large {
                vec!["keycaps", "large"]
            } else {
                vec!["keycaps"]
            })
            .build();
        Self { widget }
    }

    /// Show `hotkey` (e.g. "Super+Shift+Space").
    pub fn set(&self, hotkey: &str) {
        while let Some(child) = self.widget.first_child() {
            self.widget.remove(&child);
        }
        for (i, part) in hotkey.split('+').map(str::trim).enumerate() {
            if i > 0 {
                self.widget.append(
                    &gtk::Label::builder()
                        .label("+")
                        .css_classes(["keycap-plus"])
                        .build(),
                );
            }
            self.widget.append(
                &gtk::Label::builder()
                    .label(key_label(part))
                    .css_classes(["keycap"])
                    .build(),
            );
        }
    }

    /// Show modifiers being held, waiting for the key: [Ctrl] + [Alt] + …
    fn set_pending(&self, modifiers: &[&str]) {
        self.set(&modifiers.join("+"));
        self.widget.append(
            &gtk::Label::builder()
                .label("+ \u{2026}")
                .css_classes(["keycap-plus"])
                .build(),
        );
    }
}

/// How a key of a hotkey is written on its cap: "Space", "D", "F13", "PgUp".
fn key_label(part: &str) -> String {
    let mut chars = part.chars();
    match chars.next() {
        Some(first) if part.len() == 1 => first.to_uppercase().to_string(),
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The modifiers in the order a hotkey names them.
const MODIFIER_ORDER: [&str; 5] = ["Super", "Ctrl", "Alt", "AltGr", "Shift"];

/// Ask for a new dictation shortcut: the user presses it, sees it, and
/// confirms. `on_set` gets the new hotkey (e.g. "Ctrl+Alt+D"); `on_closed`
/// runs when the dialog closes, set or not.
///
/// While the dialog is open the desktop's own shortcuts are suspended for
/// this window, so combinations with Super reach it too.
pub fn ask_for_shortcut(
    parent: &impl IsA<gtk::Widget>,
    on_set: impl Fn(String) + 'static,
    on_closed: impl Fn() + 'static,
) {
    let current = AppConfig::load().hotkey;
    let current_hotkey = current.clone();

    let preview = Keycaps::new(true);
    preview.widget.set_halign(gtk::Align::Center);
    preview.set(&current);
    let prompt = gtk::Label::builder()
        .label("Press the keys you want to use")
        .css_classes(["title-4"])
        .wrap(true)
        .justify(gtk::Justification::Center)
        .build();
    let message = gtk::Label::builder()
        .label(format!(
            "Currently {current}. Use Ctrl, Alt or Super with a key, or a key such as F9 \
             or Pause on its own. Enter sets it, Esc cancels."
        ))
        .css_classes(["dim-label"])
        .wrap(true)
        .justify(gtk::Justification::Center)
        .build();

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .margin_top(24)
        .margin_bottom(30)
        .margin_start(30)
        .margin_end(30)
        .build();
    content.append(&prompt);
    content.append(&preview.widget);
    content.append(&message);

    let cancel = gtk::Button::with_label("Cancel");
    let set = gtk::Button::builder()
        .label("Set Shortcut")
        .css_classes(["suggested-action"])
        .sensitive(false)
        .build();
    let header = adw::HeaderBar::builder()
        .show_start_title_buttons(false)
        .show_end_title_buttons(false)
        .build();
    header.pack_start(&cancel);
    header.pack_end(&set);

    let toolbar = adw::ToolbarView::builder().content(&content).build();
    toolbar.add_top_bar(&header);

    let dialog = adw::Dialog::builder()
        .title("Dictation Shortcut")
        .content_width(420)
        .child(&toolbar)
        .build();

    // The recorded shortcut, and the modifiers held right now.
    let captured: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let held: Rc<RefCell<BTreeSet<&'static str>>> = Rc::new(RefCell::new(BTreeSet::new()));

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let captured = captured.clone();
        let held = held.clone();
        let dialog = dialog.clone();
        let set = set.clone();
        let preview = preview.clone();
        let message = message.clone();
        keys.connect_key_pressed(move |_, keyval, keycode, state| {
            // X11 and Wayland key codes are evdev codes + 8.
            let code = keycode.saturating_sub(8) as u16;
            if let Some(modifier) = modifier_name(code) {
                held.borrow_mut().insert(modifier);
            }
            // Modifiers held since before the dialog had the keyboard are
            // only in the event state. Right Alt (AltGr) also sets the Alt
            // state on some layouts; it is not Left Alt.
            let mut modifiers = held.borrow().clone();
            for (mask, name) in [
                (gdk::ModifierType::SUPER_MASK, "Super"),
                (gdk::ModifierType::META_MASK, "Super"),
                (gdk::ModifierType::CONTROL_MASK, "Ctrl"),
                (gdk::ModifierType::ALT_MASK, "Alt"),
                (gdk::ModifierType::SHIFT_MASK, "Shift"),
            ] {
                if state.contains(mask) && !(name == "Alt" && modifiers.contains("AltGr")) {
                    modifiers.insert(name);
                }
            }
            let ordered: Vec<&str> = MODIFIER_ORDER
                .iter()
                .copied()
                .filter(|m| modifiers.contains(m))
                .collect();

            if modifier_name(code).is_some() {
                // Only modifiers so far: show them, and wait for a key.
                preview.set_pending(&ordered);
                return glib::Propagation::Stop;
            }
            if ordered.is_empty() {
                match keyval {
                    gdk::Key::Escape => {
                        dialog.close();
                        return glib::Propagation::Stop;
                    }
                    // Enter sets what was pressed; Tab moves between the buttons.
                    gdk::Key::Return | gdk::Key::KP_Enter if set.is_sensitive() => {
                        set.emit_clicked();
                        return glib::Propagation::Stop;
                    }
                    gdk::Key::Tab => return glib::Propagation::Proceed,
                    _ => {}
                }
            }
            if keyval == gdk::Key::ISO_Left_Tab {
                return glib::Propagation::Proceed;
            }

            let Some(key) = key_name(code) else {
                message.set_label("That key cannot be used for the shortcut. Try another one.");
                return glib::Propagation::Stop;
            };
            let hotkey = ordered
                .iter()
                .map(|m| m.to_string())
                .chain(std::iter::once(key))
                .collect::<Vec<_>>()
                .join("+");
            preview.set(&hotkey);
            if parse_hotkey(&hotkey).is_err() {
                message.set_label("That key cannot be used for the shortcut. Try another one.");
                set.set_sensitive(false);
                *captured.borrow_mut() = None;
            } else if hotkey_blocks_typing(&hotkey) {
                message.set_label(&format!(
                    "{hotkey} would stop that key from typing. Add Ctrl, Alt or Super."
                ));
                set.set_sensitive(false);
                *captured.borrow_mut() = None;
            } else {
                message.set_label(&format!("Press Enter or Set Shortcut to use {hotkey}."));
                set.set_sensitive(true);
                *captured.borrow_mut() = Some(hotkey);
            }
            glib::Propagation::Stop
        });
    }
    {
        let held = held.clone();
        let preview = preview.clone();
        let captured = captured.clone();
        keys.connect_key_released(move |_, _, keycode, _| {
            if let Some(modifier) = modifier_name(keycode.saturating_sub(8) as u16) {
                held.borrow_mut().remove(modifier);
                // Modifiers let go without a key: back to what was there.
                if held.borrow().is_empty() {
                    preview.set(captured.borrow().as_deref().unwrap_or(&current_hotkey));
                }
            }
        });
    }
    dialog.add_controller(keys);

    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| {
            dialog.close();
        });
    }
    {
        let dialog = dialog.clone();
        let captured = captured.clone();
        set.connect_clicked(move |_| {
            if let Some(hotkey) = captured.borrow_mut().take() {
                on_set(hotkey);
            }
            dialog.close();
        });
    }

    // Let combinations the desktop would take (Super+…) reach the dialog.
    let toplevel = parent
        .as_ref()
        .root()
        .and_then(|root| root.surface())
        .and_then(|surface| surface.downcast::<gdk::Toplevel>().ok());
    if let Some(ref toplevel) = toplevel {
        toplevel.inhibit_system_shortcuts(None::<&gdk::Event>);
    }
    dialog.connect_closed(move |_| {
        if let Some(ref toplevel) = toplevel {
            toplevel.restore_system_shortcuts();
        }
        on_closed();
    });

    dialog.present(Some(parent));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_caps_are_capitalised() {
        assert_eq!(key_label("space"), "Space");
        assert_eq!(key_label("d"), "D");
        assert_eq!(key_label("F13"), "F13");
        assert_eq!(key_label("Super"), "Super");
    }
}
