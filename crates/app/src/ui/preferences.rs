use adw::prelude::*;
use linux_whisper_audio::capture::CpalCapture;
use linux_whisper_core::config::{AppConfig, PasteShortcut, Theme};
use linux_whisper_core::language::Language;
use linux_whisper_llm::model_manager::LlmModelManager;
use linux_whisper_platform::hotkey::check_input_group_membership;
use linux_whisper_platform::text_inject::UinputPaster;
use linux_whisper_whisper::model_manager::ModelManager;
use linux_whisper_whisper::model_registry;
use tracing::{error, info};

use std::cell::RefCell;
use std::rc::Rc;

use crate::context::{AppAction, AppContext};
use crate::services::models::{self, ModelEvent};

use super::shortcut;

/// Save a setting. Every change re-reads the file first, so settings changed
/// one after another never overwrite each other.
fn save(change: impl FnOnce(&mut AppConfig)) {
    if let Err(e) = AppConfig::update(change) {
        error!("Failed to save settings: {e}");
    }
}

/// Display the application preferences window.
pub fn show_preferences(ctx: &AppContext, parent: Option<&adw::ApplicationWindow>) {
    let config = AppConfig::load();

    let prefs_window = adw::PreferencesWindow::builder()
        .title("Preferences")
        .default_width(600)
        .default_height(700)
        .build();
    if let Some(parent) = parent.filter(|p| p.is_visible()) {
        prefs_window.set_transient_for(Some(parent));
        prefs_window.set_modal(true);
    }

    let general_page = adw::PreferencesPage::builder()
        .title("General")
        .icon_name("preferences-other-symbolic")
        .build();

    general_page.add(&dictation_group(ctx, &config, &prefs_window));
    general_page.add(&formatting_group(ctx, &config, &prefs_window));
    general_page.add(&appearance_group(ctx, &config));
    general_page.add(&storage_group(ctx));
    general_page.add(&system_group());

    prefs_window.add(&general_page);
    prefs_window.present();
}

// ---------------------------------------------------------------------------
// Dictation
// ---------------------------------------------------------------------------

fn dictation_group(
    ctx: &AppContext,
    config: &AppConfig,
    prefs_window: &adw::PreferencesWindow,
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder().title("Dictation").build();

    // Model selector — compact names: "Base (141 MB)".
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
    let model_name_refs: Vec<&str> = model_names.iter().map(String::as_str).collect();

    let loaded = ctx.worker.loaded_model();
    let model_row = adw::ComboRow::builder()
        .title("Whisper Model")
        .subtitle(match loaded {
            Some(ref name) if *name == config.model => "Loaded and ready".to_string(),
            _ if model_registry::find_model(&config.model)
                .is_some_and(|m| mgr.is_downloaded(m)) =>
            {
                "Downloaded".to_string()
            }
            _ => "Not downloaded yet \u{2014} choose a model to download it".to_string(),
        })
        .model(&gtk::StringList::new(&model_name_refs))
        .build();
    if let Some(idx) = all_models.iter().position(|m| m.name == config.model) {
        model_row.set_selected(idx as u32);
    }
    {
        let ctx = ctx.clone();
        let prefs_window = prefs_window.clone();
        model_row.connect_selected_notify(move |row| {
            let Some(model) = all_models.get(row.selected() as usize) else {
                return;
            };
            info!("Model preference changed to '{}'", model.name);
            let name = pretty_model_name(model.name);
            let row = row.clone();
            let prefs_window = prefs_window.clone();
            models::activate_model(&ctx, model, move |event| {
                row.set_subtitle(&models::describe(&event, &name));
                match event {
                    ModelEvent::Ready => row.set_subtitle("Loaded and ready"),
                    ModelEvent::Failed(ref e) => prefs_window.add_toast(adw::Toast::new(e)),
                    _ => {}
                }
            });
        });
    }
    group.add(&model_row);

    // Language: detected automatically (among the languages the user
    // speaks), or always the same one.
    let mut languages: Vec<(&str, &str)> = Language::all()
        .iter()
        .map(|l| (l.code(), l.name()))
        .collect();
    languages.sort_by_key(|(_, name)| *name);
    languages.insert(0, ("auto", "Automatic"));
    let lang_names: Vec<&str> = languages.iter().map(|(_, name)| *name).collect();

    let lang_row = adw::ComboRow::builder()
        .title("Language")
        .subtitle("Detection follows a switch of language between sentences")
        .model(&gtk::StringList::new(&lang_names))
        .expression(gtk::PropertyExpression::new(
            gtk::StringObject::static_type(),
            None::<&gtk::Expression>,
            "string",
        ))
        .enable_search(true)
        .build();
    let lang_idx = languages
        .iter()
        .position(|(code, _)| *code == config.language)
        .unwrap_or(0);
    lang_row.set_selected(lang_idx as u32);
    group.add(&lang_row);

    let spoken_row = adw::ActionRow::builder()
        .title("Languages You Speak")
        .subtitle(spoken_summary(&config.languages))
        .activatable(true)
        .visible(config.language == "auto")
        .build();
    spoken_row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    {
        let prefs_window = prefs_window.clone();
        spoken_row.connect_activated(move |row| {
            prefs_window.push_subpage(&languages_page(row));
        });
    }
    group.add(&spoken_row);
    {
        let spoken_row = spoken_row.clone();
        lang_row.connect_selected_notify(move |row| {
            if let Some((code, _)) = languages.get(row.selected() as usize) {
                save(|c| c.language = code.to_string());
                spoken_row.set_visible(*code == "auto");
                info!("Language preference changed to '{code}'");
            }
        });
    }

    // The dictation shortcut, recorded from the keys the user presses.
    let shortcut_label = shortcut::Keycaps::new(false);
    let hotkey_row = adw::ActionRow::builder()
        .title("Dictation Shortcut")
        .subtitle(&config.hotkey)
        .activatable(true)
        .build();
    show_hotkey(&hotkey_row, &shortcut_label, &config.hotkey);
    hotkey_row.add_suffix(&shortcut_label.widget);
    hotkey_row.add_suffix(&gtk::Image::from_icon_name("document-edit-symbolic"));
    {
        let ctx = ctx.clone();
        let prefs_window = prefs_window.clone();
        let shortcut_label = shortcut_label.clone();
        hotkey_row.connect_activated(move |row| {
            // The current shortcut must not start dictation (or be kept
            // from this window) while a new one is being pressed.
            ctx.send(AppAction::SuspendHotkey);
            let row = row.clone();
            let shortcut_label = shortcut_label.clone();
            let toast_window = prefs_window.clone();
            let ctx_closed = ctx.clone();
            shortcut::ask_for_shortcut(
                &prefs_window,
                move |hotkey| {
                    save(|c| c.hotkey = hotkey.clone());
                    show_hotkey(&row, &shortcut_label, &hotkey);
                    info!("Hotkey set to {hotkey}");
                    toast_window.add_toast(adw::Toast::new(&format!("Shortcut set to {hotkey}")));
                },
                move || ctx_closed.send(AppAction::HotkeyChanged),
            );
        });
    }
    group.add(&hotkey_row);

    let keep_keys_row = adw::SwitchRow::builder()
        .title("Keep the Shortcut From Other Apps")
        .subtitle("So pressing it does not also type into the app you are using (a space, for Super+Shift+Space)")
        .active(config.block_hotkey_keys)
        .build();
    {
        let ctx = ctx.clone();
        keep_keys_row.connect_active_notify(move |row| {
            let active = row.is_active();
            save(|c| c.block_hotkey_keys = active);
            ctx.send(AppAction::HotkeyChanged);
        });
    }
    group.add(&keep_keys_row);

    let auto_paste_row = adw::SwitchRow::builder()
        .title("Auto-paste to active window")
        .subtitle("Insert the text where the cursor is. It is always copied to the clipboard too")
        .active(config.auto_paste)
        .build();
    group.add(&auto_paste_row);

    let live_typing_row = adw::SwitchRow::builder()
        .title("Type While You Speak")
        .subtitle("Words appear at the cursor as you talk, and the last ones are corrected as recognition improves. Off: the text is inserted when you stop")
        .active(config.live_typing)
        .sensitive(config.auto_paste)
        .build();
    live_typing_row.connect_active_notify(|row| {
        let active = row.is_active();
        save(|c| c.live_typing = active);
    });
    group.add(&live_typing_row);
    auto_paste_row.connect_active_notify(move |row| {
        let active = row.is_active();
        save(|c| c.auto_paste = active);
        live_typing_row.set_sensitive(active);
    });

    let shortcuts = [
        (PasteShortcut::CtrlV, "Ctrl+V"),
        (PasteShortcut::CtrlShiftV, "Ctrl+Shift+V (terminals)"),
        (PasteShortcut::ShiftInsert, "Shift+Insert"),
    ];
    let shortcut_names: Vec<&str> = shortcuts.iter().map(|(_, name)| *name).collect();
    let shortcut_row = adw::ComboRow::builder()
        .title("Paste shortcut")
        .subtitle("For GNOME and KDE on Wayland")
        .model(&gtk::StringList::new(&shortcut_names))
        .build();
    if let Some(idx) = shortcuts
        .iter()
        .position(|(s, _)| *s == config.paste_shortcut)
    {
        shortcut_row.set_selected(idx as u32);
    }
    shortcut_row.connect_selected_notify(move |row| {
        if let Some((shortcut, _)) = shortcuts.get(row.selected() as usize) {
            save(|c| c.paste_shortcut = *shortcut);
        }
    });
    group.add(&shortcut_row);

    // Microphone: the system default (which follows the desktop's sound
    // settings) or a specific sound card.
    let devices = CpalCapture::new()
        .map(|c| c.list_input_devices())
        .unwrap_or_default();
    let mut device_names = vec!["System default".to_string()];
    device_names.extend(devices.iter().map(|d| d.label.clone()));
    let device_name_refs: Vec<&str> = device_names.iter().map(String::as_str).collect();
    let device_row = adw::ComboRow::builder()
        .title("Microphone")
        .subtitle("The system default follows your desktop's sound settings")
        .model(&gtk::StringList::new(&device_name_refs))
        .build();
    let selected = config
        .input_device
        .as_ref()
        .and_then(|id| devices.iter().position(|d| d.id == *id))
        .map(|i| i + 1)
        .unwrap_or(0);
    device_row.set_selected(selected as u32);
    {
        let ctx = ctx.clone();
        device_row.connect_selected_notify(move |row| {
            let choice = match row.selected() as usize {
                0 => None,
                i => devices.get(i - 1).map(|d| d.id.clone()),
            };
            info!("Microphone set to {choice:?}");
            save(|c| c.input_device = choice);
            ctx.send(AppAction::SettingsChanged);
        });
    }
    group.add(&device_row);

    group
}

/// Show `hotkey` in the shortcut row.
fn show_hotkey(row: &adw::ActionRow, keycaps: &shortcut::Keycaps, hotkey: &str) {
    keycaps.set(hotkey);
    row.set_subtitle("Starts and stops dictation in any app. Click to change it");
}

/// "English, Polish, German", or a hint when no language was chosen.
fn spoken_summary(codes: &[String]) -> String {
    let names: Vec<&str> = codes
        .iter()
        .filter_map(|code| Language::from_code(code))
        .map(|l| l.name())
        .collect();
    if names.is_empty() {
        "Any language. Choose yours: detection is far more reliable, especially with an accent"
            .to_string()
    } else {
        names.join(", ")
    }
}

/// The languages to detect among, with a search. Changes are saved at once
/// and shown in `summary_row`.
fn languages_page(summary_row: &adw::ActionRow) -> adw::NavigationPage {
    let chosen = Rc::new(RefCell::new(AppConfig::load().languages));

    let search = gtk::SearchEntry::builder()
        .placeholder_text("Search languages")
        .build();
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();

    let mut all: Vec<&Language> = Language::all().iter().collect();
    all.sort_by_key(|l| l.name());
    for lang in all {
        let code = lang.code();
        let check = gtk::CheckButton::builder()
            .active(chosen.borrow().iter().any(|c| c == code))
            .valign(gtk::Align::Center)
            .build();
        let row = adw::ActionRow::builder()
            .title(lang.name())
            .activatable_widget(&check)
            .build();
        row.add_prefix(&check);
        let chosen = chosen.clone();
        let summary_row = summary_row.clone();
        check.connect_toggled(move |check| {
            {
                let mut chosen = chosen.borrow_mut();
                chosen.retain(|c| c != code);
                if check.is_active() {
                    chosen.push(code.to_string());
                }
            }
            let languages = chosen.borrow().clone();
            summary_row.set_subtitle(&spoken_summary(&languages));
            save(|c| c.languages = languages);
        });
        list.append(&row);
    }
    {
        let search = search.clone();
        list.set_filter_func(move |row| {
            let query = search.text().to_lowercase();
            query.is_empty()
                || row
                    .downcast_ref::<adw::ActionRow>()
                    .is_some_and(|r| r.title().to_lowercase().contains(&query))
        });
    }
    {
        let list = list.clone();
        search.connect_search_changed(move |_| list.invalidate_filter());
    }

    let intro = gtk::Label::builder()
        .label(
            "Linux Whisper detects which of these languages each sentence is in. \
             Choosing yours makes that much more reliable, especially with an accent. \
             With none chosen, any language can be detected.\n\nLanguages are recognised \
             best with the Small model or larger.",
        )
        .wrap(true)
        .xalign(0.0)
        .css_classes(["dim-label"])
        .build();
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(24)
        .margin_start(12)
        .margin_end(12)
        .build();
    content.append(&intro);
    content.append(&search);
    content.append(&list);

    let toolbar = adw::ToolbarView::builder()
        .content(
            &gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .vexpand(true)
                .child(&adw::Clamp::builder().child(&content).build())
                .build(),
        )
        .build();
    toolbar.add_top_bar(&adw::HeaderBar::new());

    // Typing anywhere on the page searches.
    search.set_key_capture_widget(Some(&toolbar));

    adw::NavigationPage::builder()
        .title("Languages You Speak")
        .child(&toolbar)
        .build()
}

// ---------------------------------------------------------------------------
// Text formatting
// ---------------------------------------------------------------------------

fn formatting_group(
    ctx: &AppContext,
    config: &AppConfig,
    prefs_window: &adw::PreferencesWindow,
) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("Text Formatting")
        .build();

    let tidy_row = adw::SwitchRow::builder()
        .title("Tidy up text")
        .subtitle("Capital letters, spacing around punctuation, and paragraphs at long pauses")
        .active(config.format.enabled)
        .build();
    tidy_row.connect_active_notify(|row| {
        let active = row.is_active();
        save(|c| c.format.enabled = active);
    });
    group.add(&tidy_row);

    let llm_row = adw::SwitchRow::builder()
        .title("AI-powered formatting")
        .subtitle(
            "Uses a local language model (~490 MB download) to punctuate and lay out \
             dictated text. Adds 1\u{2013}2 s (GPU) or 5\u{2013}9 s (CPU) per dictation.",
        )
        .active(config.format.llm_enabled)
        .build();
    {
        let ctx = ctx.clone();
        let prefs_window = prefs_window.clone();
        llm_row.connect_active_notify(move |row| {
            let enabled = row.is_active();
            save(|c| c.format.llm_enabled = enabled);
            info!("AI formatting set to {enabled}");
            if !enabled {
                return;
            }
            let row = row.clone();
            let prefs_window = prefs_window.clone();
            models::activate_formatter(&ctx, move |event| {
                row.set_subtitle(&models::describe(&event, "the AI model"));
                match event {
                    ModelEvent::Ready => row.set_subtitle(
                        "Model ready \u{2014} formatting applies to the next dictation",
                    ),
                    ModelEvent::Failed(ref e) => {
                        // Don't leave the switch on when it cannot work.
                        save(|c| c.format.llm_enabled = false);
                        row.set_active(false);
                        prefs_window.add_toast(adw::Toast::new(e));
                    }
                    _ => {}
                }
            });
        });
    }
    group.add(&llm_row);

    group
}

// ---------------------------------------------------------------------------
// Appearance
// ---------------------------------------------------------------------------

fn appearance_group(ctx: &AppContext, config: &AppConfig) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder().title("Appearance").build();

    let themes = [Theme::System, Theme::Light, Theme::Dark];
    let theme_row = adw::ComboRow::builder()
        .title("Theme")
        .model(&gtk::StringList::new(&["System", "Light", "Dark"]))
        .build();
    if let Some(idx) = themes.iter().position(|t| *t == config.theme) {
        theme_row.set_selected(idx as u32);
    }
    theme_row.connect_selected_notify(move |row| {
        if let Some(&theme) = themes.get(row.selected() as usize) {
            save(|c| c.theme = theme);
            crate::apply_theme(theme);
        }
    });
    group.add(&theme_row);

    let confidence_row = adw::SwitchRow::builder()
        .title("Show confidence scores")
        .subtitle("Show how sure the model was about each transcribed segment")
        .active(config.show_confidence)
        .build();
    {
        let ctx = ctx.clone();
        confidence_row.connect_active_notify(move |row| {
            let active = row.is_active();
            save(|c| c.show_confidence = active);
            ctx.send(AppAction::SettingsChanged);
        });
    }
    group.add(&confidence_row);

    let background_row = adw::SwitchRow::builder()
        .title("Keep running when the window is closed")
        .subtitle("So the hotkey keeps working. Quit from the menu or the tray icon")
        .active(config.minimize_to_tray)
        .build();
    background_row.connect_active_notify(|row| {
        let active = row.is_active();
        save(|c| c.minimize_to_tray = active);
    });
    group.add(&background_row);

    group
}

// ---------------------------------------------------------------------------
// Model storage
// ---------------------------------------------------------------------------

fn storage_group(ctx: &AppContext) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("Model Storage")
        .description("Manage downloaded whisper models")
        .build();

    let mgr = ModelManager::new(AppConfig::models_dir());
    let downloaded = mgr.list_downloaded();
    let total_size: u64 = downloaded.iter().map(|m| m.size_bytes).sum();

    // Expander row that shows downloaded models when clicked.
    let storage_expander = adw::ExpanderRow::builder()
        .title("Downloaded Models")
        .subtitle(format!(
            "{} model(s) \u{2014} {}",
            downloaded.len(),
            format_bytes(total_size)
        ))
        .show_enable_switch(false)
        .build();

    let loaded = ctx.worker.loaded_model();
    for model in &downloaded {
        let in_use = loaded.as_deref() == Some(model.name);
        let row = adw::ActionRow::builder()
            .title(pretty_model_name(model.name))
            .subtitle(if in_use {
                format!("{} \u{2014} in use", format_bytes(model.size_bytes))
            } else {
                format_bytes(model.size_bytes)
            })
            .build();

        let delete_btn = gtk::Button::builder()
            .icon_name("edit-delete-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular", "error"])
            .tooltip_text("Delete model")
            .build();

        let model_ref = *model;
        let row_ref = row.clone();
        let expander_ref = storage_expander.clone();

        delete_btn.connect_clicked(move |btn| {
            let mgr = ModelManager::new(AppConfig::models_dir());
            match mgr.delete(model_ref) {
                Ok(()) => {
                    info!("Deleted model '{}'", model_ref.name);
                    btn.set_visible(false);
                    row_ref.set_subtitle("Deleted");
                    row_ref.set_sensitive(false);

                    let remaining = mgr.list_downloaded();
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

    group.add(&storage_expander);

    let llm_mgr = LlmModelManager::new(AppConfig::llm_models_dir());
    if llm_mgr.is_ready() {
        let llm_row = adw::ActionRow::builder()
            .title("AI formatting model")
            .subtitle("Qwen2.5 0.5B")
            .build();
        let delete_btn = gtk::Button::builder()
            .icon_name("edit-delete-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular", "error"])
            .tooltip_text("Delete model")
            .build();
        let row_ref = llm_row.clone();
        delete_btn.connect_clicked(move |btn| match llm_mgr.delete_all() {
            Ok(()) => {
                save(|c| c.format.llm_enabled = false);
                btn.set_visible(false);
                row_ref.set_subtitle("Deleted \u{2014} AI formatting turned off");
                row_ref.set_sensitive(false);
            }
            Err(e) => error!("Failed to delete the AI model: {e}"),
        });
        llm_row.add_suffix(&delete_btn);
        group.add(&llm_row);
    }

    group
}

// ---------------------------------------------------------------------------
// System
// ---------------------------------------------------------------------------

/// What the global hotkey and auto-paste need from the system, and whether
/// this system provides it.
fn system_group() -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder().title("System").build();

    let hotkey_ok = check_input_group_membership();
    let hotkey_row = adw::ActionRow::builder()
        .title("Global hotkey")
        .subtitle(if hotkey_ok {
            "You are in the input group, so the hotkey can read the keyboard"
        } else {
            "Needs the input group: sudo usermod -aG input $USER, then log out and back in"
        })
        .subtitle_selectable(true)
        .build();
    hotkey_row.add_prefix(&status_icon(hotkey_ok));
    group.add(&hotkey_row);

    let paste_ok = UinputPaster::is_permitted();
    let paste_row = adw::ActionRow::builder()
        .title("Virtual keyboard")
        .subtitle(if paste_ok {
            "Ready: text is pasted on GNOME and KDE, and the shortcut is kept from other apps"
        } else {
            "Needs access to /dev/uinput, which the udev rule installed with \
             Linux Whisper grants. Without it the text is only copied to the clipboard \
             on GNOME and KDE, and the shortcut also reaches the app you are using"
        })
        .build();
    paste_row.add_prefix(&status_icon(paste_ok));
    group.add(&paste_row);

    group
}

fn status_icon(ok: bool) -> gtk::Image {
    let icon = gtk::Image::from_icon_name(if ok {
        "emblem-ok-symbolic"
    } else {
        "dialog-warning-symbolic"
    });
    icon.add_css_class(if ok { "success" } else { "warning" });
    icon
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Make model names more human-friendly.
/// "base" → "Base", "tiny.en" → "Tiny EN", "large-v3" → "Large V3"
pub fn pretty_model_name(name: &str) -> String {
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
pub fn format_bytes(bytes: u64) -> String {
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
