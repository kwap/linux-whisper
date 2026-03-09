use adw::prelude::*;
use gtk::License;

/// Display the "About" dialog for the application.
pub fn show_about(parent: &adw::ApplicationWindow) {
    let dialog = adw::AboutDialog::builder()
        .application_name("Linux Whisper")
        .application_icon("com.linuxwhisper.LinuxWhisper")
        .developer_name("Linux Whisper Contributors")
        .version(env!("CARGO_PKG_VERSION"))
        .website("")
        .license_type(License::Gpl30)
        .comments("Local, privacy-focused speech-to-text transcription and dictation.")
        .build();

    dialog.present(Some(parent));
}
