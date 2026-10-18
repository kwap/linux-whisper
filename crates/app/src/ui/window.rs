// SPDX-License-Identifier: GPL-3.0-or-later
//! Main application window for Linux Whisper.
//!
//! Builds the entire window layout using the builder pattern (no composite
//! templates or GObject subclassing). All widget fields are `pub` so that
//! service layers can connect additional signals after construction.

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
use linux_whisper_whisper::model_manager::ModelManager;
use linux_whisper_whisper::model_registry::{self, WhisperModel};

use crate::context::{AppAction, AppContext};
use crate::services::dictation::DictationService;
use crate::services::models::{self, ModelEvent};
use crate::services::transcription::TranscriptionService;

use super::preferences::{format_bytes, pretty_model_name};

/// Audio file extensions offered in the Open dialog and accepted by drag and drop.
const AUDIO_EXTENSIONS: [&str; 7] = ["wav", "mp3", "ogg", "m4a", "opus", "flac", "mp4"];

/// Wrapper around the main `adw::ApplicationWindow` and all of its interactive
/// child widgets. Created once and hidden/shown afterwards.
#[derive(Clone)]
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
    ctx: AppContext,
    /// The transcript on screen, used by Copy All and Export.
    transcript: Rc<RefCell<Option<Transcript>>>,
    /// Set while the model dropdown is changed from code.
    syncing_dropdown: Rc<Cell<bool>>,
    /// Whether the first-run model prompt is open.
    prompt_open: Rc<Cell<bool>>,
}

impl MainWindow {
    /// Build and return a fully wired-up main window attached to `app`.
    /// `tray_visible` says whether the window can be reopened from a tray icon.
    pub fn new(ctx: &AppContext, tray_visible: bool) -> Self {
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

        // ── Header bar ──────────────────────────────────────────────────
        let menu_model = gio::Menu::new();
        menu_model.append(
            Some(&fl!(LANGUAGE_LOADER, "preferences")),
            Some("app.preferences"),
        );
        menu_model.append(Some(&fl!(LANGUAGE_LOADER, "about")), Some("app.about"));
        menu_model.append(Some(&fl!(LANGUAGE_LOADER, "quit")), Some("app.quit"));

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
        let model_names: Vec<String> = model_registry::all_models()
            .iter()
            .map(|m| pretty_model_name(m.name))
            .collect();
        let model_name_refs: Vec<&str> = model_names.iter().map(String::as_str).collect();
        let model_string_list = gtk::StringList::new(&model_name_refs);
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

        let export_button = gtk::MenuButton::builder()
            .icon_name("document-save-symbolic")
            .label(fl!(LANGUAGE_LOADER, "export"))
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
        let spacer = gtk::Box::builder().hexpand(true).build();

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
            .label(ready_hint())
            .css_classes(vec!["dim-label", "caption"])
            .halign(gtk::Align::Start)
            .margin_start(12)
            .margin_bottom(4)
            .ellipsize(gtk::pango::EllipsizeMode::End)
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
        let toolbar_view = adw::ToolbarView::builder().content(&toast_overlay).build();
        toolbar_view.add_top_bar(&header);

        // ── Window ──────────────────────────────────────────────────────
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(fl!(LANGUAGE_LOADER, "app-name"))
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
            ctx: ctx.clone(),
            transcript: Rc::new(RefCell::new(None)),
            syncing_dropdown: Rc::new(Cell::new(false)),
            prompt_open: Rc::new(Cell::new(false)),
        };

        main_window.sync_model_dropdown();
        main_window.connect_signals(tray_visible);
        main_window
    }

    // ── Public helpers ──────────────────────────────────────────────────

    /// Present (show) the window on screen.
    pub fn present(&self) {
        self.window.present();
    }

    /// Show `transcript` in the list and make it the one Copy All and Export use.
    pub fn show_transcript(&self, transcript: Transcript, status: &str) {
        *self.transcript.borrow_mut() = Some(transcript);
        self.refresh_rows();
        self.status_label.set_label(status);
    }

    /// Rebuild the transcript rows (e.g. after the confidence setting changed).
    pub fn refresh_rows(&self) {
        self.clear_segments();
        let show_confidence = AppConfig::load().show_confidence;
        if let Some(ref transcript) = *self.transcript.borrow() {
            for segment in &transcript.segments {
                self.transcript_list
                    .append(&segment_row(segment, show_confidence));
            }
        }
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
    pub fn set_recording_state(&self, recording: bool) {
        // A GTK button shows either a label or an icon; setting one replaces
        // the other, so only the label changes here.
        if recording {
            self.record_button.set_label(&fl!(LANGUAGE_LOADER, "stop"));
            self.record_button.remove_css_class("suggested-action");
            self.record_button.add_css_class("destructive-action");
        } else {
            self.record_button
                .set_label(&fl!(LANGUAGE_LOADER, "record"));
            self.record_button.remove_css_class("destructive-action");
            self.record_button.add_css_class("suggested-action");
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
        if self.prompt_open.replace(true) {
            return;
        }
        let model = model_registry::default_model();
        let name = pretty_model_name(model.name);
        let dialog = adw::AlertDialog::new(
            Some("Download a speech model"),
            Some(&format!(
                "Linux Whisper turns speech into text with a Whisper model that runs \
                 on this computer. The {name} model ({}) is a good start. Larger \
                 models, available in Preferences, are more accurate but slower.",
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
                this.status_label
                    .set_label("No speech model yet \u{2014} choose one above or in Preferences");
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
            this.status_label
                .set_label(&models::describe(&event, &what));
            match event {
                ModelEvent::Ready => {
                    this.model_dropdown.set_sensitive(true);
                    this.status_label.set_label(&format!(
                        "{name} model ready \u{2014} press {} to dictate",
                        AppConfig::load().hotkey
                    ));
                    this.show_toast(&format!("{name} model ready"));
                }
                ModelEvent::Failed(ref e) => {
                    this.model_dropdown.set_sensitive(true);
                    this.show_toast(e);
                }
                _ => {}
            }
        });
    }

    // ── Internal signal wiring ──────────────────────────────────────────

    fn connect_signals(&self, tray_visible: bool) {
        // Record button — same as the hotkey and the tray icon.
        {
            let ctx = self.ctx.clone();
            self.record_button
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

        // Clear button — remove all transcript rows and clear state.
        {
            let this = self.clone();
            self.clear_button.connect_clicked(move |_| {
                *this.transcript.borrow_mut() = None;
                this.clear_segments();
                this.status_label.set_label(&ready_hint());
            });
        }

        // Open File button — file chooser → transcribe → display segments.
        {
            let this = self.clone();
            self.open_button.connect_clicked(move |_| {
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

                let this = this.clone();
                dialog.open(
                    Some(&this.window.clone()),
                    gio::Cancellable::NONE,
                    move |result| {
                        // An error means the user cancelled.
                        if let Some(path) = result.ok().and_then(|file| file.path()) {
                            this.transcribe_file(path);
                        }
                    },
                );
            });
        }

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

        // Copy All button — copy the transcript text to the clipboard.
        {
            let this = self.clone();
            self.copy_button.connect_clicked(move |_| {
                let text = this
                    .transcript
                    .borrow()
                    .as_ref()
                    .map(|t| t.formatted_text(&AppConfig::load().format))
                    .unwrap_or_default();
                if text.is_empty() {
                    this.show_toast("No transcript to copy");
                    return;
                }
                match DictationService::copy_to_clipboard(&text) {
                    Ok(()) => this.show_toast("Copied to clipboard"),
                    Err(e) => {
                        error!("Clipboard copy failed: {e}");
                        this.show_toast("Failed to copy to clipboard");
                    }
                }
            });
        }

        // Export button — popover with the formats.
        self.export_button
            .set_popover(Some(&build_export_popover(self.clone())));

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
                if !tray_visible && !hint_shown.replace(true) {
                    let notification = gio::Notification::new("Linux Whisper is still running");
                    notification.set_body(Some(&format!(
                        "{}. Open Linux Whisper again from your applications to see this window.",
                        ready_hint()
                    )));
                    this.ctx
                        .app
                        .send_notification(Some("background"), &notification);
                }
                glib::Propagation::Stop
            });
        }
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

        if self.ctx.worker.loaded_model().is_none() {
            let config = AppConfig::load();
            let on_disk = model_registry::find_model(&config.model)
                .is_some_and(|m| ModelManager::new(AppConfig::models_dir()).is_downloaded(m));
            if !on_disk {
                self.prompt_model_download();
                return;
            }
        }

        self.status_label
            .set_label(&format!("Transcribing {file_name}\u{2026}"));
        self.open_button.set_sensitive(false);

        let svc = TranscriptionService::new(Arc::new(self.ctx.worker.clone()));
        let config = AppConfig::load();
        let language = (config.language != "auto").then(|| config.language.clone());

        let (result_tx, result_rx) = std_mpsc::channel::<Result<Transcript, String>>();
        let path_for_task = path.clone();
        self.ctx.rt.spawn(async move {
            let result = svc.transcribe_file(&path_for_task, language).await;
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
            match result {
                Ok(mut transcript) => {
                    transcript.title = file_name.clone();
                    transcript.source = TranscriptSource::File {
                        path: path.display().to_string(),
                    };
                    let status = format!(
                        "{file_name} \u{2014} {} segment(s), {:.1}s{}",
                        transcript.segment_count(),
                        transcript.duration,
                        transcript
                            .language
                            .as_deref()
                            .map(|l| format!(", language: {l}"))
                            .unwrap_or_default(),
                    );
                    this.show_transcript(transcript, &status);
                    this.show_toast(&format!("Transcribed: {file_name}"));
                }
                Err(e) => {
                    error!("File transcription failed: {e}");
                    this.status_label.set_label("Transcription failed");
                    this.show_toast(&format!("Error: {e}"));
                }
            }
            glib::ControlFlow::Break
        });
    }
}

/// A transcript row. Text is shown as-is (not as Pango markup), so "&" and
/// "<" in what was said cannot break the row.
fn segment_row(segment: &Segment, show_confidence: bool) -> adw::ActionRow {
    let mut subtitle = format!(
        "[{} - {}]",
        format_timestamp(segment.start),
        format_timestamp(segment.end),
    );
    if show_confidence {
        if let Some(confidence) = segment.confidence {
            subtitle.push_str(&format!(" \u{00b7} {:.0}% confidence", confidence * 100.0));
        }
    }
    adw::ActionRow::builder()
        .use_markup(false)
        .title(&segment.text)
        .title_selectable(true)
        .subtitle(&subtitle)
        .build()
}

/// "Press <hotkey> to dictate".
fn ready_hint() -> String {
    format!("Press {} to dictate", AppConfig::load().hotkey)
}

// ── Export popover builder ───────────────────────────────────────────────

/// Build the export format selection popover. When a format row is activated,
/// it opens a file-save dialog and writes the exported transcript to disk.
fn build_export_popover(win: MainWindow) -> gtk::Popover {
    let formats = [
        ("Plain Text (.txt)", ExportFormat::Txt, "txt"),
        ("SubRip (.srt)", ExportFormat::Srt, "srt"),
        ("WebVTT (.vtt)", ExportFormat::Vtt, "vtt"),
        ("CSV (.csv)", ExportFormat::Csv, "csv"),
    ];

    let list_box = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(vec!["boxed-list"])
        .build();

    let popover = gtk::Popover::builder().child(&list_box).build();

    for (label, _, _) in &formats {
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

    let popover_weak = popover.downgrade();
    list_box.connect_row_activated(move |_list_box, row| {
        let Some(&(_, format, ext)) = usize::try_from(row.index())
            .ok()
            .and_then(|i| formats.get(i))
        else {
            return;
        };
        if let Some(popover) = popover_weak.upgrade() {
            popover.popdown();
        }

        let Some(transcript) = win.transcript.borrow().clone() else {
            win.show_toast("No transcript to export");
            return;
        };

        let content = match TranscriptionService::export_transcript(&transcript, format) {
            Ok(c) => c,
            Err(e) => {
                error!("Export failed: {e}");
                win.show_toast(&format!("Export failed: {e}"));
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

        let win = win.clone();
        dialog.save(
            Some(&win.window.clone()),
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
                        win.show_toast(&format!("Exported: {name}"));
                    }
                    Err(e) => {
                        error!("Save failed: {e}");
                        win.show_toast(&format!("Save failed: {e}"));
                    }
                }
            },
        );
    });

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

    #[test]
    fn open_dialog_offers_every_advertised_format() {
        for ext in ["wav", "mp3", "ogg", "m4a", "opus", "flac", "mp4"] {
            assert!(AUDIO_EXTENSIONS.contains(&ext));
        }
    }
}
