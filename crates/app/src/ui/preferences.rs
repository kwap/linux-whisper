use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use linux_whisper_audio::capture::CpalCapture;
use linux_whisper_core::config::AppConfig;
use linux_whisper_core::language::Language;
use linux_whisper_whisper::model_manager::ModelManager;
use linux_whisper_whisper::model_registry;
use tracing::{error, info};

/// Display the application preferences window.
///
/// `tokio_handle` is used to spawn async model downloads.
pub fn show_preferences(tokio_handle: &tokio::runtime::Handle) {
    let config = AppConfig::load();

    let prefs_window = adw::PreferencesWindow::builder()
        .title("Preferences")
        .modal(true)
        .build();

    // -----------------------------------------------------------------------
    // General page
    // -----------------------------------------------------------------------
    let general_page = adw::PreferencesPage::builder()
        .title("General")
        .icon_name("preferences-other-symbolic")
        .build();

    // -- Dictation group ----------------------------------------------------
    let dictation_group = adw::PreferencesGroup::builder()
        .title("Dictation")
        .build();

    let hotkey_row = adw::EntryRow::builder()
        .title("Global Hotkey")
        .text(&config.hotkey)
        .build();
    dictation_group.add(&hotkey_row);

    let auto_paste_row = adw::SwitchRow::builder()
        .title("Auto-paste to active window")
        .active(config.auto_paste)
        .build();
    dictation_group.add(&auto_paste_row);

    // Language combo row
    let languages = Language::all();
    let language_names: Vec<&str> = languages.iter().map(|l| l.name()).collect();
    let language_model = gtk::StringList::new(&language_names);

    let language_row = adw::ComboRow::builder()
        .title("Language")
        .model(&language_model)
        .build();

    let selected_index = languages
        .iter()
        .position(|l| l.code() == config.language)
        .unwrap_or(0);
    language_row.set_selected(selected_index as u32);

    dictation_group.add(&language_row);

    // -- Audio device -------------------------------------------------------
    let device_names = CpalCapture::new()
        .ok()
        .and_then(|c| c.list_physical_devices().ok())
        .unwrap_or_default();

    let device_model = gtk::StringList::new(
        &device_names.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
    );

    let device_row = adw::ComboRow::builder()
        .title("Audio Input Device")
        .subtitle("Currently uses the system default device")
        .model(&device_model)
        .build();

    dictation_group.add(&device_row);
    general_page.add(&dictation_group);

    // -- Appearance group ---------------------------------------------------
    let appearance_group = adw::PreferencesGroup::builder()
        .title("Appearance")
        .build();

    let theme_model = gtk::StringList::new(&["System", "Light", "Dark"]);
    let theme_row = adw::ComboRow::builder()
        .title("Theme")
        .model(&theme_model)
        .build();

    let theme_index = match config.theme {
        linux_whisper_core::config::Theme::System => 0u32,
        linux_whisper_core::config::Theme::Light => 1,
        linux_whisper_core::config::Theme::Dark => 2,
    };
    theme_row.set_selected(theme_index);

    appearance_group.add(&theme_row);

    let confidence_row = adw::SwitchRow::builder()
        .title("Show confidence scores")
        .active(config.show_confidence)
        .build();
    appearance_group.add(&confidence_row);

    general_page.add(&appearance_group);

    // -- Keyboard shortcuts note --------------------------------------------
    let shortcuts_group = adw::PreferencesGroup::builder()
        .title("Keyboard Shortcuts")
        .description(
            "Global hotkeys require access to /dev/input devices.\n\
             To enable: sudo usermod -aG input $USER && reboot",
        )
        .build();
    general_page.add(&shortcuts_group);

    prefs_window.add(&general_page);

    // -----------------------------------------------------------------------
    // Models page
    // -----------------------------------------------------------------------
    let models_page = adw::PreferencesPage::builder()
        .title("Models")
        .icon_name("folder-download-symbolic")
        .build();

    let mgr = ModelManager::new(AppConfig::models_dir());

    // -- Active model selector ----------------------------------------------
    let selector_group = adw::PreferencesGroup::builder()
        .title("Active Model")
        .build();

    let downloaded = mgr.list_downloaded();
    let downloaded_names: Vec<&str> = downloaded.iter().map(|m| m.name).collect();
    let model_list = gtk::StringList::new(&downloaded_names);

    let model_row = adw::ComboRow::builder()
        .title("Whisper Model")
        .subtitle("Model used for transcription")
        .model(&model_list)
        .build();

    // Pre-select the configured model.
    let model_selected_index = downloaded_names
        .iter()
        .position(|n| *n == config.model)
        .unwrap_or(0);
    model_row.set_selected(model_selected_index as u32);

    // Persist model choice on change.
    {
        let config_clone = config.clone();
        let names = downloaded_names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        model_row.connect_notify(Some("selected"), move |row, _| {
            let idx = row.selected() as usize;
            if let Some(name) = names.get(idx) {
                let mut cfg = config_clone.clone();
                cfg.model = name.clone();
                if let Err(e) = cfg.save() {
                    error!("Failed to save model preference: {e}");
                }
            }
        });
    }

    if downloaded.is_empty() {
        model_row.set_subtitle("Download a model below first");
        model_row.set_sensitive(false);
    }

    selector_group.add(&model_row);
    models_page.add(&selector_group);

    // -- All models list ----------------------------------------------------
    let models_group = adw::PreferencesGroup::builder()
        .title("Available Models")
        .build();

    for model in model_registry::all_models() {
        let row = adw::ActionRow::builder()
            .title(model.name)
            .subtitle(&format_bytes(model.size_bytes))
            .build();

        if mgr.is_downloaded(model) {
            let check = gtk::Image::from_icon_name("object-select-symbolic");
            check.set_valign(gtk::Align::Center);
            let label = gtk::Label::builder()
                .label("Downloaded")
                .valign(gtk::Align::Center)
                .css_classes(["dim-label"])
                .build();
            row.add_suffix(&label);
            row.add_suffix(&check);
        } else {
            let progress_bar = gtk::ProgressBar::builder()
                .valign(gtk::Align::Center)
                .hexpand(false)
                .visible(false)
                .build();
            progress_bar.set_width_request(120);

            let download_btn = gtk::Button::builder()
                .label("Download")
                .valign(gtk::Align::Center)
                .css_classes(["suggested-action"])
                .build();

            let model_clone = model.clone();
            let handle = tokio_handle.clone();
            let btn_ref = download_btn.clone();
            let bar_ref = progress_bar.clone();
            let row_ref = row.clone();

            download_btn.connect_clicked(move |_| {
                btn_ref.set_sensitive(false);
                btn_ref.set_label("Downloading…");
                bar_ref.set_visible(true);

                let model = model_clone.clone();
                let bar = bar_ref.clone();
                let btn = btn_ref.clone();
                let row = row_ref.clone();

                // Channel to send progress updates from tokio to GTK thread.
                // None signals completion, Some(fraction) is progress.
                let (prog_tx, prog_rx) = std_mpsc::channel::<Option<f64>>();

                handle.spawn(async move {
                    let mgr = ModelManager::new(AppConfig::models_dir());
                    let tx = prog_tx.clone();
                    let progress_cb = Box::new(move |downloaded: u64, total: u64| {
                        if total > 0 {
                            let fraction = downloaded as f64 / total as f64;
                            let _ = tx.send(Some(fraction));
                        }
                    });

                    match mgr.download(&model, Some(progress_cb)).await {
                        Ok(path) => {
                            info!("Model '{}' downloaded to {}", model.name, path.display());
                        }
                        Err(e) => {
                            error!("Model download failed: {e}");
                        }
                    }
                    let _ = prog_tx.send(None); // signals completion
                });

                // Poll for progress updates from the GTK main loop.
                glib::timeout_add_local(Duration::from_millis(100), move || {
                    loop {
                        match prog_rx.try_recv() {
                            Ok(Some(fraction)) => {
                                bar.set_fraction(fraction);
                            }
                            Ok(None) => {
                                // Download finished — replace button with checkmark.
                                bar.set_visible(false);
                                btn.set_visible(false);
                                let check =
                                    gtk::Image::from_icon_name("object-select-symbolic");
                                check.set_valign(gtk::Align::Center);
                                let label = gtk::Label::builder()
                                    .label("Downloaded")
                                    .valign(gtk::Align::Center)
                                    .css_classes(["dim-label"])
                                    .build();
                                row.add_suffix(&label);
                                row.add_suffix(&check);
                                return glib::ControlFlow::Break;
                            }
                            Err(std_mpsc::TryRecvError::Empty) => break,
                            Err(std_mpsc::TryRecvError::Disconnected) => {
                                bar.set_visible(false);
                                btn.set_label("Failed");
                                btn.set_sensitive(false);
                                return glib::ControlFlow::Break;
                            }
                        }
                    }
                    glib::ControlFlow::Continue
                });
            });

            row.add_suffix(&progress_bar);
            row.add_suffix(&download_btn);
        }

        models_group.add(&row);
    }

    models_page.add(&models_group);
    prefs_window.add(&models_page);

    // -----------------------------------------------------------------------
    // Present
    // -----------------------------------------------------------------------
    prefs_window.present();
}

/// Format a byte count into a human-readable string (e.g. "74 MB", "1.4 GB").
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1_024;
    const MB: u64 = 1_024 * KB;
    const GB: u64 = 1_024 * MB;

    if bytes >= GB {
        let value = bytes as f64 / GB as f64;
        if value >= 10.0 {
            format!("{:.0} GB", value)
        } else {
            format!("{:.1} GB", value)
        }
    } else if bytes >= MB {
        let value = bytes as f64 / MB as f64;
        if value >= 10.0 {
            format!("{:.0} MB", value)
        } else {
            format!("{:.1} MB", value)
        }
    } else if bytes >= KB {
        let value = bytes as f64 / KB as f64;
        if value >= 10.0 {
            format!("{:.0} KB", value)
        } else {
            format!("{:.1} KB", value)
        }
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bytes_zero() {
        assert_eq!(format_bytes(0), "0 B");
    }

    #[test]
    fn format_bytes_small() {
        assert_eq!(format_bytes(512), "512 B");
    }

    #[test]
    fn format_bytes_one_and_half_kb() {
        assert_eq!(format_bytes(1_536), "1.5 KB");
    }

    #[test]
    fn format_bytes_megabytes() {
        assert_eq!(format_bytes(77_704_715), "74 MB");
    }

    #[test]
    fn format_bytes_large_megabytes() {
        assert_eq!(format_bytes(147_964_211), "141 MB");
    }

    #[test]
    fn format_bytes_gigabytes() {
        assert_eq!(format_bytes(3_094_623_691), "2.9 GB");
    }
}
