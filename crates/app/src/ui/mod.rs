#[allow(dead_code)]
pub mod about;
#[allow(dead_code)]
pub mod model_row;
#[allow(dead_code)]
pub mod preferences;
pub mod shortcut;
#[allow(dead_code)]
pub mod window;

use gtk::gdk;

/// Load the app's styles and make its icon available, also when it runs
/// from the source tree rather than installed.
pub fn init_style() {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_data(include_str!("style.css"));
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let dev_icon_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/icons");
    if std::path::Path::new(dev_icon_dir).exists() {
        gtk::IconTheme::for_display(&display).add_search_path(dev_icon_dir);
    }
    gtk::Window::set_default_icon_name("com.linuxwhisper.LinuxWhisper");
}
