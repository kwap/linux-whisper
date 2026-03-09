// SPDX-License-Identifier: GPL-3.0-or-later
//! Main application window for Linux Whisper.
//!
//! Builds the entire window layout using the builder pattern (no composite
//! templates or GObject subclassing). All widget fields are `pub` so that
//! service layers can connect additional signals after construction.

use adw::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::rc::Rc;

use linux_whisper_core::model::Segment;
use linux_whisper_i18n::{fl, LANGUAGE_LOADER};
use linux_whisper_whisper::model_registry;

use super::about;
use super::preferences;

/// Wrapper around the main `adw::ApplicationWindow` and all of its interactive
/// child widgets. Created once per application activation.
pub struct MainWindow {
    pub window: adw::ApplicationWindow,
    pub search_entry: gtk::SearchEntry,
    pub model_dropdown: gtk::DropDown,
    pub transcript_list: gtk::ListBox,
    pub record_button: gtk::Button,
    pub open_button: gtk::Button,
    pub export_button: gtk::MenuButton,
    pub copy_button: gtk::Button,
    pub clear_button: gtk::Button,
    pub status_label: gtk::Label,
    pub toast_overlay: adw::ToastOverlay,
}

impl MainWindow {
    /// Build and return a fully wired-up main window attached to `app`.
    pub fn new(app: &adw::Application) -> Self {
        // ── App menu actions ───────────────────────────────────────────
        let preferences_action = gio::SimpleAction::new("preferences", None);
        let about_action = gio::SimpleAction::new("about", None);
        let quit_action = gio::SimpleAction::new("quit", None);

        app.add_action(&preferences_action);
        app.add_action(&about_action);
        app.add_action(&quit_action);

        // ── Header bar ──────────────────────────────────────────────────
        let menu_model = gio::Menu::new();
        menu_model.append(
            Some(&fl!(LANGUAGE_LOADER, "preferences")),
            Some("app.preferences"),
        );
        menu_model.append(
            Some(&fl!(LANGUAGE_LOADER, "about")),
            Some("app.about"),
        );
        menu_model.append(
            Some(&fl!(LANGUAGE_LOADER, "quit")),
            Some("app.quit"),
        );

        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu_model)
            .build();

        let header = adw::HeaderBar::builder()
            .title_widget(&adw::WindowTitle::new(
                &fl!(LANGUAGE_LOADER, "app-name"),
                "",
            ))
            .build();
        header.pack_end(&menu_button);

        // ── Search row ──────────────────────────────────────────────────
        let search_entry = gtk::SearchEntry::builder()
            .placeholder_text(fl!(LANGUAGE_LOADER, "search-placeholder"))
            .hexpand(true)
            .build();

        // Model dropdown populated from the whisper model registry.
        let model_names: Vec<&str> = model_registry::all_models()
            .iter()
            .map(|m| m.name)
            .collect();
        let model_string_list = gtk::StringList::new(&model_names);
        let model_dropdown = gtk::DropDown::builder()
            .model(&model_string_list)
            .tooltip_text(fl!(LANGUAGE_LOADER, "model-select"))
            .build();

        let search_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(12)
            .margin_end(12)
            .build();
        search_row.append(&search_entry);
        search_row.append(&model_dropdown);

        // ── Transcript area ─────────────────────────────────────────────
        let transcript_list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(vec!["boxed-list"])
            .build();
        transcript_list.set_placeholder(Some(
            &gtk::Label::builder()
                .label(fl!(LANGUAGE_LOADER, "drop-files"))
                .css_classes(vec!["dim-label"])
                .margin_top(24)
                .margin_bottom(24)
                .build(),
        ));

        let scrolled_window = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .margin_start(12)
            .margin_end(12)
            .margin_top(6)
            .margin_bottom(6)
            .child(&transcript_list)
            .build();

        // ── Bottom action bar ───────────────────────────────────────────
        let record_button = gtk::Button::builder()
            .icon_name("media-record-symbolic")
            .label(fl!(LANGUAGE_LOADER, "record"))
            .css_classes(vec!["suggested-action"])
            .build();

        let open_button = gtk::Button::builder()
            .icon_name("document-open-symbolic")
            .label("Open File")
            .build();

        // Export popover with format choices.
        let export_popover = build_export_popover();
        let export_button = gtk::MenuButton::builder()
            .icon_name("document-save-symbolic")
            .label(fl!(LANGUAGE_LOADER, "export"))
            .popover(&export_popover)
            .build();

        let copy_button = gtk::Button::builder()
            .icon_name("edit-copy-symbolic")
            .label(fl!(LANGUAGE_LOADER, "copy-all"))
            .build();

        let clear_button = gtk::Button::builder()
            .icon_name("edit-clear-symbolic")
            .label(fl!(LANGUAGE_LOADER, "clear"))
            .css_classes(vec!["destructive-action"])
            .build();

        // Spacer pushes the right-hand buttons to the end.
        let spacer = gtk::Box::builder()
            .hexpand(true)
            .build();

        let bottom_bar = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .margin_top(6)
            .margin_bottom(8)
            .margin_start(12)
            .margin_end(12)
            .build();
        bottom_bar.append(&record_button);
        bottom_bar.append(&open_button);
        bottom_bar.append(&spacer);
        bottom_bar.append(&export_button);
        bottom_bar.append(&copy_button);
        bottom_bar.append(&clear_button);

        // ── Status label ────────────────────────────────────────────────
        let status_label = gtk::Label::builder()
            .label("Ready")
            .css_classes(vec!["dim-label", "caption"])
            .halign(gtk::Align::Start)
            .margin_start(12)
            .margin_bottom(4)
            .build();

        // ── Assemble content ────────────────────────────────────────────
        let content_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        content_box.append(&search_row);
        content_box.append(&scrolled_window);
        content_box.append(&bottom_bar);
        content_box.append(&status_label);

        // Toast overlay wraps all main content so toasts appear on top.
        let toast_overlay = adw::ToastOverlay::new();
        toast_overlay.set_child(Some(&content_box));

        // ToolbarView places the header bar at the top.
        let toolbar_view = adw::ToolbarView::builder()
            .content(&toast_overlay)
            .build();
        toolbar_view.add_top_bar(&header);

        // ── Window ──────────────────────────────────────────────────────
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(&fl!(LANGUAGE_LOADER, "app-name"))
            .default_width(900)
            .default_height(650)
            .content(&toolbar_view)
            .build();

        let main_window = Self {
            window,
            search_entry,
            model_dropdown,
            transcript_list,
            record_button,
            open_button,
            export_button,
            copy_button,
            clear_button,
            status_label,
            toast_overlay,
        };

        // ── Connect menu actions ──────────────────────────────────────
        {
            let win = main_window.window.clone();
            preferences_action.connect_activate(move |_, _| {
                preferences::show_preferences(&win);
            });
        }
        {
            let win = main_window.window.clone();
            about_action.connect_activate(move |_, _| {
                about::show_about(&win);
            });
        }
        {
            let app_clone = app.clone();
            quit_action.connect_activate(move |_, _| {
                app_clone.quit();
            });
        }

        // ── Connect widget signals ────────────────────────────────────
        main_window.connect_signals();

        main_window
    }

    // ── Public helpers ──────────────────────────────────────────────────

    /// Present (show) the window on screen.
    pub fn present(&self) {
        self.window.present();
    }

    /// Append a single segment row to the transcript list.
    ///
    /// Each segment is displayed as an `adw::ActionRow` with the transcribed
    /// text as the title and the formatted time range as the subtitle.
    pub fn add_segment_row(&self, segment: &Segment) {
        let subtitle = format!(
            "[{} - {}]",
            format_timestamp(segment.start),
            format_timestamp(segment.end),
        );
        let row = adw::ActionRow::builder()
            .title(&segment.text)
            .subtitle(&subtitle)
            .build();
        self.transcript_list.append(&row);
    }

    /// Remove every row from the transcript list.
    pub fn clear_segments(&self) {
        while let Some(row) = self.transcript_list.row_at_index(0) {
            self.transcript_list.remove(&row);
        }
    }

    /// Display a brief toast notification.
    pub fn show_toast(&self, message: &str) {
        let toast = adw::Toast::new(message);
        self.toast_overlay.add_toast(toast);
    }

    /// Update the record button to reflect whether a recording is in progress.
    ///
    /// * `recording == true` — label switches to "Stop", icon becomes
    ///   `media-playback-stop-symbolic`, and the button gets the
    ///   `destructive-action` style class.
    /// * `recording == false` — label reverts to "Record", icon becomes
    ///   `media-record-symbolic`, and the button gets the `suggested-action`
    ///   style class.
    pub fn set_recording_state(&self, recording: bool) {
        if recording {
            self.record_button
                .set_label(&fl!(LANGUAGE_LOADER, "stop"));
            self.record_button
                .set_icon_name("media-playback-stop-symbolic");
            self.record_button.remove_css_class("suggested-action");
            self.record_button.add_css_class("destructive-action");
        } else {
            self.record_button
                .set_label(&fl!(LANGUAGE_LOADER, "record"));
            self.record_button.set_icon_name("media-record-symbolic");
            self.record_button.remove_css_class("destructive-action");
            self.record_button.add_css_class("suggested-action");
        }
    }

    // ── Internal signal wiring ──────────────────────────────────────────

    fn connect_signals(&self) {
        // Recording state toggle stored in a local `RefCell` so the closure
        // can flip it on each click.
        let is_recording = Rc::new(RefCell::new(false));

        // Record button — toggle recording visual state.
        {
            let record_button = self.record_button.clone();
            let is_recording = Rc::clone(&is_recording);
            self.record_button.connect_clicked(move |_| {
                let mut recording = is_recording.borrow_mut();
                *recording = !*recording;
                let now_recording = *recording;
                // Apply visual state change.
                if now_recording {
                    record_button.set_label(&fl!(LANGUAGE_LOADER, "stop"));
                    record_button.set_icon_name("media-playback-stop-symbolic");
                    record_button.remove_css_class("suggested-action");
                    record_button.add_css_class("destructive-action");
                } else {
                    record_button.set_label(&fl!(LANGUAGE_LOADER, "record"));
                    record_button.set_icon_name("media-record-symbolic");
                    record_button.remove_css_class("destructive-action");
                    record_button.add_css_class("suggested-action");
                }
            });
        }

        // Clear button — remove all transcript rows.
        {
            let transcript_list = self.transcript_list.clone();
            self.clear_button.connect_clicked(move |_| {
                while let Some(row) = transcript_list.row_at_index(0) {
                    transcript_list.remove(&row);
                }
            });
        }

        // Open File button — launch file chooser for audio files.
        {
            let window = self.window.clone();
            let toast_overlay = self.toast_overlay.clone();
            self.open_button.connect_clicked(move |_| {
                let audio_filter = gtk::FileFilter::new();
                audio_filter.set_name(Some("Audio files"));
                audio_filter.add_mime_type("audio/*");
                gtk::FileFilter::add_suffix(&audio_filter, "wav");
                gtk::FileFilter::add_suffix(&audio_filter, "mp3");
                gtk::FileFilter::add_suffix(&audio_filter, "ogg");
                gtk::FileFilter::add_suffix(&audio_filter, "m4a");
                gtk::FileFilter::add_suffix(&audio_filter, "opus");
                gtk::FileFilter::add_suffix(&audio_filter, "flac");
                gtk::FileFilter::add_suffix(&audio_filter, "mp4");

                let filters = gio::ListStore::new::<gtk::FileFilter>();
                filters.append(&audio_filter);

                let dialog = gtk::FileDialog::builder()
                    .title("Open Audio File")
                    .filters(&filters)
                    .modal(true)
                    .build();

                let toast_overlay = toast_overlay.clone();
                dialog.open(Some(&window), gio::Cancellable::NONE, move |result| {
                    match result {
                        Ok(file) => {
                            if let Some(path) = file.path() {
                                let name = path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_else(|| "file".to_string());
                                let toast = adw::Toast::new(
                                    &format!("Opened: {name}"),
                                );
                                toast_overlay.add_toast(toast);
                            }
                        }
                        Err(_) => {
                            // User cancelled — do nothing.
                        }
                    }
                });
            });
        }

        // Copy All button — placeholder toast.
        {
            let toast_overlay = self.toast_overlay.clone();
            self.copy_button.connect_clicked(move |_| {
                let toast = adw::Toast::new("Copied to clipboard");
                toast_overlay.add_toast(toast);
            });
        }

        // Search entry — case-insensitive filter on transcript rows.
        {
            let transcript_list = self.transcript_list.clone();
            let search_entry = self.search_entry.clone();
            transcript_list.set_filter_func(glib::clone!(
                #[weak]
                search_entry,
                #[upgrade_or]
                true,
                move |row| {
                    let query = search_entry.text().to_string().to_lowercase();
                    if query.is_empty() {
                        return true;
                    }
                    // Try to downcast to adw::ActionRow to read its title.
                    if let Some(action_row) = row.downcast_ref::<adw::ActionRow>() {
                        let title = action_row.title().to_string().to_lowercase();
                        let subtitle = action_row
                            .subtitle()
                            .map(|s| s.to_string())
                            .unwrap_or_default()
                            .to_lowercase();
                        title.contains(&query) || subtitle.contains(&query)
                    } else {
                        true
                    }
                }
            ));

            self.search_entry.connect_search_changed(move |_entry| {
                transcript_list.invalidate_filter();
            });
        }
    }
}

// ── Export popover builder ───────────────────────────────────────────────

/// Build the export format selection popover shown when the user clicks the
/// Export menu button. Contains four rows for the supported formats.
fn build_export_popover() -> gtk::Popover {
    let formats = [
        ("Plain Text (.txt)", "TXT"),
        ("SubRip (.srt)", "SRT"),
        ("WebVTT (.vtt)", "VTT"),
        ("CSV (.csv)", "CSV"),
    ];

    let list_box = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(vec!["boxed-list"])
        .build();

    let popover = gtk::Popover::builder()
        .child(&list_box)
        .build();

    for (label, _format_tag) in &formats {
        let row = gtk::ListBoxRow::builder().build();
        let row_label = gtk::Label::builder()
            .label(*label)
            .halign(gtk::Align::Start)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(12)
            .margin_end(12)
            .build();
        row.set_child(Some(&row_label));
        list_box.append(&row);
    }

    // Connect row activation to dismiss the popover and show a toast.
    // We cannot show the toast here because we don't have a reference to the
    // overlay; instead we use the popover's parent chain at activation time.
    {
        let popover_weak = popover.downgrade();
        list_box.connect_row_activated(move |_list_box, row| {
            let index = row.index();
            let format_name = match index {
                0 => "TXT",
                1 => "SRT",
                2 => "VTT",
                3 => "CSV",
                _ => "Unknown",
            };

            // Dismiss the popover first.
            if let Some(popover) = popover_weak.upgrade() {
                popover.popdown();

                // Walk the widget tree to find the ToastOverlay and show a toast.
                let mut ancestor = popover.parent();
                while let Some(widget) = ancestor {
                    if let Some(overlay) = widget.downcast_ref::<adw::ToastOverlay>() {
                        let toast = adw::Toast::new(&format!("Exported as {format_name}"));
                        overlay.add_toast(toast);
                        return;
                    }
                    ancestor = widget.parent();
                }
            }
        });
    }

    popover
}

// ── Utility ─────────────────────────────────────────────────────────────

/// Format a time value in seconds to `MM:SS.t` where `t` is tenths of a second.
fn format_timestamp(seconds: f64) -> String {
    let total_tenths = (seconds * 10.0).round() as u64;
    let mins = total_tenths / 600;
    let secs = (total_tenths % 600) / 10;
    let tenths = total_tenths % 10;
    format!("{:02}:{:02}.{}", mins, secs, tenths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_timestamp_zero() {
        assert_eq!(format_timestamp(0.0), "00:00.0");
    }

    #[test]
    fn format_timestamp_simple() {
        assert_eq!(format_timestamp(3.2), "00:03.2");
    }

    #[test]
    fn format_timestamp_over_one_minute() {
        assert_eq!(format_timestamp(65.5), "01:05.5");
    }

    #[test]
    fn format_timestamp_exact_minute() {
        assert_eq!(format_timestamp(120.0), "02:00.0");
    }
}
