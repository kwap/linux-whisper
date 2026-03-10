mod services;
mod ui;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};
use tracing::{error, info, warn};

use linux_whisper_audio::capture::{AudioCapture, CpalCapture};
use linux_whisper_core::config::AppConfig;
use linux_whisper_core::model::Transcript;
use linux_whisper_platform::hotkey::{EvdevHotkeyManager, HotkeyEvent, HotkeyManager};
use linux_whisper_platform::tray::{spawn_tray, TrayAction, TrayHandle};
use linux_whisper_whisper::model_manager::ModelManager;
use linux_whisper_whisper::model_registry;
use linux_whisper_whisper::worker::WhisperWorker;

use services::dictation::DictationService;
use ui::window::MainWindow;

const APP_ID: &str = "com.linuxwhisper.LinuxWhisper";

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
        info!("Linux Whisper is already running — exiting duplicate instance");
        return;
    }

    // Create a tokio runtime and leak it so it lives for the entire process.
    let rt = Box::leak(Box::new(
        tokio::runtime::Runtime::new().expect("failed to create tokio runtime"),
    ));
    let tokio_handle = rt.handle().clone();

    // Load persisted configuration.
    let config = AppConfig::load();
    info!(
        "Config: model={}, language={}, hotkey={}",
        config.model, config.language, config.hotkey
    );

    // Create the whisper worker (background inference thread).
    let worker = WhisperWorker::new();

    // Auto-load the configured model (or fall back to default).
    let mgr = ModelManager::new(AppConfig::models_dir());
    let target_model = model_registry::find_model(&config.model).unwrap_or_else(|| {
        warn!(
            "Configured model '{}' not in registry; falling back to default",
            config.model
        );
        model_registry::default_model()
    });

    if mgr.is_downloaded(target_model) {
        let model_path = mgr.model_path(target_model);
        info!(
            "Auto-loading model '{}' from {}",
            target_model.name,
            model_path.display()
        );
        let worker_clone = worker.clone();
        rt.block_on(async move {
            if let Err(e) = worker_clone.load_model(model_path).await {
                error!("Failed to auto-load model: {e}");
            }
        });
    } else {
        warn!(
            "Model '{}' not downloaded — recording will fail until a model is loaded",
            target_model.name
        );
    }

    // App was already created and registered above for single-instance check.
    let worker_for_activate = worker.clone();
    let tokio_handle_for_activate = tokio_handle.clone();

    app.connect_activate(move |app| {
        on_activate(
            app,
            worker_for_activate.clone(),
            tokio_handle_for_activate.clone(),
        );
    });

    // Hold the application so it doesn't exit when no windows are open.
    // The guard must be kept alive for the lifetime of the application.
    let _hold_guard = app.hold();

    // Run with empty args — GTK parses std::env::args internally.
    app.run_with_args::<String>(&[]);
}

/// Called once when the GTK application activates.
///
/// Sets up the tray icon, audio capture, and bridges tray actions into the GTK
/// main loop via a polling timer.
fn on_activate(
    app: &adw::Application,
    worker: WhisperWorker,
    tokio_handle: tokio::runtime::Handle,
) {
    // Create audio capture.
    let capture = match CpalCapture::new() {
        Ok(c) => {
            let device_name = c
                .default_device_name()
                .unwrap_or_else(|| "<unknown>".into());
            info!("Audio capture ready — default device: {device_name}");
            Rc::new(RefCell::new(c))
        }
        Err(e) => {
            error!("Failed to create audio capture: {e}");
            return;
        }
    };

    // Channel for tray→GTK communication (std::sync so we can poll from glib).
    let (action_tx, action_rx) = std_mpsc::channel::<TrayAction>();

    // We need a tokio::sync::mpsc sender for the tray (which is Send + 'static).
    let (tray_tx, mut tray_rx) = tokio::sync::mpsc::unbounded_channel::<TrayAction>();

    // Forward from tokio channel to std channel in a background task.
    let action_tx_for_bridge = action_tx.clone();
    tokio_handle.spawn(async move {
        while let Some(action) = tray_rx.recv().await {
            if action_tx_for_bridge.send(action).is_err() {
                break;
            }
        }
    });

    // Spawn the tray icon on the tokio runtime.
    let tray_handle = match tokio_handle.block_on(spawn_tray(tray_tx)) {
        Ok(handle) => {
            info!("System tray icon spawned");
            Arc::new(handle)
        }
        Err(e) => {
            error!("Failed to spawn system tray: {e}");
            return;
        }
    };

    // Set up global hotkeys via evdev.
    {
        let config = AppConfig::load();
        let action_tx_for_hotkey = action_tx.clone();
        let (hotkey_tx, mut hotkey_rx) = tokio::sync::mpsc::channel::<HotkeyEvent>(16);

        let mut hotkey_mgr = EvdevHotkeyManager::new();
        hotkey_mgr.set_event_sender(hotkey_tx);

        match hotkey_mgr.bind(&config.hotkey) {
            Ok(()) => {
                info!("Global hotkey '{}' bound successfully", config.hotkey);
                // Forward hotkey events → TrayAction::ToggleRecording on the action channel.
                tokio_handle.spawn(async move {
                    while let Some(event) = hotkey_rx.recv().await {
                        if event == HotkeyEvent::Pressed {
                            if action_tx_for_hotkey
                                .send(TrayAction::ToggleRecording)
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                });
                // Leak the manager so the listener thread stays alive.
                std::mem::forget(hotkey_mgr);
            }
            Err(e) => {
                warn!(
                    "Failed to bind global hotkey '{}': {e}\n\
                     → To enable: sudo usermod -aG input $USER && reboot",
                    config.hotkey
                );
            }
        }
    }

    // State for recording toggle.
    let is_recording = Rc::new(RefCell::new(false));

    // Shared MainWindow — created on first ShowWindow request, reused thereafter.
    let main_window: Rc<RefCell<Option<MainWindow>>> = Rc::new(RefCell::new(None));

    // Poll the action channel from the GTK main loop (~50ms interval).
    let app_clone = app.clone();
    let tokio_handle_clone = tokio_handle.clone();
    let worker_clone = worker.clone();

    glib::timeout_add_local(Duration::from_millis(50), move || {
        while let Ok(action) = action_rx.try_recv() {
            match action {
                TrayAction::ToggleRecording => {
                    handle_toggle_recording(
                        &capture,
                        &is_recording,
                        &worker_clone,
                        &tokio_handle_clone,
                        &tray_handle,
                        &main_window,
                    );
                }
                TrayAction::ShowWindow => {
                    let mut win_opt = main_window.borrow_mut();
                    if let Some(ref win) = *win_opt {
                        win.present();
                    } else {
                        let win = MainWindow::new(
                            &app_clone,
                            worker_clone.clone(),
                            tokio_handle_clone.clone(),
                        );
                        win.present();
                        *win_opt = Some(win);
                    }
                }
                TrayAction::Preferences => {
                    ui::preferences::show_preferences(&tokio_handle_clone);
                }
                TrayAction::About => {
                    ui::about::show_about(gtk::Widget::NONE);
                }
                TrayAction::Quit => {
                    info!("Quit requested from tray");
                    app_clone.quit();
                    return glib::ControlFlow::Break;
                }
            }
        }
        glib::ControlFlow::Continue
    });
}

/// Handle the toggle-recording action from the tray.
fn handle_toggle_recording(
    capture: &Rc<RefCell<CpalCapture>>,
    is_recording: &Rc<RefCell<bool>>,
    worker: &WhisperWorker,
    tokio_handle: &tokio::runtime::Handle,
    tray_handle: &Arc<TrayHandle>,
    main_window: &Rc<RefCell<Option<MainWindow>>>,
) {
    let mut recording = is_recording.borrow_mut();

    if !*recording {
        // Start recording.
        let mut cap = capture.borrow_mut();
        let device_name = cap
            .default_device_name()
            .unwrap_or_else(|| "<unknown>".into());

        match cap.start_recording() {
            Ok(()) => {
                info!("Recording started from {device_name}");
                *recording = true;
                drop(cap);
                drop(recording);

                // Update tray icon to recording state.
                let handle = tray_handle.clone();
                let device = device_name.clone();
                tokio_handle.spawn(async move {
                    handle
                        .update(|tray| {
                            tray.recording = true;
                            tray.status_text = format!("Recording from {device}…");
                        })
                        .await;
                });
            }
            Err(e) => {
                error!("Failed to start recording: {e}");
            }
        }
    } else {
        // Stop recording and transcribe.
        let mut cap = capture.borrow_mut();
        match cap.stop_recording() {
            Ok(audio) => {
                info!(
                    "Recording stopped — {} samples captured",
                    audio.samples.len()
                );
                *recording = false;
                drop(cap);
                drop(recording);

                // Update tray to transcribing state.
                let handle = tray_handle.clone();
                tokio_handle.spawn(async move {
                    handle
                        .update(|tray| {
                            tray.recording = false;
                            tray.status_text = "Transcribing…".into();
                        })
                        .await;
                });

                // Spawn transcription on tokio.
                let worker = worker.clone();
                let handle = tray_handle.clone();
                let config = AppConfig::load();

                let language = match config.language.as_str() {
                    "auto" => None,
                    lang => Some(lang.to_string()),
                };
                let options = linux_whisper_whisper::engine::TranscribeOptions {
                    language,
                    translate: false,
                };

                // Channel to send transcription result back to GTK thread.
                let (result_tx, result_rx) = std_mpsc::channel::<Result<Transcript, String>>();

                tokio_handle.spawn(async move {
                    let result = worker.transcribe(audio, options).await;
                    match result {
                        Ok(transcript) => {
                            info!(
                                "Transcription complete: {} segment(s)",
                                transcript.segment_count()
                            );
                            let _ = result_tx.send(Ok(transcript));
                        }
                        Err(e) => {
                            error!("Transcription failed: {e}");
                            let _ = result_tx.send(Err(e.to_string()));
                        }
                    }

                    // Reset tray back to idle.
                    handle
                        .update(|tray| {
                            tray.recording = false;
                            tray.status_text = "Ready".into();
                        })
                        .await;
                });

                // Poll for the transcription result on the GTK thread.
                let config = AppConfig::load();
                let main_window = main_window.clone();
                glib::timeout_add_local(Duration::from_millis(100), move || {
                    match result_rx.try_recv() {
                        Ok(Ok(transcript)) => {
                            let text = transcript.full_text();
                            if text.is_empty() {
                                warn!("Transcription returned empty text");
                            } else {
                                // Auto-paste the text.
                                if let Err(e) = DictationService::auto_paste(&config, &text) {
                                    error!("Auto-paste failed: {e}");
                                }

                                // Populate the MainWindow transcript list if it exists.
                                if let Some(ref win) = *main_window.borrow() {
                                    win.clear_segments();
                                    for seg in &transcript.segments {
                                        win.add_segment_row(seg);
                                    }
                                    win.status_label.set_label(&format!(
                                        "Dictation — {} segment(s), {:.1}s",
                                        transcript.segment_count(),
                                        transcript.duration,
                                    ));
                                }
                            }
                            glib::ControlFlow::Break
                        }
                        Ok(Err(e)) => {
                            error!("Transcription error: {e}");
                            glib::ControlFlow::Break
                        }
                        Err(std_mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                        Err(std_mpsc::TryRecvError::Disconnected) => {
                            error!("Transcription channel disconnected");
                            glib::ControlFlow::Break
                        }
                    }
                });
            }
            Err(e) => {
                error!("Failed to stop recording: {e}");
                *recording = false;
            }
        }
    }
}
