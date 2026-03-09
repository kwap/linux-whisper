use adw::prelude::*;
use linux_whisper_core::config::AppConfig;
use linux_whisper_core::language::Language;
use linux_whisper_whisper::model_registry;

/// Display the application preferences window.
pub fn show_preferences(parent: &adw::ApplicationWindow) {
    let config = AppConfig::default();

    let prefs_window = adw::PreferencesWindow::builder()
        .title("Preferences")
        .transient_for(parent)
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

    // Select the language matching the current config.
    let selected_index = languages
        .iter()
        .position(|l| l.code() == config.language)
        .unwrap_or(0);
    language_row.set_selected(selected_index as u32);

    dictation_group.add(&language_row);
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

    let minimize_row = adw::SwitchRow::builder()
        .title("Minimize to system tray")
        .active(config.minimize_to_tray)
        .build();
    appearance_group.add(&minimize_row);

    let confidence_row = adw::SwitchRow::builder()
        .title("Show confidence scores")
        .active(config.show_confidence)
        .build();
    appearance_group.add(&confidence_row);

    general_page.add(&appearance_group);
    prefs_window.add(&general_page);

    // -----------------------------------------------------------------------
    // Models page
    // -----------------------------------------------------------------------
    let models_page = adw::PreferencesPage::builder()
        .title("Models")
        .icon_name("folder-download-symbolic")
        .build();

    let models_group = adw::PreferencesGroup::builder()
        .title("Whisper Models")
        .build();

    for model in model_registry::all_models() {
        let row = adw::ActionRow::builder()
            .title(model.name)
            .subtitle(&format_bytes(model.size_bytes))
            .build();

        let download_btn = gtk::Button::builder()
            .label("Download")
            .valign(gtk::Align::Center)
            .css_classes(["suggested-action"])
            .build();

        row.add_suffix(&download_btn);
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
