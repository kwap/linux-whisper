use adw::prelude::*;
use gtk::License;

/// Display the "About" dialog for the application.
///
/// When `parent` is `None` the dialog is presented without a transient parent.
pub fn show_about(parent: Option<&impl IsA<gtk::Widget>>) {
    // Register the app icon from the bundled SVG so it shows in the dialog.
    register_app_icon();

    let dialog = adw::AboutDialog::builder()
        .application_name("Linux Whisper")
        .application_icon("com.linuxwhisper.LinuxWhisper")
        .developer_name("Piotr Kwapin")
        .version(env!("CARGO_PKG_VERSION"))
        .release_notes_version(env!("CARGO_PKG_VERSION"))
        .website("https://github.com/piotrkwapin/linux-whisper")
        .issue_url("https://github.com/piotrkwapin/linux-whisper/issues")
        .license_type(License::Gpl30)
        .comments("Local, privacy-focused speech-to-text transcription and dictation for Linux.\n\nPowered by OpenAI Whisper via whisper.cpp \u{2014} all processing happens on your device.\n\nFeatures: system-wide hotkey dictation, audio file transcription, export to TXT/SRT/VTT/CSV, 9 model sizes, 50+ languages, CUDA GPU acceleration.\n\nThe geometric pigeon tray icon represents your always-listening assistant \u{2014} glowing red when recording.\n\nReleased March 2026.")
        .build();

    dialog.add_acknowledgement_section(Some("Built With"), &[
        "whisper.cpp / whisper-rs https://github.com/tazz4843/whisper-rs",
        "GTK4 / libadwaita https://gtk.org",
        "CPAL https://github.com/RustAudio/cpal",
    ]);

    dialog.add_acknowledgement_section(Some("Special Thanks"), &[
        "Claude by Anthropic \u{2014} AI pair-programming partner https://claude.ai",
    ]);

    dialog.present(parent);
}

/// Register the application icon with GTK's icon theme so it can be
/// referenced by name in the About dialog and elsewhere.
fn register_app_icon() {
    use gtk::gdk;

    // Try loading from standard install paths first.
    let icon_theme = gtk::IconTheme::for_display(&gdk::Display::default().expect("no display"));

    // If not installed system-wide, add our data/icons directory.
    let dev_icon_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/icons");
    if std::path::Path::new(dev_icon_dir).exists() {
        icon_theme.add_search_path(dev_icon_dir);
    }
}
