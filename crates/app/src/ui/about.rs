use adw::prelude::*;
use gtk::License;

/// Display the "About" dialog for the application.
///
/// When `parent` is `None` the dialog is presented without a transient parent.
pub fn show_about(parent: Option<&impl IsA<gtk::Widget>>) {
    let dialog = adw::AboutDialog::builder()
        .application_name("Linux Whisper")
        .application_icon("com.linuxwhisper.LinuxWhisper")
        .developer_name("Linux Whisper Contributors")
        .version(env!("CARGO_PKG_VERSION"))
        .website("")
        .license_type(License::Gpl30)
        .comments("Local, privacy-focused speech-to-text transcription and dictation.")
        .build();

    dialog.present(parent);
}
