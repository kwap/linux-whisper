// SPDX-License-Identifier: GPL-3.0-or-later
//! Main application window for Linux Whisper.
//!
//! A welcome screen with a large record button until there is something to
//! show, then this session's transcripts as cards (newest first) under a
//! compact record bar. Built with the builder pattern (no composite
//! templates or GObject subclassing).

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use std::time::Duration;

use tracing::{error, info};

use linux_whisper_core::config::AppConfig;
use linux_whisper_core::export::ExportFormat;
use linux_whisper_core::model::{Segment, Transcript, TranscriptSource};
use linux_whisper_i18n::{fl, LANGUAGE_LOADER};
use linux_whisper_whisper::engine::TranscribeOptions;
use linux_whisper_whisper::model_manager::ModelManager;
use linux_whisper_whisper::model_registry::{self, WhisperModel};

use crate::context::{AppAction, AppContext};
use crate::services::dictation::DictationService;
use crate::services::models::{self, ModelEvent};
use crate::services::transcription::TranscriptionService;
use linux_whisper_platform::tray;

use super::preferences::{format_bytes, pretty_model_name};
use super::shortcut;

/// Audio file extensions offered in the Open dialog and accepted by drag and drop.
const AUDIO_EXTENSIONS: [&str; 7] = ["wav", "mp3", "ogg", "m4a", "opus", "flac", "mp4"];

const APP_ICON: &str = "com.linuxwhisper.LinuxWhisper";

/// What the app is doing, as shown around the record buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    Idle,
    Recording,
    Transcribing,
}

/// A transcript in the window, with the text that was delivered for it.
struct Entry {
    transcript: Transcript,
    text: String,
    card: gtk::Widget,
}

/// Wrapper around the main `adw::ApplicationWindow` and its interactive
/// widgets. Created once and hidden/shown afterwards.
#[derive(Clone)]
pub struct MainWindow {
    pub window: adw::ApplicationWindow,
    toast_overlay: adw::ToastOverlay,
    search_bar: gtk::SearchBar,
    search_entry: gtk::SearchEntry,
    model_dropdown: gtk::DropDown,
    open_button: gtk::Button,
    stack: gtk::Stack,
    /// Record controls on the welcome page, and above the transcripts.
    hero: RecordPanel,
    bar: RecordPanel,
    cards: gtk::Box,
    ctx: AppContext,
    entries: Rc<RefCell<Vec<Entry>>>,
    activity: Rc<Cell<Activity>>,
    /// Status shown under the title while idle or recording.
    status: Rc<RefCell<String>>,
    /// Set while the model dropdown is changed from code.
    syncing_dropdown: Rc<Cell<bool>>,
    /// Whether the first-run model prompt is open.
    prompt_open: Rc<Cell<bool>>,
    /// Whether a file is being transcribed.
    file_busy: Rc<Cell<bool>>,
}

/// The pigeon from the tray, as it looks idle and while recording (its eye
/// glows red).
#[derive(Clone)]
struct Pigeons {
    idle: Option<gdk::Texture>,
    recording: Option<gdk::Texture>,
}

impl Pigeons {
    /// Rendered large enough for the welcome page on a HiDPI screen.
    const SIZE: u32 = 320;

    fn new() -> Self {
        let texture = |recording| {
            let rgba = tray::pigeon_rgba(recording, Self::SIZE)?;
            let size = Self::SIZE as i32;
            let texture = gdk::MemoryTexture::new(
                size,
                size,
                gdk::MemoryFormat::R8g8b8a8Premultiplied,
                &glib::Bytes::from_owned(rgba),
                Self::SIZE as usize * 4,
            );
            Some(texture.upcast::<gdk::Texture>())
        };
        Self {
            idle: texture(false),
            recording: texture(true),
        }
    }
}

/// The record button (the pigeon) and the text around it.
#[derive(Clone)]
struct RecordPanel {
    button: gtk::Button,
    pigeon: gtk::Image,
    pigeons: Pigeons,
    title: gtk::Label,
    subtitle: gtk::Label,
    shortcut: shortcut::Keycaps,
}

impl RecordPanel {
    fn new(large: bool, pigeons: &Pigeons) -> Self {
        let (align, justify, xalign) = if large {
            (gtk::Align::Center, gtk::Justification::Center, 0.5)
        } else {
            (gtk::Align::End, gtk::Justification::Left, 0.0)
        };
        let pigeon = gtk::Image::builder()
            .pixel_size(if large { 136 } else { 44 })
            .build();
        pigeon.set_paintable(pigeons.idle.as_ref());
        let button = gtk::Button::builder()
            .child(&pigeon)
            .tooltip_text("Start dictation")
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .css_classes(["flat", "pigeon-button"])
            .build();
        let title = gtk::Label::builder()
            .css_classes([if large { "title-1" } else { "heading" }])
            .wrap(true)
            .justify(justify)
            .xalign(xalign)
            .build();
        let subtitle = gtk::Label::builder()
            .css_classes(if large {
                ["dim-label", "hero-subtitle"]
            } else {
                ["dim-label", "caption"]
            })
            .wrap(true)
            .justify(justify)
            .xalign(xalign)
            .max_width_chars(if large { 46 } else { -1 })
            .build();
        let shortcut = shortcut::Keycaps::new(large);
        shortcut.widget.set_halign(align);
        Self {
            button,
            pigeon,
            pigeons: pigeons.clone(),
            title,
            subtitle,
            shortcut,
        }
    }

    fn show(&self, activity: Activity, status: &str, hotkey: &str) {
        // While recording the pigeon's eye glows red, as in the tray.
        let recording = activity == Activity::Recording;
        self.pigeon.set_paintable(if recording {
            self.pigeons.recording.as_ref()
        } else {
            self.pigeons.idle.as_ref()
        });
        self.button.set_tooltip_text(Some(if recording {
            "Stop and transcribe"
        } else {
            "Start dictation"
        }));
        match activity {
            Activity::Idle => {
                self.title.set_label("Ready to dictate");
                self.subtitle.set_label(status);
            }
            Activity::Recording => {
                self.title.set_label("Listening\u{2026}");
                self.subtitle.set_label(&format!(
                    "{status}. Press the shortcut again, or the pigeon, to finish."
                ));
            }
            Activity::Transcribing => {
                self.title.set_label("Transcribing\u{2026}");
                self.subtitle.set_label("Turning your speech into text");
            }
        }
        self.shortcut.set(hotkey);
    }
}

impl MainWindow {
    /// Build and return a fully wired-up main window attached to `app`.
    pub fn new(ctx: &AppContext) -> Self {
        let app = &ctx.app;

        // ── App menu actions ───────────────────────────────────────────
        for (name, action) in [
            ("preferences", AppAction::Preferences),
            ("about", AppAction::About),
            ("quit", AppAction::Quit),
        ] {
            let simple = gio::SimpleAction::new(name, None);
            let ctx = ctx.clone();
            simple.connect_activate(move |_, _| ctx.send(action));
            app.add_action(&simple);
        }
        app.set_accels_for_action("app.preferences", &["<Control>comma"]);
        app.set_accels_for_action("app.quit", &["<Control>q"]);
        app.set_accels_for_action("win.open", &["<Control>o"]);
        app.set_accels_for_action("win.search", &["<Control>f"]);

        // ── Header bar ──────────────────────────────────────────────────
        let menu_model = gio::Menu::new();
        let section = gio::Menu::new();
        section.append(Some("Clear History"), Some("win.clear"));
        menu_model.append_section(None, &section);
        let section = gio::Menu::new();
        section.append(
            Some(&fl!(LANGUAGE_LOADER, "preferences")),
            Some("app.preferences"),
        );
        section.append(Some("About Linux Whisper"), Some("app.about"));
        section.append(Some(&fl!(LANGUAGE_LOADER, "quit")), Some("app.quit"));
        menu_model.append_section(None, &section);

        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu_model)
            .tooltip_text("Main menu")
            .primary(true)
            .build();

        let search_button = gtk::ToggleButton::builder()
            .icon_name("system-search-symbolic")
            .tooltip_text("Search transcripts")
            .build();

        let open_button = gtk::Button::builder()
            .icon_name("document-open-symbolic")
            .tooltip_text("Transcribe an audio file")
            .action_name("win.open")
            .build();

        // Model dropdown populated from the whisper model registry.
        let model_names: Vec<String> = model_registry::all_models()
            .iter()
            .map(|m| pretty_model_name(m.name))
            .collect();
        let model_name_refs: Vec<&str> = model_names.iter().map(String::as_str).collect();
        let model_dropdown = gtk::DropDown::builder()
            .model(&gtk::StringList::new(&model_name_refs))
            .tooltip_text(fl!(LANGUAGE_LOADER, "model-select"))
            .css_classes(["flat"])
            .build();

        let header = adw::HeaderBar::builder()
            .title_widget(&adw::WindowTitle::new(
                &fl!(LANGUAGE_LOADER, "app-name"),
                "",
            ))
            .build();
        header.pack_start(&open_button);
        header.pack_end(&menu_button);
        header.pack_end(&search_button);
        header.pack_end(&model_dropdown);

        // ── Search ──────────────────────────────────────────────────────
        let search_entry = gtk::SearchEntry::builder()
            .placeholder_text(fl!(LANGUAGE_LOADER, "search-placeholder"))
            .hexpand(true)
            .build();
        let search_bar = gtk::SearchBar::builder()
            .child(
                &adw::Clamp::builder()
                    .maximum_size(720)
                    .child(&search_entry)
                    .build(),
            )
            .build();
        search_bar.connect_entry(&search_entry);
        search_bar
            .bind_property("search-mode-enabled", &search_button, "active")
            .bidirectional()
            .sync_create()
            .build();

        // ── Welcome page ────────────────────────────────────────────────
        let pigeons = Pigeons::new();
        let hero = RecordPanel::new(true, &pigeons);
        let shortcut_caption = gtk::Label::builder()
            .label("Press this in any app to start dictating, and again to stop. Your words are typed where the cursor is.")
            .max_width_chars(52)
            .css_classes(["dim-label", "caption"])
            .wrap(true)
            .justify(gtk::Justification::Center)
            .margin_top(10)
            .build();
        let open_link = gtk::Button::builder()
            .label("Transcribe an Audio File\u{2026}")
            .action_name("win.open")
            .halign(gtk::Align::Center)
            .css_classes(["pill"])
            .margin_top(6)
            .build();
        let drop_hint = gtk::Label::builder()
            .label("or drop one onto this window")
            .css_classes(["dim-label", "caption"])
            .build();

        let welcome_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .valign(gtk::Align::Center)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(24)
            .margin_end(24)
            .build();
        welcome_box.append(&hero.button);
        welcome_box.append(&hero.title);
        welcome_box.append(&hero.subtitle);
        welcome_box.append(&shortcut_caption);
        welcome_box.append(&hero.shortcut.widget);
        let spacer = gtk::Box::builder().height_request(18).build();
        welcome_box.append(&spacer);
        welcome_box.append(&open_link);
        welcome_box.append(&drop_hint);

        let welcome = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(
                &adw::Clamp::builder()
                    .maximum_size(520)
                    .child(&welcome_box)
                    .build(),
            )
            .build();

        // ── Transcripts page ────────────────────────────────────────────
        let bar = RecordPanel::new(false, &pigeons);
        let bar_text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        bar_text.append(&bar.title);
        bar_text.append(&bar.subtitle);
        let bar_box = gtk::Box::builder()
            .spacing(14)
            .css_classes(["card", "record-bar"])
            .build();
        bar_box.append(&bar.button);
        bar_box.append(&bar_text);
        bar_box.append(&bar.shortcut.widget);

        let cards = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .build();
        let no_results = gtk::Label::builder()
            .label("No transcript matches the search")
            .css_classes(["dim-label"])
            .margin_top(24)
            .visible(false)
            .build();

        let history_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .margin_top(18)
            .margin_bottom(24)
            .margin_start(16)
            .margin_end(16)
            .build();
        history_box.append(&bar_box);
        history_box.append(&cards);
        history_box.append(&no_results);

        let history = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(
                &adw::Clamp::builder()
                    .maximum_size(760)
                    .child(&history_box)
                    .build(),
            )
            .build();

        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        stack.add_named(&welcome, Some("welcome"));
        stack.add_named(&history, Some("history"));

        // Toast overlay wraps all main content so toasts appear on top.
        let toast_overlay = adw::ToastOverlay::new();
        toast_overlay.set_child(Some(&stack));

        let toolbar_view = adw::ToolbarView::builder().content(&toast_overlay).build();
        toolbar_view.add_top_bar(&header);
        toolbar_view.add_top_bar(&search_bar);

        // ── Window ──────────────────────────────────────────────────────
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(fl!(LANGUAGE_LOADER, "app-name"))
            .icon_name(APP_ICON)
            .default_width(760)
            .default_height(720)
            .width_request(360)
            .height_request(480)
            .content(&toolbar_view)
            .build();
        search_bar.set_key_capture_widget(Some(&window));

        let main_window = Self {
            window,
            toast_overlay,
            search_bar,
            search_entry,
            model_dropdown,
            open_button,
            stack,
            hero,
            bar,
            cards,
            ctx: ctx.clone(),
            entries: Rc::new(RefCell::new(Vec::new())),
            activity: Rc::new(Cell::new(Activity::Idle)),
            status: Rc::new(RefCell::new(String::new())),
            syncing_dropdown: Rc::new(Cell::new(false)),
            prompt_open: Rc::new(Cell::new(false)),
            file_busy: Rc::new(Cell::new(false)),
        };

        main_window.sync_model_dropdown();
        main_window.set_status(&ready_hint(ctx));
        main_window.connect_signals(&no_results);
        main_window
    }

    // ── Public helpers ──────────────────────────────────────────────────

    /// Present (show) the window on screen.
    pub fn present(&self) {
        self.window.present();
    }

    /// Add a transcript (shown first). `text` is what was delivered for it
    /// (formatted, possibly by the AI formatter); its Copy button copies it.
    pub fn add_transcript(&self, transcript: Transcript, text: String) {
        let card = self.transcript_card(&transcript, &text);
        self.cards.prepend(&card);
        self.entries.borrow_mut().push(Entry {
            transcript,
            text,
            card: card.upcast(),
        });
        self.stack.set_visible_child_name("history");
        self.apply_search();
    }

    /// Rebuild the cards (e.g. after the confidence setting changed).
    pub fn refresh_rows(&self) {
        let mut entries = self.entries.borrow_mut();
        for entry in entries.iter_mut() {
            let card = self.transcript_card(&entry.transcript, &entry.text);
            self.cards.insert_child_after(&card, Some(&entry.card));
            self.cards.remove(&entry.card);
            entry.card = card.upcast();
        }
    }

    /// Display a brief toast notification.
    pub fn show_toast(&self, message: &str) {
        let toast = adw::Toast::new(message);
        self.toast_overlay.add_toast(toast);
    }

    /// Show what the app is doing around the record buttons.
    pub fn set_activity(&self, activity: Activity) {
        self.activity.set(activity);
        self.refresh_panels();
    }

    /// The status line (e.g. "The Base model is ready…", "Recording from …").
    pub fn set_status(&self, status: &str) {
        *self.status.borrow_mut() = status.to_string();
        self.refresh_panels();
    }

    /// The idle status line for the current model.
    pub fn set_ready_status(&self) {
        self.set_status(&ready_hint(&self.ctx));
    }

    /// Update the recording time.
    pub fn update_time(&self, secs: f64) {
        let title = format!(
            "Listening\u{2026} {}:{:02}",
            secs as u64 / 60,
            secs as u64 % 60
        );
        for panel in [&self.hero, &self.bar] {
            panel.title.set_label(&title);
        }
    }

    /// The hotkey changed: show the new one.
    pub fn refresh_hotkey(&self) {
        self.refresh_panels();
    }

    fn refresh_panels(&self) {
        let hotkey = AppConfig::load().hotkey;
        let status = self.status.borrow();
        for panel in [&self.hero, &self.bar] {
            panel.show(self.activity.get(), &status, &hotkey);
        }
    }

    /// Select the configured model in the dropdown without switching models.
    pub fn sync_model_dropdown(&self) {
        let config = AppConfig::load();
        if let Some(index) = model_registry::all_models()
            .iter()
            .position(|m| m.name == config.model)
        {
            self.syncing_dropdown.set(true);
            self.model_dropdown.set_selected(index as u32);
            self.syncing_dropdown.set(false);
        }
    }

    /// First run: ask to download the default model.
    pub fn prompt_model_download(&self) {
        if models::model_download_in_progress() || self.prompt_open.replace(true) {
            return;
        }
        let model = model_registry::default_model();
        let name = pretty_model_name(model.name);
        let dialog = adw::AlertDialog::new(
            Some("Download a speech model"),
            Some(&format!(
                "Linux Whisper turns speech into text with a Whisper model that runs \
                 on this computer. The {name} model ({}) is a good start. Larger \
                 models, available at the top right, are more accurate but slower.",
                format_bytes(model.size_bytes)
            )),
        );
        dialog.add_responses(&[
            ("later", "Later"),
            ("download", &format!("Download {name}")),
        ]);
        dialog.set_response_appearance("download", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("download"));
        dialog.set_close_response("later");

        let this = self.clone();
        dialog.connect_response(None, move |_, response| {
            this.prompt_open.set(false);
            if response == "download" {
                this.switch_model(model);
            } else {
                this.set_status("No speech model yet \u{2014} choose one at the top right");
            }
        });
        dialog.present(Some(&self.window));
    }

    /// Download (if needed) and load `model`, showing progress in the window.
    fn switch_model(&self, model: &'static WhisperModel) {
        let name = pretty_model_name(model.name);
        self.model_dropdown.set_sensitive(false);
        let this = self.clone();
        models::activate_model(&self.ctx, model, move |event| {
            let what = format!("the {name} model");
            this.set_status(&models::describe(&event, &what));
            if event.is_final() {
                this.model_dropdown.set_sensitive(true);
            }
            match event {
                ModelEvent::Ready => {
                    this.set_ready_status();
                    this.show_toast(&format!("{name} model ready"));
                }
                ModelEvent::Failed(ref e) => this.show_toast(e),
                _ => {}
            }
        });
    }

    // ── Transcript cards ────────────────────────────────────────────────

    fn transcript_card(&self, transcript: &Transcript, text: &str) -> gtk::Box {
        let show_confidence = AppConfig::load().show_confidence;
        let is_file = matches!(transcript.source, TranscriptSource::File { .. });

        let icon = gtk::Image::builder()
            .icon_name(if is_file {
                "audio-x-generic-symbolic"
            } else {
                "audio-input-microphone-symbolic"
            })
            .valign(gtk::Align::Center)
            .css_classes(["card-icon"])
            .build();

        let title = gtk::Label::builder()
            .label(if is_file {
                transcript.title.as_str()
            } else {
                "Dictation"
            })
            .css_classes(["heading"])
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .build();
        let created = glib::DateTime::from_unix_local(transcript.created_at.timestamp())
            .and_then(|t| t.format("%H:%M"))
            .map(|t| t.to_string())
            .unwrap_or_default();
        let meta = [
            created,
            format_duration(transcript.duration),
            pretty_model_name(&transcript.model_name),
        ]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" \u{00b7} ");
        let subtitle = gtk::Label::builder()
            .label(meta)
            .css_classes(["caption", "dim-label"])
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let heading = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        heading.append(&title);
        heading.append(&subtitle);

        // The languages spoken, in order.
        let languages = gtk::Box::builder()
            .spacing(4)
            .valign(gtk::Align::Center)
            .build();
        let mut seen: Vec<&str> = Vec::new();
        for lang in transcript
            .segments
            .iter()
            .filter_map(|s| s.language.as_deref())
            .chain(transcript.language.as_deref())
        {
            if !seen.contains(&lang) {
                seen.push(lang);
                languages.append(
                    &gtk::Label::builder()
                        .label(lang.to_uppercase())
                        .tooltip_text(language_name(lang))
                        .css_classes(["lang-pill"])
                        .build(),
                );
            }
        }

        let id = transcript.id.to_string();
        let copy_button = gtk::Button::builder()
            .icon_name("edit-copy-symbolic")
            .tooltip_text("Copy")
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular"])
            .build();
        {
            let this = self.clone();
            let text = text.to_string();
            copy_button.connect_clicked(move |_| this.copy_text(&text));
        }

        let export_menu = gio::Menu::new();
        for (label, fmt, _) in EXPORT_FORMATS {
            export_menu.append(Some(label), Some(&format!("win.export::{fmt}:{id}")));
        }
        let export_button = gtk::MenuButton::builder()
            .icon_name("document-save-symbolic")
            .tooltip_text(fl!(LANGUAGE_LOADER, "export"))
            .valign(gtk::Align::Center)
            .menu_model(&export_menu)
            .css_classes(["flat", "circular"])
            .build();

        let remove_button = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Remove from the list")
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular"])
            .build();
        {
            let this = self.clone();
            remove_button.connect_clicked(move |_| this.remove_transcript(&id));
        }

        let header = gtk::Box::builder().spacing(12).build();
        header.append(&icon);
        header.append(&heading);
        header.append(&languages);
        header.append(&copy_button);
        header.append(&export_button);
        header.append(&remove_button);

        let body = gtk::Label::builder()
            .label(text)
            .use_markup(false)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .selectable(true)
            .xalign(0.0)
            .css_classes(["transcript-text"])
            .build();

        let card = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .css_classes(["card", "transcript-card"])
            .build();
        card.append(&header);
        card.append(&body);

        // Timings (and confidence) per segment, folded away.
        if transcript.segments.len() > 1 || show_confidence {
            let rows = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(6)
                .margin_top(8)
                .build();
            for segment in &transcript.segments {
                rows.append(&segment_row(segment, show_confidence));
            }
            let count = transcript.segments.len();
            let expander = gtk::Expander::builder()
                .label(format!(
                    "{count} segment{} with timings",
                    if count == 1 { "" } else { "s" }
                ))
                .child(&rows)
                .css_classes(["segments"])
                .build();
            card.append(&expander);
        }
        card
    }

    fn remove_transcript(&self, id: &str) {
        let mut entries = self.entries.borrow_mut();
        if let Some(pos) = entries
            .iter()
            .position(|e| e.transcript.id.to_string() == id)
        {
            let entry = entries.remove(pos);
            self.cards.remove(&entry.card);
        }
        if entries.is_empty() {
            self.stack.set_visible_child_name("welcome");
        }
    }

    fn copy_text(&self, text: &str) {
        if text.is_empty() {
            self.show_toast("Nothing to copy");
            return;
        }
        match DictationService::copy_to_clipboard(text) {
            Ok(()) => self.show_toast("Copied to clipboard"),
            Err(e) => {
                error!("Clipboard copy failed: {e}");
                self.show_toast("Failed to copy to clipboard");
            }
        }
    }

    /// Show only the cards whose text matches the search.
    fn apply_search(&self) {
        let query = self.search_entry.text().to_lowercase();
        for entry in self.entries.borrow().iter() {
            let matches = query.is_empty()
                || entry.text.to_lowercase().contains(&query)
                || entry.transcript.title.to_lowercase().contains(&query);
            entry.card.set_visible(matches);
        }
    }

    // ── Internal signal wiring ──────────────────────────────────────────

    fn connect_signals(&self, no_results: &gtk::Label) {
        // Record buttons — same as the hotkey and the tray icon.
        for panel in [&self.hero, &self.bar] {
            let ctx = self.ctx.clone();
            panel
                .button
                .connect_clicked(move |_| ctx.send(AppAction::ToggleRecording));
        }

        // Model dropdown — switch to the chosen model.
        {
            let this = self.clone();
            self.model_dropdown
                .connect_selected_notify(move |dropdown| {
                    if this.syncing_dropdown.get() {
                        return;
                    }
                    if let Some(model) =
                        model_registry::all_models().get(dropdown.selected() as usize)
                    {
                        if this.ctx.worker.loaded_model().as_deref() != Some(model.name)
                            || AppConfig::load().model != model.name
                        {
                            this.switch_model(model);
                        }
                    }
                });
        }

        // Window actions: open a file, search, clear the list.
        let open = gio::SimpleAction::new("open", None);
        {
            let this = self.clone();
            open.connect_activate(move |_, _| this.choose_file());
        }
        self.window.add_action(&open);

        let search = gio::SimpleAction::new("search", None);
        {
            let search_bar = self.search_bar.clone();
            search.connect_activate(move |_, _| search_bar.set_search_mode(true));
        }
        self.window.add_action(&search);

        let clear = gio::SimpleAction::new("clear", None);
        {
            let this = self.clone();
            clear.connect_activate(move |_, _| {
                for entry in this.entries.borrow_mut().drain(..) {
                    this.cards.remove(&entry.card);
                }
                this.stack.set_visible_child_name("welcome");
            });
        }
        self.window.add_action(&clear);

        // Drag and drop an audio file onto the window.
        {
            let drop_target = gtk::DropTarget::new(gio::File::static_type(), gdk::DragAction::COPY);
            let this = self.clone();
            drop_target.connect_drop(move |_, value, _, _| {
                match value.get::<gio::File>().ok().and_then(|f| f.path()) {
                    Some(path) => {
                        this.transcribe_file(path);
                        true
                    }
                    None => false,
                }
            });
            self.toast_overlay.add_controller(drop_target);
        }

        // Export menu items.
        connect_export_action(self);

        // Search — case-insensitive filter on the cards.
        {
            let this = self.clone();
            let no_results = no_results.clone();
            self.search_entry.connect_search_changed(move |_| {
                this.apply_search();
                let entries = this.entries.borrow();
                let none_shown = entries.iter().all(|e| !e.card.is_visible());
                no_results.set_visible(none_shown && !entries.is_empty());
            });
        }

        // Closing the window keeps the app running in the background (so the
        // hotkey keeps working) unless the user turned that off.
        {
            let this = self.clone();
            let hint_shown = Cell::new(false);
            self.window.connect_close_request(move |window| {
                if !AppConfig::load().minimize_to_tray {
                    this.ctx.send(AppAction::Quit);
                    return glib::Propagation::Stop;
                }
                window.set_visible(false);
                if !crate::tray_host_running() && !hint_shown.replace(true) {
                    let notification = gio::Notification::new("Linux Whisper is still running");
                    notification.set_body(Some(&format!(
                        "Press {} to dictate. Open Linux Whisper again from your \
                         applications to see this window.",
                        AppConfig::load().hotkey
                    )));
                    this.ctx
                        .app
                        .send_notification(Some("background"), &notification);
                }
                glib::Propagation::Stop
            });
        }
    }

    /// Ask for an audio file, then transcribe it.
    fn choose_file(&self) {
        let audio_filter = gtk::FileFilter::new();
        audio_filter.set_name(Some("Audio files"));
        audio_filter.add_mime_type("audio/*");
        for ext in AUDIO_EXTENSIONS {
            audio_filter.add_suffix(ext);
        }

        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&audio_filter);

        let dialog = gtk::FileDialog::builder()
            .title("Open Audio File")
            .filters(&filters)
            .modal(true)
            .build();

        let this = self.clone();
        dialog.open(
            Some(&self.window.clone()),
            gio::Cancellable::NONE,
            move |result| {
                // An error means the user cancelled.
                if let Some(path) = result.ok().and_then(|file| file.path()) {
                    this.transcribe_file(path);
                }
            },
        );
    }

    /// Decode and transcribe an audio file, then show its transcript.
    fn transcribe_file(&self, path: PathBuf) {
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());

        let supported = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| AUDIO_EXTENSIONS.contains(&e.to_lowercase().as_str()));
        if !supported {
            self.show_toast(&format!(
                "{file_name} is not a supported audio file ({})",
                AUDIO_EXTENSIONS.join(", ").to_uppercase()
            ));
            return;
        }

        if self.file_busy.get() {
            self.show_toast("Another file is being transcribed \u{2014} wait for it to finish");
            return;
        }

        if self.ctx.worker.loaded_model().is_none() {
            let config = AppConfig::load();
            let on_disk = model_registry::find_model(&config.model)
                .is_some_and(|m| ModelManager::new(AppConfig::models_dir()).is_downloaded(m));
            if !on_disk {
                if models::model_download_in_progress() {
                    self.show_toast("The speech model is still downloading");
                } else {
                    self.prompt_model_download();
                }
                return;
            }
        }

        let progress = adw::Toast::builder()
            .title(format!("Transcribing {file_name}\u{2026}"))
            .timeout(0)
            .build();
        self.toast_overlay.add_toast(progress.clone());
        self.open_button.set_sensitive(false);
        self.file_busy.set(true);

        let svc = TranscriptionService::new(Arc::new(self.ctx.worker.clone()));
        let config = AppConfig::load();
        let options = TranscribeOptions::from_config(&config);

        let (result_tx, result_rx) = std_mpsc::channel::<Result<Transcript, String>>();
        let path_for_task = path.clone();
        self.ctx.rt.spawn(async move {
            let result = svc.transcribe_file(&path_for_task, options).await;
            let _ = result_tx.send(result.map_err(|e| e.to_string()));
        });

        let this = self.clone();
        glib::timeout_add_local(Duration::from_millis(100), move || {
            let result = match result_rx.try_recv() {
                Ok(result) => result,
                Err(std_mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Err(std_mpsc::TryRecvError::Disconnected) => {
                    Err("transcription stopped unexpectedly".to_string())
                }
            };
            this.open_button.set_sensitive(true);
            this.file_busy.set(false);
            progress.dismiss();
            match result {
                Ok(mut transcript) => {
                    transcript.title = file_name.clone();
                    transcript.source = TranscriptSource::File {
                        path: path.display().to_string(),
                    };
                    info!(
                        "{file_name}: {} segment(s), {:.1}s, language {:?}",
                        transcript.segment_count(),
                        transcript.duration,
                        transcript.language
                    );
                    let text = transcript.formatted_text(&config.format);
                    this.add_transcript(transcript, text);
                    this.show_toast(&format!("Transcribed: {file_name}"));
                }
                Err(e) => {
                    error!("File transcription failed: {e}");
                    this.show_toast(&format!("Error: {e}"));
                }
            }
            glib::ControlFlow::Break
        });
    }
}

/// One segment with its timing. Text is shown as-is (not as Pango markup), so
/// "&" and "<" in what was said cannot break the row.
fn segment_row(segment: &Segment, show_confidence: bool) -> gtk::Box {
    let row = gtk::Box::builder().spacing(12).build();
    row.append(
        &gtk::Label::builder()
            .label(format!(
                "{} \u{2013} {}",
                format_timestamp(segment.start),
                format_timestamp(segment.end)
            ))
            .css_classes(["timestamp", "dim-label", "numeric"])
            .valign(gtk::Align::Start)
            .build(),
    );
    row.append(
        &gtk::Label::builder()
            .label(&segment.text)
            .use_markup(false)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .selectable(true)
            .xalign(0.0)
            .hexpand(true)
            .build(),
    );
    if show_confidence {
        if let Some(confidence) = segment.confidence {
            row.append(
                &gtk::Label::builder()
                    .label(format!("{:.0}%", confidence * 100.0))
                    .tooltip_text("How sure the model was")
                    .css_classes(["caption", "dim-label", "numeric"])
                    .valign(gtk::Align::Start)
                    .build(),
            );
        }
    }
    row
}

/// The idle status line: which model is ready, or what is missing.
fn ready_hint(ctx: &AppContext) -> String {
    match ctx.worker.loaded_model() {
        Some(model) => {
            let languages = AppConfig::load().languages;
            let names: Vec<&str> = languages
                .iter()
                .filter_map(|code| linux_whisper_core::language::Language::from_code(code))
                .map(|l| l.name())
                .collect();
            format!(
                "{} model \u{00b7} {}",
                pretty_model_name(&model),
                match AppConfig::load().language.as_str() {
                    "auto" if names.is_empty() => "any language".to_string(),
                    "auto" => names.join(", "),
                    code => language_name(code),
                }
            )
        }
        None if models::model_download_in_progress() => {
            "Getting the speech model ready\u{2026}".to_string()
        }
        None => "Loading the speech model\u{2026}".to_string(),
    }
}

/// English name of a language code, for tooltips.
fn language_name(code: &str) -> String {
    linux_whisper_core::language::Language::from_code(code)
        .map(|l| l.name().to_string())
        .unwrap_or_else(|| code.to_string())
}

/// "4.2 s", "2 min 05 s".
fn format_duration(seconds: f64) -> String {
    if seconds < 60.0 {
        format!("{seconds:.1} s")
    } else {
        let total = seconds.round() as u64;
        format!("{} min {:02} s", total / 60, total % 60)
    }
}

// ── Export ───────────────────────────────────────────────────────────────

/// Export formats: menu label, action target, format.
const EXPORT_FORMATS: [(&str, &str, ExportFormat); 4] = [
    ("Plain Text (.txt)", "txt", ExportFormat::Txt),
    ("SubRip Subtitles (.srt)", "srt", ExportFormat::Srt),
    ("WebVTT Subtitles (.vtt)", "vtt", ExportFormat::Vtt),
    ("Spreadsheet (.csv)", "csv", ExportFormat::Csv),
];

/// Add the window's `export` action: export a transcript via a save dialog.
/// The target is "<format>:<transcript id>", or just "<format>" for the
/// newest transcript.
fn connect_export_action(win: &MainWindow) {
    let action = gio::SimpleAction::new("export", Some(glib::VariantTy::STRING));
    let this = win.clone();
    action.connect_activate(move |_, target| {
        let Some(target) = target.and_then(|t| t.get::<String>()) else {
            return;
        };
        let (ext, id) = target.split_once(':').unwrap_or((target.as_str(), ""));
        let Some(&(_, ext, format)) = EXPORT_FORMATS.iter().find(|(_, e, _)| *e == ext) else {
            return;
        };

        let transcript = this
            .entries
            .borrow()
            .iter()
            .rev()
            .find(|e| id.is_empty() || e.transcript.id.to_string() == id)
            .map(|e| e.transcript.clone());
        let Some(transcript) = transcript else {
            this.show_toast("No transcript to export");
            return;
        };

        let content = match TranscriptionService::export_transcript(&transcript, format) {
            Ok(c) => c,
            Err(e) => {
                error!("Export failed: {e}");
                this.show_toast(&format!("Export failed: {e}"));
                return;
            }
        };

        let file_filter = gtk::FileFilter::new();
        file_filter.set_name(Some(&ext.to_uppercase()));
        file_filter.add_suffix(ext);
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&file_filter);

        let stem = Path::new(&transcript.title)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .filter(|s| !s.is_empty() && transcript.source != TranscriptSource::Dictation)
            .unwrap_or_else(|| "transcript".to_string());
        let dialog = gtk::FileDialog::builder()
            .title(format!("Export as {}", ext.to_uppercase()))
            .initial_name(format!("{stem}.{ext}"))
            .filters(&filters)
            .modal(true)
            .build();

        let this = this.clone();
        dialog.save(
            Some(&this.window.clone()),
            gio::Cancellable::NONE,
            move |result| {
                // An error means the user cancelled.
                let Some(path) = result.ok().and_then(|file| file.path()) else {
                    return;
                };
                match TranscriptionService::save_export(&content, &path) {
                    Ok(()) => {
                        info!("Exported to {}", path.display());
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        this.show_toast(&format!("Exported: {name}"));
                    }
                    Err(e) => {
                        error!("Save failed: {e}");
                        this.show_toast(&format!("Save failed: {e}"));
                    }
                }
            },
        );
    });
    win.window.add_action(&action);
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

    #[test]
    fn format_duration_short_and_long() {
        assert_eq!(format_duration(4.24), "4.2 s");
        assert_eq!(format_duration(125.0), "2 min 05 s");
    }

    #[test]
    fn open_dialog_offers_every_advertised_format() {
        for ext in ["wav", "mp3", "ogg", "m4a", "opus", "flac", "mp4"] {
            assert!(AUDIO_EXTENSIONS.contains(&ext));
        }
    }
}
