//! Handles shared by the application controller and its windows.

use std::sync::mpsc as std_mpsc;

use linux_whisper_llm::worker::LlmWorker;
use linux_whisper_platform::tray::TrayAction;
use linux_whisper_whisper::worker::WhisperWorker;

/// Requests handled by the application controller on the GTK main thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppAction {
    ToggleRecording,
    ShowWindow,
    Preferences,
    About,
    Quit,
    /// The hotkey setting changed: bind the new one.
    HotkeyChanged,
    /// A whisper model finished loading.
    ModelChanged,
    /// Other settings changed (microphone, confidence display, ...).
    SettingsChanged,
}

impl From<TrayAction> for AppAction {
    fn from(action: TrayAction) -> Self {
        match action {
            TrayAction::ToggleRecording => AppAction::ToggleRecording,
            TrayAction::ShowWindow => AppAction::ShowWindow,
            TrayAction::Preferences => AppAction::Preferences,
            TrayAction::About => AppAction::About,
            TrayAction::Quit => AppAction::Quit,
        }
    }
}

/// Everything a window needs to talk to the rest of the application.
#[derive(Clone)]
pub struct AppContext {
    pub app: adw::Application,
    pub rt: tokio::runtime::Handle,
    pub worker: WhisperWorker,
    pub llm_worker: LlmWorker,
    pub actions: std_mpsc::Sender<AppAction>,
}

impl AppContext {
    /// Ask the controller to do something. Never fails: the controller lives
    /// as long as the application.
    pub fn send(&self, action: AppAction) {
        let _ = self.actions.send(action);
    }
}
