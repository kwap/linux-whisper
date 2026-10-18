mod context;
mod services;
mod ui;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};
use tracing::{error, info, warn};

use linux_whisper_audio::capture::{AudioCapture, CpalCapture};
use linux_whisper_core::config::{AppConfig, Theme};
use linux_whisper_core::format::tidy_text;
use linux_whisper_core::model::Transcript;
use linux_whisper_llm::model_manager::LlmModelManager;
use linux_whisper_llm::worker::LlmWorker;
use linux_whisper_platform::display::{self, DisplayServer};
use linux_whisper_platform::hotkey::{EvdevHotkeyManager, HotkeyEvent, HotkeyManager};
use linux_whisper_platform::text_inject::{Delivery, UinputPaster};
use linux_whisper_platform::tray::{spawn_tray, TrayAction, TrayHandle};
use linux_whisper_whisper::model_manager::ModelManager;
use linux_whisper_whisper::model_registry;
use linux_whisper_whisper::worker::WhisperWorker;

use context::{AppAction, AppContext};
use services::dictation::DictationService;
use services::models::{self, ModelEvent};
use ui::window::MainWindow;

const APP_ID: &str = "com.linuxwhisper.LinuxWhisper";

/// Recordings shorter than this are treated as an accidental double press.
const MIN_RECORDING_SECS: f64 = 0.3;

fn main() {
    // Initialize logging.
    tracing_subscriber::fmt().init();

    info!("Starting Linux Whisper v{}", env!("CARGO_PKG_VERSION"));

    // Initialize the i18n loader.
    let _ = &*linux_whisper_i18n::LANGUAGE_LOADER;

    // Build the GTK application early so we can check for existing instances
    // BEFORE doing expensive work (tokio runtime, model loading).
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::FLAGS_NONE)
        .build();

    if let Err(e) = app.register(gio::Cancellable::NONE) {
        error!("Failed to register application: {e}");
        return;
    }

    if app.is_remote() {
        // Starting the app again (e.g. from the applications menu) brings up
        // the window of the instance that is already running.
        info!("Linux Whisper is already running — showing its window");
        app.activate();
        return;
    }

    // Detect the display server while the process is still single-threaded:
    // the fallback detection may set WAYLAND_DISPLAY for child processes.
    info!("Display server: {}", display::detect());

    // Create a tokio runtime and leak it so it lives for the entire process.
    let rt = Box::leak(Box::new(
        tokio::runtime::Runtime::new().expect("failed to create tokio runtime"),
    ));

    let (actions_tx, actions_rx) = std_mpsc::channel::<AppAction>();
    let ctx = AppContext {
        app: app.clone(),
        rt: rt.handle().clone(),
        worker: WhisperWorker::new(),
        llm_worker: LlmWorker::new(),
        actions: actions_tx,
    };

    let controller: RefCell<Option<Rc<Controller>>> = RefCell::new(None);
    let actions_rx = RefCell::new(Some(actions_rx));
    app.connect_activate(move |_| {
        if let Some(ref c) = *controller.borrow() {
            c.show_window();
            return;
        }
        if let Some(rx) = actions_rx.borrow_mut().take() {
            *controller.borrow_mut() = Some(Controller::start(ctx.clone(), rx));
        }
    });

    // Hold the application so it doesn't exit when no windows are open.
    // The guard must be kept alive for the lifetime of the application.
    let _hold_guard = app.hold();

    // Run with empty args — GTK parses std::env::args internally.
    app.run_with_args::<String>(&[]);
}

/// Owns the long-lived state (recording, tray, hotkey, window) and handles
/// [`AppAction`]s on the GTK main thread.
struct Controller {
    ctx: AppContext,
    capture: RefCell<Option<CpalCapture>>,
    recording: Cell<bool>,
    tray: Option<Arc<TrayHandle>>,
    /// Whether a tray host is running, i.e. whether the tray icon is visible.
    tray_visible: bool,
    window: RefCell<Option<MainWindow>>,
    hotkey: RefCell<Option<EvdevHotkeyManager>>,
    hotkey_problem_shown: Cell<bool>,
    /// The last dictation, shown when the window is opened later.
    last_dictation: RefCell<Option<(Transcript, String)>>,
}

impl Controller {
    fn start(ctx: AppContext, actions_rx: std_mpsc::Receiver<AppAction>) -> Rc<Self> {
        let config = AppConfig::load();
        info!(
            "Config: model={}, language={}, hotkey={}",
            config.model, config.language, config.hotkey
        );
        apply_theme(config.theme);

        // Audio capture.
        let capture = match CpalCapture::new() {
            Ok(c) => {
                let device_name = c
                    .default_device_name()
                    .unwrap_or_else(|| "<unknown>".into());
                info!("Audio capture ready — default device: {device_name}");
                Some(c)
            }
            Err(e) => {
                error!("Failed to create audio capture: {e}");
                None
            }
        };

        // Tray icon. Tray actions are forwarded to the action channel.
        let (tray_tx, mut tray_rx) = tokio::sync::mpsc::unbounded_channel::<TrayAction>();
        let actions = ctx.actions.clone();
        ctx.rt.spawn(async move {
            while let Some(action) = tray_rx.recv().await {
                if actions.send(action.into()).is_err() {
                    break;
                }
            }
        });
        let tray = match ctx.rt.block_on(spawn_tray(tray_tx)) {
            Ok(handle) => {
                info!("System tray icon spawned");
                Some(Arc::new(handle))
            }
            Err(e) => {
                error!("Failed to spawn system tray: {e}");
                None
            }
        };
        let tray_visible = tray.is_some() && tray_host_running();
        if !tray_visible {
            warn!(
                "No system tray is available — keeping the window open instead. \
                 On GNOME, the AppIndicator extension adds tray support."
            );
        }

        let controller = Rc::new(Self {
            ctx,
            capture: RefCell::new(capture),
            recording: Cell::new(false),
            tray,
            tray_visible,
            window: RefCell::new(None),
            hotkey: RefCell::new(None),
            hotkey_problem_shown: Cell::new(false),
            last_dictation: RefCell::new(None),
        });

        controller.bind_hotkey();

        // Poll the action channel from the GTK main loop.
        {
            let controller = Rc::clone(&controller);
            glib::timeout_add_local(Duration::from_millis(50), move || {
                while let Ok(action) = actions_rx.try_recv() {
                    if !controller.handle(action) {
                        return glib::ControlFlow::Break;
                    }
                }
                glib::ControlFlow::Continue
            });
        }

        // Without a tray icon the window is the only way in.
        if !controller.tray_visible {
            controller.show_window();
        }
        controller.load_models(&config);

        // Create the virtual paste keyboard ahead of the first dictation:
        // the compositor needs a moment to pick up a new input device.
        if config.auto_paste
            && display::detect() == DisplayServer::Wayland
            && UinputPaster::is_permitted()
        {
            controller.ctx.rt.spawn_blocking(|| {
                if let Err(e) = UinputPaster::prepare() {
                    warn!("Virtual paste keyboard unavailable: {e}");
                }
            });
        }

        controller
    }

    /// Handle one action. Returns `false` once the application is quitting.
    fn handle(self: &Rc<Self>, action: AppAction) -> bool {
        match action {
            AppAction::ToggleRecording => self.toggle_recording(),
            AppAction::ShowWindow => self.show_window(),
            AppAction::Preferences => {
                let parent = self.window.borrow().as_ref().map(|w| w.window.clone());
                ui::preferences::show_preferences(&self.ctx, parent.as_ref());
            }
            AppAction::About => {
                let parent = self.window.borrow().as_ref().map(|w| w.window.clone());
                ui::about::show_about(parent.as_ref());
            }
            AppAction::Quit => {
                info!("Quit requested");
                self.ctx.app.quit();
                return false;
            }
            AppAction::HotkeyChanged => {
                self.hotkey_problem_shown.set(false);
                self.bind_hotkey();
            }
            AppAction::ModelChanged => {
                if let Some(ref win) = *self.window.borrow() {
                    win.sync_model_dropdown();
                }
                self.set_tray_status(self.ready_text());
            }
            AppAction::SettingsChanged => {
                if let Some(ref win) = *self.window.borrow() {
                    win.refresh_rows();
                }
            }
        }
        true
    }

    fn show_window(self: &Rc<Self>) {
        let mut win = self.window.borrow_mut();
        let win = win.get_or_insert_with(|| {
            let win = MainWindow::new(&self.ctx, self.tray_visible);
            if let Some((transcript, label)) = self.last_dictation.borrow().clone() {
                win.show_transcript(transcript, &label);
            }
            win
        });
        win.present();
    }

    // -- Models --------------------------------------------------------------

    /// Load the configured whisper model (or offer to download one) and, if
    /// enabled, the AI formatter.
    fn load_models(self: &Rc<Self>, config: &AppConfig) {
        let target = model_registry::find_model(&config.model).unwrap_or_else(|| {
            warn!(
                "Configured model '{}' not in registry; falling back to default",
                config.model
            );
            model_registry::default_model()
        });

        if ModelManager::new(AppConfig::models_dir()).is_downloaded(target) {
            info!("Loading model '{}'", target.name);
            self.set_tray_status(format!("Loading the {} model\u{2026}", target.name));
            let this = Rc::clone(self);
            models::activate_model(&self.ctx, target, move |event| match event {
                ModelEvent::Failed(e) => {
                    error!("Failed to load model: {e}");
                    this.notify("model", "The speech model could not be loaded", &e);
                }
                ModelEvent::Ready => info!("Model '{}' ready", target.name),
                _ => {}
            });
        } else {
            info!("No model downloaded yet — asking the user to download one");
            self.set_tray_status("No speech model yet".into());
            self.show_window();
            if let Some(ref win) = *self.window.borrow() {
                win.prompt_model_download();
            }
        }

        if config.format.llm_enabled {
            if LlmModelManager::new(AppConfig::llm_models_dir()).is_ready() {
                models::activate_formatter(&self.ctx, |event| {
                    if let ModelEvent::Failed(e) = event {
                        error!("Failed to load the AI formatter: {e}");
                    }
                });
            } else {
                warn!("AI formatting enabled but its model is not downloaded");
            }
        }
    }

    // -- Hotkey --------------------------------------------------------------

    fn bind_hotkey(&self) {
        let config = AppConfig::load();
        // Dropping the previous manager stops its listener thread.
        self.hotkey.borrow_mut().take();

        let (hotkey_tx, mut hotkey_rx) = tokio::sync::mpsc::channel::<HotkeyEvent>(16);
        let mut manager = EvdevHotkeyManager::new();
        manager.set_event_sender(hotkey_tx);

        match manager.bind(&config.hotkey) {
            Ok(()) => {
                info!("Global hotkey '{}' bound successfully", config.hotkey);
                let actions = self.ctx.actions.clone();
                self.ctx.rt.spawn(async move {
                    while let Some(event) = hotkey_rx.recv().await {
                        if event == HotkeyEvent::Pressed
                            && actions.send(AppAction::ToggleRecording).is_err()
                        {
                            break;
                        }
                    }
                });
                *self.hotkey.borrow_mut() = Some(manager);
            }
            Err(e) => {
                warn!("Failed to bind global hotkey '{}': {e}", config.hotkey);
                if !self.hotkey_problem_shown.replace(true) {
                    self.notify(
                        "hotkey",
                        "The global hotkey is not available",
                        &format!("{e}. Until then, record from the tray icon or the window."),
                    );
                }
            }
        }
    }

    // -- Dictation -----------------------------------------------------------

    fn toggle_recording(self: &Rc<Self>) {
        if self.recording.get() {
            self.stop_and_transcribe();
        } else {
            self.start_recording();
        }
    }

    fn start_recording(self: &Rc<Self>) {
        let config = AppConfig::load();

        // Without any model there is nothing to transcribe with.
        let model_on_disk = model_registry::find_model(&config.model)
            .is_some_and(|m| ModelManager::new(AppConfig::models_dir()).is_downloaded(m));
        if self.ctx.worker.loaded_model().is_none() && !model_on_disk {
            self.show_window();
            if let Some(ref win) = *self.window.borrow() {
                win.prompt_model_download();
            }
            return;
        }

        let mut capture = self.capture.borrow_mut();
        let Some(capture) = capture.as_mut() else {
            self.notify("record", "Cannot record", "No audio system is available.");
            return;
        };
        capture.set_input_device(config.input_device.clone());

        match capture.start_recording() {
            Ok(()) => {
                let device = capture
                    .active_device_name()
                    .unwrap_or_else(|| "microphone".into());
                info!("Recording started from {device}");
                self.recording.set(true);
                self.set_recording_ui(true, format!("Recording from {device}\u{2026}"));
            }
            Err(e) => {
                error!("Failed to start recording: {e}");
                self.notify("record", "Cannot record", &e.to_string());
            }
        }
    }

    fn stop_and_transcribe(self: &Rc<Self>) {
        let audio = {
            let mut capture = self.capture.borrow_mut();
            let Some(capture) = capture.as_mut() else {
                return;
            };
            capture.stop_recording()
        };
        self.recording.set(false);

        let audio = match audio {
            Ok(audio) => audio,
            Err(e) => {
                error!("Failed to stop recording: {e}");
                self.set_recording_ui(false, self.ready_text());
                return;
            }
        };

        let secs = audio.samples.len() as f64 / audio.sample_rate as f64;
        info!("Recording stopped — {secs:.1}s of audio");
        if secs < MIN_RECORDING_SECS {
            self.set_recording_ui(false, self.ready_text());
            return;
        }
        self.set_recording_ui(false, "Transcribing\u{2026}".into());

        let config = AppConfig::load();

        // When recording was stopped from this app's own window, typing the
        // text would go into that window (and a space would press its focused
        // button). The text is shown there and copied to the clipboard instead.
        let own_window_focused = self
            .window
            .borrow()
            .as_ref()
            .is_some_and(|w| w.window.is_active());
        let insert = config.auto_paste && !own_window_focused;

        let options = linux_whisper_whisper::engine::TranscribeOptions {
            language: (config.language != "auto").then(|| config.language.clone()),
            translate: false,
        };

        // Transcribe, format and deliver off the GTK thread; send the result back.
        let (result_tx, result_rx) =
            std_mpsc::channel::<Result<(Transcript, String, Delivery), String>>();
        let worker = self.ctx.worker.clone();
        let llm_worker = self.ctx.llm_worker.clone();
        self.ctx.rt.spawn(async move {
            let transcript = match worker.transcribe(audio, options).await {
                Ok(t) => t,
                Err(e) => {
                    let _ = result_tx.send(Err(e.to_string()));
                    return;
                }
            };
            info!(
                "Transcription complete: {} segment(s)",
                transcript.segment_count()
            );

            let basic = transcript.formatted_text(&config.format);
            let text = if config.format.llm_enabled && !basic.is_empty() {
                match llm_worker.format_text(transcript.full_text()).await {
                    Ok(formatted) => {
                        info!("AI formatting applied");
                        tidy_text(&formatted)
                    }
                    Err(e) => {
                        warn!("AI formatting failed, using basic formatting: {e}");
                        basic
                    }
                }
            } else {
                basic
            };

            let delivery = if text.is_empty() {
                Delivery::Failed("no speech".into())
            } else {
                let deliver_config = AppConfig {
                    auto_paste: insert,
                    ..config.clone()
                };
                let deliver_text = text.clone();
                tokio::task::spawn_blocking(move || {
                    DictationService::deliver(&deliver_config, &deliver_text)
                })
                .await
                .unwrap_or_else(|e| Delivery::Failed(e.to_string()))
            };
            let _ = result_tx.send(Ok((transcript, text, delivery)));
        });

        let this = Rc::clone(self);
        glib::timeout_add_local(Duration::from_millis(100), move || {
            match result_rx.try_recv() {
                Ok(result) => {
                    this.finish_dictation(result, insert);
                    glib::ControlFlow::Break
                }
                Err(std_mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(std_mpsc::TryRecvError::Disconnected) => {
                    this.finish_dictation(Err("transcription stopped unexpectedly".into()), insert);
                    glib::ControlFlow::Break
                }
            }
        });
    }

    /// Show the result of a dictation. `insert` says whether the text was
    /// meant to be inserted into the focused window.
    fn finish_dictation(
        &self,
        result: Result<(Transcript, String, Delivery), String>,
        insert: bool,
    ) {
        self.set_tray_status(self.ready_text());
        let (transcript, text, delivery) = match result {
            Ok(r) => r,
            Err(e) => {
                error!("Transcription failed: {e}");
                self.notify("dictation", "Transcription failed", &e);
                return;
            }
        };

        let label = format!(
            "Dictation \u{2014} {} segment(s), {:.1}s",
            transcript.segment_count(),
            transcript.duration
        );
        if let Some(ref win) = *self.window.borrow() {
            win.show_transcript(transcript.clone(), &label);
        }
        *self.last_dictation.borrow_mut() = Some((transcript, label));

        match delivery {
            Delivery::Typed(_) | Delivery::Pasted => {}
            Delivery::ClipboardOnly(reason) => {
                info!("Text left on the clipboard: {reason}");
                if insert {
                    self.notify(
                        "dictation",
                        "Dictated text is on the clipboard",
                        &format!(
                            "Press Ctrl+V to paste it. It could not be pasted automatically: {reason}."
                        ),
                    );
                }
            }
            Delivery::Failed(reason) if text.is_empty() => {
                info!("Nothing to deliver: {reason}");
                self.notify(
                    "dictation",
                    "No speech recognised",
                    "Nothing was heard in the recording.",
                );
            }
            Delivery::Failed(reason) => {
                error!("Could not deliver the text: {reason}");
                self.notify("dictation", "Could not deliver the text", &reason);
            }
        }
    }

    // -- Feedback ------------------------------------------------------------

    fn ready_text(&self) -> String {
        match self.ctx.worker.loaded_model() {
            Some(model) => format!("Ready \u{2014} {model} model"),
            None => "Ready".into(),
        }
    }

    fn set_recording_ui(&self, recording: bool, status: String) {
        if let Some(ref win) = *self.window.borrow() {
            win.set_recording_state(recording);
            win.status_label.set_label(&status);
        }
        if let Some(ref tray) = self.tray {
            let tray = Arc::clone(tray);
            self.ctx.rt.spawn(async move {
                tray.update(|t| {
                    t.recording = recording;
                    t.status_text = status;
                })
                .await;
            });
        }
    }

    fn set_tray_status(&self, status: String) {
        if let Some(ref tray) = self.tray {
            let tray = Arc::clone(tray);
            self.ctx.rt.spawn(async move {
                tray.update(|t| t.status_text = status).await;
            });
        }
    }

    /// Show a desktop notification, and a toast if the window is open.
    fn notify(&self, id: &str, title: &str, body: &str) {
        let notification = gio::Notification::new(title);
        notification.set_body(Some(body));
        self.ctx.app.send_notification(Some(id), &notification);
        if let Some(ref win) = *self.window.borrow() {
            if win.window.is_visible() {
                win.show_toast(title);
            }
        }
    }
}

/// Apply the theme preference to all windows.
pub fn apply_theme(theme: Theme) {
    let scheme = match theme {
        Theme::System => adw::ColorScheme::Default,
        Theme::Light => adw::ColorScheme::ForceLight,
        Theme::Dark => adw::ColorScheme::ForceDark,
    };
    adw::StyleManager::default().set_color_scheme(scheme);
}

/// Whether a StatusNotifier host (the desktop's tray) is running, so the tray
/// icon is actually visible. Stock GNOME has none.
fn tray_host_running() -> bool {
    let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) else {
        return false;
    };
    bus.call_sync(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "NameHasOwner",
        Some(&("org.kde.StatusNotifierWatcher",).to_variant()),
        Some(glib::VariantTy::new("(b)").expect("valid variant type")),
        gio::DBusCallFlags::NONE,
        1000,
        gio::Cancellable::NONE,
    )
    .ok()
    .and_then(|reply| reply.get::<(bool,)>())
    .is_some_and(|(owned,)| owned)
}
