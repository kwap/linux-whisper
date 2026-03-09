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
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use std::time::Duration;

use tracing::{error, info};

use linux_whisper_core::config::AppConfig;
use linux_whisper_core::export::ExportFormat;
use linux_whisper_core::model::{Segment, Transcript};
use linux_whisper_i18n::{fl, LANGUAGE_LOADER};
use linux_whisper_whisper::model_registry;
use linux_whisper_whisper::worker::WhisperWorker;

use crate::services::dictation::DictationService;
use crate::services::transcription::TranscriptionService;

use super::about;

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
    pub fn new(
        app: &adw::Application,
        worker: WhisperWorker,
        tokio_handle: tokio::runtime::Handle,
    ) -> Self {
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
            let tokio_handle = tokio_handle.clone();
            preferences_action.connect_activate(move |_, _| {
                crate::ui::preferences::show_preferences(&tokio_handle);
            });
        }
        {
            about_action.connect_activate(move |_, _| {
                about::show_about(gtk::Widget::NONE);
            });
        }
        {
            let app_clone = app.clone();
            quit_action.connect_activate(move |_, _| {
                app_clone.quit();
            });
        }

        // ── Connect widget signals ────────────────────────────────────
        main_window.connect_signals(worker, tokio_handle);

        main_window
    }

    // ── Public helpers ──────────────────────────────────────────────────

    /// Present (show) the window on screen.
    pub fn present(&self) {
        self.window.present();
    }

    /// Append a single segment row to the transcript list.
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

    fn connect_signals(&self, worker: WhisperWorker, tokio_handle: tokio::runtime::Handle) {
        // Shared transcript state — updated after transcription, used by
        // export and copy-all.
        let transcript_state: Rc<RefCell<Option<Transcript>>> = Rc::new(RefCell::new(None));

        // Record button — visual toggle only (recording is handled via tray).
        {
            let is_recording = Rc::new(RefCell::new(false));
            let record_button = self.record_button.clone();
            self.record_button.connect_clicked(move |_| {
                let mut recording = is_recording.borrow_mut();
                *recording = !*recording;
                if *recording {
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

        // Clear button — remove all transcript rows and clear state.
        {
            let transcript_list = self.transcript_list.clone();
            let transcript_state = Rc::clone(&transcript_state);
            let status_label = self.status_label.clone();
            self.clear_button.connect_clicked(move |_| {
                while let Some(row) = transcript_list.row_at_index(0) {
                    transcript_list.remove(&row);
                }
                *transcript_state.borrow_mut() = None;
                status_label.set_label("Ready");
            });
        }

        // Open File button — file chooser → transcribe → display segments.
        {
            let window = self.window.clone();
            let transcript_list = self.transcript_list.clone();
            let toast_overlay = self.toast_overlay.clone();
            let status_label = self.status_label.clone();
            let transcript_state = Rc::clone(&transcript_state);
            let worker = worker.clone();
            let tokio_handle = tokio_handle.clone();

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

                let transcript_list = transcript_list.clone();
                let toast_overlay = toast_overlay.clone();
                let status_label = status_label.clone();
                let transcript_state = Rc::clone(&transcript_state);
                let worker = worker.clone();
                let tokio_handle = tokio_handle.clone();

                dialog.open(Some(&window), gio::Cancellable::NONE, move |result| {
                    match result {
                        Ok(file) => {
                            if let Some(path) = file.path() {
                                let file_name = path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_else(|| "file".to_string());

                                status_label.set_label(&format!("Transcribing {file_name}..."));

                                // Spawn transcription on tokio.
                                let svc = TranscriptionService::new(Arc::new(worker.clone()));
                                let config = AppConfig::load();
                                let language = match config.language.as_str() {
                                    "auto" => None,
                                    lang => Some(lang.to_string()),
                                };

                                let (result_tx, result_rx) =
                                    std_mpsc::channel::<Result<Transcript, String>>();

                                let path_clone = path.clone();
                                tokio_handle.spawn(async move {
                                    let result = svc.transcribe_file(&path_clone, language).await;
                                    let _ = result_tx.send(
                                        result.map_err(|e| e.to_string()),
                                    );
                                });

                                // Poll for result on GTK thread.
                                let transcript_list = transcript_list.clone();
                                let toast_overlay = toast_overlay.clone();
                                let status_label = status_label.clone();
                                let transcript_state = Rc::clone(&transcript_state);

                                glib::timeout_add_local(
                                    Duration::from_millis(100),
                                    move || match result_rx.try_recv() {
                                        Ok(Ok(transcript)) => {
                                            // Clear existing rows.
                                            while let Some(row) =
                                                transcript_list.row_at_index(0)
                                            {
                                                transcript_list.remove(&row);
                                            }

                                            // Display segments.
                                            let seg_count = transcript.segment_count();
                                            for seg in &transcript.segments {
                                                let subtitle = format!(
                                                    "[{} - {}]",
                                                    format_timestamp(seg.start),
                                                    format_timestamp(seg.end),
                                                );
                                                let row = adw::ActionRow::builder()
                                                    .title(&seg.text)
                                                    .subtitle(&subtitle)
                                                    .build();
                                                transcript_list.append(&row);
                                            }

                                            status_label.set_label(&format!(
                                                "{file_name} — {seg_count} segment(s), {:.1}s",
                                                transcript.duration,
                                            ));

                                            *transcript_state.borrow_mut() = Some(transcript);

                                            let toast = adw::Toast::new(&format!(
                                                "Transcribed: {file_name}"
                                            ));
                                            toast_overlay.add_toast(toast);

                                            glib::ControlFlow::Break
                                        }
                                        Ok(Err(e)) => {
                                            error!("File transcription failed: {e}");
                                            status_label.set_label("Transcription failed");
                                            let toast =
                                                adw::Toast::new(&format!("Error: {e}"));
                                            toast_overlay.add_toast(toast);
                                            glib::ControlFlow::Break
                                        }
                                        Err(std_mpsc::TryRecvError::Empty) => {
                                            glib::ControlFlow::Continue
                                        }
                                        Err(std_mpsc::TryRecvError::Disconnected) => {
                                            status_label.set_label("Transcription failed");
                                            glib::ControlFlow::Break
                                        }
                                    },
                                );
                            }
                        }
                        Err(_) => {
                            // User cancelled — do nothing.
                        }
                    }
                });
            });
        }

        // Copy All button — copy full transcript text to clipboard.
        {
            let toast_overlay = self.toast_overlay.clone();
            let transcript_state = Rc::clone(&transcript_state);
            self.copy_button.connect_clicked(move |_| {
                let state = transcript_state.borrow();
                if let Some(ref transcript) = *state {
                    let text = transcript.full_text();
                    if text.is_empty() {
                        let toast = adw::Toast::new("No text to copy");
                        toast_overlay.add_toast(toast);
                        return;
                    }
                    match DictationService::copy_to_clipboard(&text) {
                        Ok(()) => {
                            let toast = adw::Toast::new("Copied to clipboard");
                            toast_overlay.add_toast(toast);
                        }
                        Err(e) => {
                            error!("Clipboard copy failed: {e}");
                            let toast = adw::Toast::new("Failed to copy to clipboard");
                            toast_overlay.add_toast(toast);
                        }
                    }
                } else {
                    let toast = adw::Toast::new("No transcript to copy");
                    toast_overlay.add_toast(toast);
                }
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

/// Build the export format selection popover. When a format row is activated,
/// it opens a file-save dialog and writes the exported transcript to disk.
fn build_export_popover() -> gtk::Popover {
    let formats = [
        ("Plain Text (.txt)", "txt"),
        ("SubRip (.srt)", "srt"),
        ("WebVTT (.vtt)", "vtt"),
        ("CSV (.csv)", "csv"),
    ];

    let list_box = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(vec!["boxed-list"])
        .build();

    let popover = gtk::Popover::builder()
        .child(&list_box)
        .build();

    for (label, _ext) in &formats {
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

    // Row activation → dismiss popover, open save dialog, write export.
    {
        let popover_weak = popover.downgrade();
        list_box.connect_row_activated(move |_list_box, row| {
            let index = row.index();
            let (format, ext, format_label) = match index {
                0 => (ExportFormat::Txt, "txt", "TXT"),
                1 => (ExportFormat::Srt, "srt", "SRT"),
                2 => (ExportFormat::Vtt, "vtt", "VTT"),
                3 => (ExportFormat::Csv, "csv", "CSV"),
                _ => return,
            };

            if let Some(popover) = popover_weak.upgrade() {
                popover.popdown();

                // Walk the widget tree to find the ToastOverlay and the
                // ApplicationWindow.
                let mut toast_overlay_opt: Option<adw::ToastOverlay> = None;
                let mut app_window_opt: Option<adw::ApplicationWindow> = None;

                let mut ancestor = popover.parent();
                while let Some(widget) = ancestor {
                    if toast_overlay_opt.is_none() {
                        if let Some(overlay) = widget.downcast_ref::<adw::ToastOverlay>() {
                            toast_overlay_opt = Some(overlay.clone());
                        }
                    }
                    if app_window_opt.is_none() {
                        if let Some(win) = widget.downcast_ref::<adw::ApplicationWindow>() {
                            app_window_opt = Some(win.clone());
                        }
                    }
                    ancestor = widget.parent();
                }

                let Some(toast_overlay) = toast_overlay_opt else {
                    return;
                };
                let Some(app_window) = app_window_opt else {
                    return;
                };

                // Find the transcript state via the transcript_list in the
                // widget tree. We export whatever rows are currently displayed.
                // Build a Transcript from the ActionRows in the list.
                let transcript = collect_transcript_from_list(&app_window);
                if transcript.segments.is_empty() {
                    let toast = adw::Toast::new("No transcript to export");
                    toast_overlay.add_toast(toast);
                    return;
                }

                // Generate the export content.
                let content = match TranscriptionService::export_transcript(&transcript, format) {
                    Ok(c) => c,
                    Err(e) => {
                        error!("Export failed: {e}");
                        let toast = adw::Toast::new(&format!("Export failed: {e}"));
                        toast_overlay.add_toast(toast);
                        return;
                    }
                };

                // Open a save dialog.
                let file_filter = gtk::FileFilter::new();
                file_filter.set_name(Some(format_label));
                gtk::FileFilter::add_suffix(&file_filter, ext);

                let filters = gio::ListStore::new::<gtk::FileFilter>();
                filters.append(&file_filter);

                let default_name = format!("transcript.{ext}");
                let dialog = gtk::FileDialog::builder()
                    .title(&format!("Export as {format_label}"))
                    .initial_name(&default_name)
                    .filters(&filters)
                    .modal(true)
                    .build();

                let toast_overlay = toast_overlay.clone();
                dialog.save(
                    Some(&app_window),
                    gio::Cancellable::NONE,
                    move |result| {
                        match result {
                            Ok(file) => {
                                if let Some(path) = file.path() {
                                    match TranscriptionService::save_export(&content, &path) {
                                        Ok(()) => {
                                            let name = path
                                                .file_name()
                                                .map(|n| n.to_string_lossy().to_string())
                                                .unwrap_or_default();
                                            info!("Exported to {}", path.display());
                                            let toast = adw::Toast::new(&format!(
                                                "Exported: {name}"
                                            ));
                                            toast_overlay.add_toast(toast);
                                        }
                                        Err(e) => {
                                            error!("Save failed: {e}");
                                            let toast = adw::Toast::new(&format!(
                                                "Save failed: {e}"
                                            ));
                                            toast_overlay.add_toast(toast);
                                        }
                                    }
                                }
                            }
                            Err(_) => {
                                // User cancelled.
                            }
                        }
                    },
                );
            }
        });
    }

    popover
}

/// Collect transcript segments from the ListBox rows currently in the window.
/// This walks the widget tree starting from the ApplicationWindow to find
/// the ListBox with class "boxed-list", then reads ActionRow titles/subtitles.
fn collect_transcript_from_list(window: &adw::ApplicationWindow) -> Transcript {
    use linux_whisper_core::model::TranscriptSource;

    let mut transcript = Transcript::new(
        "Transcription",
        None,
        "",
        TranscriptSource::File { path: String::new() },
    );

    // Find the ListBox by walking children.
    fn find_list_box(widget: &impl IsA<gtk::Widget>) -> Option<gtk::ListBox> {
        let widget = widget.upcast_ref::<gtk::Widget>();
        if let Some(lb) = widget.downcast_ref::<gtk::ListBox>() {
            if lb.css_classes().iter().any(|c| c == "boxed-list") {
                return Some(lb.clone());
            }
        }
        let mut child = widget.first_child();
        while let Some(c) = child {
            if let Some(lb) = find_list_box(&c) {
                return Some(lb);
            }
            child = c.next_sibling();
        }
        None
    }

    let Some(list_box) = find_list_box(window) else {
        return transcript;
    };

    let mut i = 0;
    while let Some(row) = list_box.row_at_index(i) {
        if let Some(action_row) = row.downcast_ref::<adw::ActionRow>() {
            let text = action_row.title().to_string();
            let subtitle = action_row
                .subtitle()
                .map(|s| s.to_string())
                .unwrap_or_default();

            // Parse timestamps from subtitle like "[00:01.5 - 00:03.2]"
            let (start, end) = parse_timestamp_range(&subtitle);
            transcript.add_segment(Segment::new(start, end, text));
        }
        i += 1;
    }

    transcript
}

/// Parse a timestamp range like "[00:01.5 - 00:03.2]" into (start_secs, end_secs).
fn parse_timestamp_range(s: &str) -> (f64, f64) {
    let s = s.trim_matches(|c: char| c == '[' || c == ']' || c.is_whitespace());
    let parts: Vec<&str> = s.split(" - ").collect();
    if parts.len() == 2 {
        (parse_timestamp(parts[0]), parse_timestamp(parts[1]))
    } else {
        (0.0, 0.0)
    }
}

/// Parse a timestamp like "01:05.5" into seconds.
fn parse_timestamp(s: &str) -> f64 {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() == 2 {
        let mins: f64 = parts[0].parse().unwrap_or(0.0);
        let secs: f64 = parts[1].parse().unwrap_or(0.0);
        mins * 60.0 + secs
    } else {
        0.0
    }
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
    fn parse_timestamp_basic() {
        assert!((parse_timestamp("01:05.5") - 65.5).abs() < 0.01);
        assert!((parse_timestamp("00:00.0") - 0.0).abs() < 0.01);
    }

    #[test]
    fn parse_timestamp_range_basic() {
        let (start, end) = parse_timestamp_range("[00:01.5 - 00:03.2]");
        assert!((start - 1.5).abs() < 0.01);
        assert!((end - 3.2).abs() < 0.01);
    }
}
