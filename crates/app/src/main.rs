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

use linux_whisper_audio::capture::{AudioBuffer, AudioCapture, CpalCapture};
use linux_whisper_audio::resample::resample;
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
use ui::window::{Activity, MainWindow};

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

    // Ctrl+Z in a terminal must not stop the app while it holds the keyboards.
    linux_whisper_platform::hotkey::release_keyboards_on_stop(rt.handle());

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
    /// Recordings stopped and still being transcribed.
    transcribing: Cell<usize>,
    tray: Option<Arc<TrayHandle>>,
    /// Whether a tray host is running, i.e. whether the tray icon is visible.
    tray_visible: bool,
    window: RefCell<Option<MainWindow>>,
    hotkey: RefCell<Option<EvdevHotkeyManager>>,
    hotkey_problem_shown: Cell<bool>,
    /// This session's dictations and the text delivered for each, shown when
    /// the window is opened later.
    history: RefCell<Vec<(Transcript, String)>>,
}

impl Controller {
    fn start(ctx: AppContext, actions_rx: std_mpsc::Receiver<AppAction>) -> Rc<Self> {
        let config = AppConfig::load();
        info!(
            "Config: model={}, language={}, hotkey={}",
            config.model, config.language, config.hotkey
        );
        apply_theme(config.theme);
        ui::init_style();

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
            transcribing: Cell::new(0),
            tray,
            tray_visible,
            window: RefCell::new(None),
            hotkey: RefCell::new(None),
            hotkey_problem_shown: Cell::new(false),
            history: RefCell::new(Vec::new()),
        });

        controller.bind_hotkey();

        // Also reachable from outside, e.g. for a shortcut set in the
        // desktop's settings: gapplication action com.linuxwhisper.LinuxWhisper toggle-recording
        let toggle = gio::SimpleAction::new("toggle-recording", None);
        {
            let ctx = controller.ctx.clone();
            toggle.connect_activate(move |_, _| ctx.send(AppAction::ToggleRecording));
        }
        controller.ctx.app.add_action(&toggle);

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
                if let Some(ref win) = *self.window.borrow() {
                    win.refresh_hotkey();
                }
            }
            AppAction::SuspendHotkey => {
                // Dropping the manager stops its listener and releases the
                // keyboards.
                self.hotkey.borrow_mut().take();
            }
            AppAction::ModelChanged => {
                if let Some(ref win) = *self.window.borrow() {
                    win.sync_model_dropdown();
                    if !self.recording.get() {
                        win.set_ready_status();
                    }
                }
                self.set_tray_status(self.ready_text());
            }
            AppAction::SettingsChanged => {
                if let Some(ref mut capture) = *self.capture.borrow_mut() {
                    capture.set_input_device(AppConfig::load().input_device);
                }
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
            let win = MainWindow::new(&self.ctx);
            for (transcript, text) in self.history.borrow().iter() {
                win.add_transcript(transcript.clone(), text.clone());
            }
            // Opened while dictating: show it.
            if self.recording.get() {
                let device = self
                    .capture
                    .borrow()
                    .as_ref()
                    .and_then(|c| c.active_device_name());
                win.set_status(&format!(
                    "Recording from {}",
                    device.as_deref().unwrap_or("the microphone")
                ));
                win.set_activity(Activity::Recording);
            } else if self.transcribing.get() > 0 {
                win.set_activity(Activity::Transcribing);
            }
            win
        });
        win.present();
    }

    // -- Models --------------------------------------------------------------

    /// Load the configured whisper model (or offer to download one) and, if
    /// enabled, the AI formatter.
    fn load_models(self: &Rc<Self>, config: &AppConfig) {
        let mgr = ModelManager::new(AppConfig::models_dir());
        let configured = model_registry::find_model(&config.model).unwrap_or_else(|| {
            warn!(
                "Configured model '{}' not in registry; falling back to default",
                config.model
            );
            model_registry::default_model()
        });
        // If the configured model is gone (deleted, or never finished
        // downloading), use one that is on disk rather than none.
        let downloaded = mgr.list_downloaded();
        let target = if mgr.is_downloaded(configured) {
            Some(configured)
        } else {
            let default = model_registry::default_model();
            let fallback = downloaded
                .iter()
                .find(|m| m.name == default.name)
                .or(downloaded.first())
                .copied();
            if let Some(m) = fallback {
                warn!(
                    "Model '{}' is not downloaded; using '{}' instead",
                    configured.name, m.name
                );
            }
            fallback
        };

        if let Some(target) = target {
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
        manager.keep_keys_from_apps(config.block_hotkey_keys);

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
            if models::model_download_in_progress() {
                self.notify(
                    "record",
                    "The speech model is still downloading",
                    "Dictation starts working as soon as the download finishes.",
                );
            } else {
                self.show_window();
                if let Some(ref win) = *self.window.borrow() {
                    win.prompt_model_download();
                }
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
                self.set_recording_ui(true, format!("Recording from {device}"));
                self.show_recording_time();
            }
            Err(e) => {
                error!("Failed to start recording: {e}");
                self.notify("record", "Cannot record", &e.to_string());
            }
        }
    }

    fn stop_and_transcribe(self: &Rc<Self>) {
        let raw = {
            let mut capture = self.capture.borrow_mut();
            let Some(capture) = capture.as_mut() else {
                return;
            };
            capture.stop_recording_raw()
        };
        self.recording.set(false);

        let (samples, rate) = match raw {
            Ok(raw) => raw,
            Err(e) => {
                error!("Failed to stop recording: {e}");
                self.set_recording_ui(false, self.ready_text());
                return;
            }
        };

        let secs = samples.len() as f64 / rate.max(1) as f64;
        info!("Recording stopped — {secs:.1}s of audio");
        if secs < MIN_RECORDING_SECS {
            self.set_recording_ui(false, self.ready_text());
            return;
        }
        self.transcribing.set(self.transcribing.get() + 1);
        self.set_recording_ui(false, "Transcribing\u{2026}".into());

        let config = AppConfig::load();
        let options = linux_whisper_whisper::engine::TranscribeOptions::from_config(&config);

        // Resample, transcribe and format off the GTK thread.
        let (result_tx, result_rx) = std_mpsc::channel::<Result<(Transcript, String), String>>();
        let worker = self.ctx.worker.clone();
        let llm_worker = self.ctx.llm_worker.clone();
        self.ctx.rt.spawn(async move {
            let audio = tokio::task::spawn_blocking(move || AudioBuffer {
                samples: resample(&samples, rate, AudioBuffer::TARGET_SAMPLE_RATE),
                sample_rate: AudioBuffer::TARGET_SAMPLE_RATE,
            })
            .await;
            let Ok(audio) = audio else {
                let _ = result_tx.send(Err("resampling failed".into()));
                return;
            };

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
            let _ = result_tx.send(Ok((transcript, text)));
        });

        let this = Rc::clone(self);
        when_ready(result_rx, move |result| {
            this.finish_dictation(
                result.unwrap_or_else(|| Err("transcription stopped unexpectedly".into())),
            );
        });
    }

    /// Show a finished transcription and deliver its text.
    fn finish_dictation(self: &Rc<Self>, result: Result<(Transcript, String), String>) {
        self.transcribing
            .set(self.transcribing.get().saturating_sub(1));
        // A second recording may already be running; leave its status alone.
        let status = |text: String| {
            if !self.recording.get() {
                self.set_recording_ui(false, text);
            }
        };

        let (transcript, text) = match result {
            Ok(r) => r,
            Err(e) => {
                error!("Transcription failed: {e}");
                status("Transcription failed".into());
                self.notify("dictation", "Transcription failed", &e);
                return;
            }
        };
        status(self.ready_text());

        if text.is_empty() {
            // Keep whatever transcript the window shows.
            info!("No speech recognised");
            self.notify(
                "dictation",
                "No speech recognised",
                "Nothing was heard in the recording.",
            );
            return;
        }

        if let Some(ref win) = *self.window.borrow() {
            win.add_transcript(transcript.clone(), text.clone());
        }
        self.history.borrow_mut().push((transcript, text.clone()));

        // Decided now, not when recording stopped: typing into this app's own
        // window would put the text there (and a space would press its focused
        // button). The text is shown there and copied to the clipboard instead.
        let config = AppConfig::load();
        let own_window_focused = self
            .window
            .borrow()
            .as_ref()
            .is_some_and(|w| w.window.is_active());
        let insert = config.auto_paste && !own_window_focused;

        let (delivery_tx, delivery_rx) = std_mpsc::channel::<Delivery>();
        self.ctx.rt.spawn_blocking(move || {
            let config = AppConfig {
                auto_paste: insert,
                ..config
            };
            let _ = delivery_tx.send(DictationService::deliver(&config, &text));
        });

        let this = Rc::clone(self);
        when_ready(delivery_rx, move |delivery| match delivery {
            Some(Delivery::Typed(_) | Delivery::Pasted) => {}
            Some(Delivery::ClipboardOnly(reason)) => {
                info!("Text left on the clipboard: {reason}");
                if insert {
                    this.notify(
                        "dictation",
                        "Dictated text is on the clipboard",
                        &format!(
                            "Press Ctrl+V to paste it. It could not be pasted automatically: {reason}."
                        ),
                    );
                }
            }
            Some(Delivery::Failed(reason)) => {
                error!("Could not deliver the text: {reason}");
                this.notify("dictation", "Could not deliver the text", &reason);
            }
            None => error!("Text delivery stopped unexpectedly"),
        });
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
            if recording {
                win.set_status(&status);
                win.set_activity(Activity::Recording);
            } else if self.transcribing.get() > 0 {
                win.set_activity(Activity::Transcribing);
            } else {
                win.set_ready_status();
                win.set_activity(Activity::Idle);
            }
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

    /// Keep the window's recording time up to date while recording.
    fn show_recording_time(self: &Rc<Self>) {
        let this = Rc::clone(self);
        glib::timeout_add_local(Duration::from_millis(250), move || {
            if !this.recording.get() {
                return glib::ControlFlow::Break;
            }
            let secs = this
                .capture
                .borrow()
                .as_ref()
                .map(CpalCapture::recorded_secs);
            if let (Some(secs), Some(win)) = (secs, this.window.borrow().as_ref()) {
                if win.window.is_visible() {
                    win.update_time(secs);
                }
            }
            glib::ControlFlow::Continue
        });
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

/// Run `on_result` on the GTK thread once a background task sends its result
/// (`None` if the task ended without sending one).
fn when_ready<T: 'static>(rx: std_mpsc::Receiver<T>, on_result: impl FnOnce(Option<T>) + 'static) {
    let mut on_result = Some(on_result);
    glib::timeout_add_local(Duration::from_millis(50), move || {
        let result = match rx.try_recv() {
            Ok(value) => Some(value),
            Err(std_mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
            Err(std_mpsc::TryRecvError::Disconnected) => None,
        };
        if let Some(f) = on_result.take() {
            f(result);
        }
        glib::ControlFlow::Break
    });
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
pub fn tray_host_running() -> bool {
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
