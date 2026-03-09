mod services;
mod ui;

use adw::prelude::*;
use gtk::gio;
use tracing::info;

const APP_ID: &str = "com.linuxwhisper.LinuxWhisper";

fn main() {
    // Initialize logging.
    tracing_subscriber::fmt().init();

    info!("Starting Linux Whisper v{}", env!("CARGO_PKG_VERSION"));

    // Initialize the i18n loader.
    let _ = &*linux_whisper_i18n::LANGUAGE_LOADER;

    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::FLAGS_NONE)
        .build();

    app.connect_activate(on_activate);

    // Run with empty args — GTK parses std::env::args internally.
    app.run_with_args::<String>(&[]);
}

fn on_activate(app: &adw::Application) {
    let win = ui::window::MainWindow::new(app);
    win.present();
}
