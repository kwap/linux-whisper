use std::sync::mpsc as std_mpsc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use linux_whisper_audio::capture::CpalCapture;
use linux_whisper_core::config::AppConfig;
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
        .default_width(600)
        .default_height(700)
        .build();

    // -----------------------------------------------------------------------
    // General page
    // -----------------------------------------------------------------------
    let general_page = adw::PreferencesPage::builder()
        .title("General")
        .icon_name("preferences-other-symbolic")
        .build();

    // -- Dictation group ----------------------------------------------------
    let dictation_group = adw::PreferencesGroup::builder().title("Dictation").build();

    // Model selector — compact names: "Tiny (74 MB)" instead of long strings.
    let all_models = model_registry::all_models();
    let mgr = ModelManager::new(AppConfig::models_dir());

    let model_names: Vec<String> = all_models
        .iter()
        .map(|m| {
            let pretty = pretty_model_name(m.name);
            let size = format_bytes(m.size_bytes);
            if mgr.is_downloaded(m) {
                format!("\u{2713} {pretty} ({size})") // ✓ checkmark
            } else {
                format!("  {pretty} ({size})") // aligned with checkmark
            }
        })
        .collect();
    let model_name_refs: Vec<&str> = model_names.iter().map(|s| s.as_str()).collect();
    let model_list = gtk::StringList::new(&model_name_refs);

    let current_downloaded = model_registry::find_model(&config.model)
        .map(|m| mgr.is_downloaded(m))
        .unwrap_or(false);

    let model_row = adw::ComboRow::builder()
        .title("Whisper Model")
        .subtitle(if current_downloaded {
            "Model loaded and ready"
        } else {
            "No model downloaded \u{2014} select one to download"
        })
        .model(&model_list)
        .build();

    // Pre-select configured model (without triggering download).
    let model_idx = all_models
        .iter()
        .position(|m| m.name == config.model)
        .unwrap_or(0);
    model_row.set_selected(model_idx as u32);

    // On model change: save config + auto-download if needed.
    {
        let config_for_model = config.clone();
        let handle_for_model = tokio_handle.clone();
        let initial_idx = std::sync::atomic::AtomicU32::new(model_idx as u32);
        let first_change = std::sync::atomic::AtomicBool::new(true);

        model_row.connect_notify(Some("selected"), move |row, _| {
            let idx = row.selected();

            // Skip the initial set_selected callback.
            use std::sync::atomic::Ordering::Relaxed;
            if first_change.load(Relaxed) && idx == initial_idx.load(Relaxed) {
                first_change.store(false, Relaxed);
                return;
            }
            first_change.store(false, Relaxed);

            let idx = idx as usize;
            if let Some(model) = all_models.get(idx) {
                let mut cfg = config_for_model.clone();
                cfg.model = model.name.to_string();
                if let Err(e) = cfg.save() {
                    error!("Failed to save model preference: {e}");
                }
                info!("Model preference changed to '{}'", model.name);

                // Already downloaded — just confirm.
                let mgr = ModelManager::new(AppConfig::models_dir());
                if mgr.is_downloaded(model) {
                    row.set_subtitle("Model loaded and ready");
                    return;
                }

                // Auto-download.
                let model_name = model.name.to_string();
                info!("Auto-downloading model '{}'", model_name);

                let model_for_task = model.clone();
                let (prog_tx, prog_rx) = std_mpsc::channel::<Option<f64>>();

                handle_for_model.spawn(async move {
                    let mgr = ModelManager::new(AppConfig::models_dir());
                    let tx = prog_tx.clone();
                    let progress_cb = Box::new(move |downloaded: u64, total: u64| {
                        if total > 0 {
                            let _ = tx.send(Some(downloaded as f64 / total as f64));
                        }
                    });
                    let ok = mgr
                        .download(&model_for_task, Some(progress_cb))
                        .await
                        .is_ok();
                    if ok {
                        info!("Model '{}' downloaded", model_for_task.name);
                    } else {
                        error!("Model '{}' download failed", model_for_task.name);
                    }
                    let _ = prog_tx.send(None); // signal done
                });

                let row_ref = row.clone();
                let name_done = model_name.clone();
                row.set_subtitle(&format!("Downloading {model_name}..."));

                glib::timeout_add_local(Duration::from_millis(200), move || {
                    loop {
                        match prog_rx.try_recv() {
                            Ok(Some(frac)) => {
                                let pct = (frac * 100.0) as u32;
                                row_ref.set_subtitle(&format!("Downloading {name_done}... {pct}%"));
                            }
                            Ok(None) => {
                                row_ref.set_subtitle("Model loaded and ready");
                                return glib::ControlFlow::Break;
                            }
                            Err(std_mpsc::TryRecvError::Empty) => break,
                            Err(std_mpsc::TryRecvError::Disconnected) => {
                                row_ref.set_subtitle("Download failed");
                                return glib::ControlFlow::Break;
                            }
                        }
                    }
                    glib::ControlFlow::Continue
                });
            }
        });
    }

    dictation_group.add(&model_row);

    // Language selector.
    let languages = [
        ("auto", "Auto-detect"),
        ("en", "English"),
        ("pl", "Polish"),
        ("es", "Spanish"),
        ("fr", "French"),
        ("de", "German"),
        ("it", "Italian"),
        ("pt", "Portuguese"),
        ("nl", "Dutch"),
        ("ja", "Japanese"),
        ("ko", "Korean"),
        ("zh", "Chinese"),
        ("ru", "Russian"),
        ("uk", "Ukrainian"),
        ("cs", "Czech"),
        ("sv", "Swedish"),
        ("da", "Danish"),
        ("fi", "Finnish"),
        ("no", "Norwegian"),
    ];
    let lang_names: Vec<&str> = languages.iter().map(|(_, name)| *name).collect();
    let lang_list = gtk::StringList::new(&lang_names);

    let lang_row = adw::ComboRow::builder()
        .title("Language")
        .subtitle("Language for speech recognition")
        .model(&lang_list)
        .build();

    let lang_idx = languages
        .iter()
        .position(|(code, _)| *code == config.language)
        .unwrap_or(0);
    lang_row.set_selected(lang_idx as u32);

    // Save language on change.
    {
        let config_for_lang = config.clone();
        let initial_lang_idx = std::sync::atomic::AtomicU32::new(lang_idx as u32);
        let first_lang_change = std::sync::atomic::AtomicBool::new(true);

        lang_row.connect_notify(Some("selected"), move |row, _| {
            let idx = row.selected();
            use std::sync::atomic::Ordering::Relaxed;
            if first_lang_change.load(Relaxed) && idx == initial_lang_idx.load(Relaxed) {
                first_lang_change.store(false, Relaxed);
                return;
            }
            first_lang_change.store(false, Relaxed);

            if let Some((code, _)) = languages.get(idx as usize) {
                let mut cfg = config_for_lang.clone();
                cfg.language = code.to_string();
                if let Err(e) = cfg.save() {
                    error!("Failed to save language preference: {e}");
                }
                info!("Language preference changed to '{}'", code);
            }
        });
    }

    dictation_group.add(&lang_row);

    let hotkey_row = adw::EntryRow::builder()
        .title("Global Hotkey")
        .text(&config.hotkey)
        .build();
    dictation_group.add(&hotkey_row);

    let auto_paste_row = adw::SwitchRow::builder()
        .title("Auto-paste to active window")
        .subtitle("Type transcribed text into the focused window automatically")
        .active(config.auto_paste)
        .build();
    dictation_group.add(&auto_paste_row);

    // -- Audio device -------------------------------------------------------
    let device_names = CpalCapture::new()
        .ok()
        .and_then(|c| c.list_physical_devices().ok())
        .unwrap_or_default();

    if !device_names.is_empty() {
        let device_refs: Vec<&str> = device_names.iter().map(|s| s.as_str()).collect();
        let device_model = gtk::StringList::new(&device_refs);

        let device_row = adw::ComboRow::builder()
            .title("Audio Input Device")
            .subtitle("Uses the system default device")
            .model(&device_model)
            .build();

        dictation_group.add(&device_row);
    }

    general_page.add(&dictation_group);

    // -- Appearance group ---------------------------------------------------
    let appearance_group = adw::PreferencesGroup::builder().title("Appearance").build();

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
        .subtitle("Display word-level confidence in transcription results")
        .active(config.show_confidence)
        .build();
    appearance_group.add(&confidence_row);

    general_page.add(&appearance_group);

    // -- Model storage with expandable delete list --------------------------
    let storage_group = adw::PreferencesGroup::builder()
        .title("Model Storage")
        .description("Manage downloaded whisper models")
        .build();

    let mgr = ModelManager::new(AppConfig::models_dir());
    let all = model_registry::all_models();
    let downloaded: Vec<_> = all.iter().filter(|m| mgr.is_downloaded(m)).collect();
    let total_size: u64 = downloaded.iter().map(|m| m.size_bytes).sum();

    // Expander row that shows downloaded models when clicked.
    let storage_expander = adw::ExpanderRow::builder()
        .title("Downloaded Models")
        .subtitle(&format!(
            "{} model(s) \u{2014} {}",
            downloaded.len(),
            format_bytes(total_size)
        ))
        .show_enable_switch(false)
        .build();

    // Add a row for each downloaded model with a delete button.
    for model in &downloaded {
        let row = adw::ActionRow::builder()
            .title(pretty_model_name(model.name))
            .subtitle(&format_bytes(model.size_bytes))
            .build();

        let delete_btn = gtk::Button::builder()
            .icon_name("edit-delete-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular", "error"])
            .tooltip_text("Delete model")
            .build();

        let model_clone = (*model).clone();
        let row_ref = row.clone();
        let expander_ref = storage_expander.clone();

        delete_btn.connect_clicked(move |btn| {
            let mgr = ModelManager::new(AppConfig::models_dir());
            match mgr.delete(&model_clone) {
                Ok(()) => {
                    info!("Deleted model '{}'", model_clone.name);
                    btn.set_visible(false);
                    row_ref.set_subtitle("Deleted");
                    row_ref.set_sensitive(false);

                    // Update expander subtitle.
                    let mgr = ModelManager::new(AppConfig::models_dir());
                    let remaining: Vec<_> = model_registry::all_models()
                        .iter()
                        .filter(|m| mgr.is_downloaded(m))
                        .collect();
                    let size: u64 = remaining.iter().map(|m| m.size_bytes).sum();
                    expander_ref.set_subtitle(&format!(
                        "{} model(s) \u{2014} {}",
                        remaining.len(),
                        format_bytes(size)
                    ));
                }
                Err(e) => {
                    error!("Failed to delete model: {e}");
                }
            }
        });

        row.add_suffix(&delete_btn);
        storage_expander.add_row(&row);
    }

    if downloaded.is_empty() {
        let empty_row = adw::ActionRow::builder()
            .title("No models downloaded")
            .sensitive(false)
            .build();
        storage_expander.add_row(&empty_row);
    }

    storage_group.add(&storage_expander);

    general_page.add(&storage_group);

    // -- System note --------------------------------------------------------
    let system_group = adw::PreferencesGroup::builder()
        .title("System")
        .description(
            "Global hotkeys require access to /dev/input devices.\n\
             To enable: sudo usermod -aG input $USER &amp;&amp; reboot",
        )
        .build();

    general_page.add(&system_group);

    prefs_window.add(&general_page);

    // -----------------------------------------------------------------------
    // Present
    // -----------------------------------------------------------------------
    prefs_window.present();
}

/// Make model names more human-friendly.
/// "base" → "Base", "tiny.en" → "Tiny EN", "large-v3" → "Large V3"
fn pretty_model_name(name: &str) -> String {
    match name {
        "tiny.en" => "Tiny EN".to_string(),
        "base.en" => "Base EN".to_string(),
        "small.en" => "Small EN".to_string(),
        "medium.en" => "Medium EN".to_string(),
        "large-v3" => "Large V3".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(c) => format!("{}{}", c.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        }
    }
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

    #[test]
    fn pretty_model_name_basic() {
        assert_eq!(pretty_model_name("tiny"), "Tiny");
        assert_eq!(pretty_model_name("base"), "Base");
        assert_eq!(pretty_model_name("small"), "Small");
        assert_eq!(pretty_model_name("medium"), "Medium");
    }

    #[test]
    fn pretty_model_name_english_variants() {
        assert_eq!(pretty_model_name("tiny.en"), "Tiny EN");
        assert_eq!(pretty_model_name("base.en"), "Base EN");
        assert_eq!(pretty_model_name("small.en"), "Small EN");
        assert_eq!(pretty_model_name("medium.en"), "Medium EN");
    }

    #[test]
    fn pretty_model_name_large_v3() {
        assert_eq!(pretty_model_name("large-v3"), "Large V3");
    }
}
