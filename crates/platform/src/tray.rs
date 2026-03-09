use ksni::menu::{StandardItem, MenuItem};
use ksni::{ToolTip, TrayMethods};
use tokio::sync::mpsc;

/// Re-export the ksni Handle type for use by the app crate.
pub type TrayHandle = ksni::Handle<LinuxWhisperTray>;

/// Re-export the ksni Error type for use by the app crate.
pub type TrayError = ksni::Error;

/// Visual state of the system tray icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayState {
    Idle,
    Recording,
    Transcribing,
}

/// Actions that can be triggered from the tray menu or left-click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    ToggleRecording,
    Preferences,
    About,
    Quit,
}

/// The ksni tray implementation for Linux Whisper.
pub struct LinuxWhisperTray {
    /// Whether the tray is currently in recording state.
    pub recording: bool,
    /// Status text shown in the tooltip description.
    pub status_text: String,
    action_tx: mpsc::UnboundedSender<TrayAction>,
}

impl ksni::Tray for LinuxWhisperTray {
    fn id(&self) -> String {
        "linux-whisper".into()
    }

    fn title(&self) -> String {
        "Linux Whisper".into()
    }

    fn icon_name(&self) -> String {
        if self.recording {
            "media-record-symbolic".into()
        } else {
            "audio-input-microphone-symbolic".into()
        }
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: "Linux Whisper".into(),
            description: self.status_text.clone(),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.action_tx.send(TrayAction::ToggleRecording);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let record_label = if self.recording {
            "Stop Recording"
        } else {
            "Record"
        };
        let record_icon = if self.recording {
            "media-playback-stop-symbolic"
        } else {
            "media-record-symbolic"
        };

        vec![
            StandardItem {
                label: record_label.into(),
                icon_name: record_icon.into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.action_tx.send(TrayAction::ToggleRecording);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Preferences…".into(),
                icon_name: "preferences-other-symbolic".into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.action_tx.send(TrayAction::Preferences);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "About".into(),
                icon_name: "help-about-symbolic".into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.action_tx.send(TrayAction::About);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit-symbolic".into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.action_tx.send(TrayAction::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Spawn the system tray icon on the tokio runtime.
///
/// Returns a handle that can be used to update the tray state (icon, tooltip).
/// The `action_tx` sender is used to communicate tray actions back to the GTK
/// main loop.
pub async fn spawn_tray(
    action_tx: mpsc::UnboundedSender<TrayAction>,
) -> Result<ksni::Handle<LinuxWhisperTray>, ksni::Error> {
    let tray = LinuxWhisperTray {
        recording: false,
        status_text: "Ready".into(),
        action_tx,
    };
    tray.spawn().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_state_debug() {
        assert_eq!(format!("{:?}", TrayState::Idle), "Idle");
        assert_eq!(format!("{:?}", TrayState::Recording), "Recording");
        assert_eq!(format!("{:?}", TrayState::Transcribing), "Transcribing");
    }

    #[test]
    fn tray_action_debug() {
        assert_eq!(format!("{:?}", TrayAction::ToggleRecording), "ToggleRecording");
        assert_eq!(format!("{:?}", TrayAction::Preferences), "Preferences");
        assert_eq!(format!("{:?}", TrayAction::About), "About");
        assert_eq!(format!("{:?}", TrayAction::Quit), "Quit");
    }

    #[test]
    fn tray_state_equality() {
        assert_eq!(TrayState::Idle, TrayState::Idle);
        assert_ne!(TrayState::Idle, TrayState::Recording);
    }

    #[test]
    fn tray_action_equality() {
        assert_eq!(TrayAction::Quit, TrayAction::Quit);
        assert_ne!(TrayAction::Quit, TrayAction::Preferences);
    }

    #[test]
    fn tray_impl_id() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let tray = LinuxWhisperTray {
            recording: false,
            status_text: "Ready".into(),
            action_tx: tx,
        };
        use ksni::Tray;
        assert_eq!(tray.id(), "linux-whisper");
    }

    #[test]
    fn tray_impl_icon_name_idle() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let tray = LinuxWhisperTray {
            recording: false,
            status_text: "Ready".into(),
            action_tx: tx,
        };
        use ksni::Tray;
        assert_eq!(tray.icon_name(), "audio-input-microphone-symbolic");
    }

    #[test]
    fn tray_impl_icon_name_recording() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let tray = LinuxWhisperTray {
            recording: true,
            status_text: "Recording...".into(),
            action_tx: tx,
        };
        use ksni::Tray;
        assert_eq!(tray.icon_name(), "media-record-symbolic");
    }

    #[test]
    fn tray_impl_title() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let tray = LinuxWhisperTray {
            recording: false,
            status_text: "Ready".into(),
            action_tx: tx,
        };
        use ksni::Tray;
        assert_eq!(tray.title(), "Linux Whisper");
    }

    #[test]
    fn tray_impl_tooltip() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let tray = LinuxWhisperTray {
            recording: false,
            status_text: "Ready".into(),
            action_tx: tx,
        };
        use ksni::Tray;
        let tip = tray.tool_tip();
        assert_eq!(tip.title, "Linux Whisper");
        assert_eq!(tip.description, "Ready");
    }

    #[test]
    fn tray_activate_sends_toggle() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut tray = LinuxWhisperTray {
            recording: false,
            status_text: "Ready".into(),
            action_tx: tx,
        };
        use ksni::Tray;
        tray.activate(0, 0);
        let action = rx.try_recv().unwrap();
        assert_eq!(action, TrayAction::ToggleRecording);
    }

    #[test]
    fn tray_menu_has_expected_items() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let tray = LinuxWhisperTray {
            recording: false,
            status_text: "Ready".into(),
            action_tx: tx,
        };
        use ksni::Tray;
        let menu = tray.menu();
        // Record, Separator, Preferences, About, Separator, Quit = 6 items
        assert_eq!(menu.len(), 6);
    }
}
