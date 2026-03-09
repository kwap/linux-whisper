/// Visual state of the system tray icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayState {
    Idle,
    Recording,
    Transcribing,
}

/// Actions that can be triggered from the tray menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    ToggleRecording,
    ShowWindow,
    Preferences,
    Quit,
}

/// Trait for managing the system tray icon and menu.
#[cfg_attr(test, mockall::automock)]
pub trait TrayManager: Send {
    /// Update the visual state of the tray icon.
    fn set_state(&mut self, state: TrayState);

    /// Update the tray icon tooltip text.
    fn set_tooltip(&mut self, tooltip: &str);

    /// Returns `true` if the tray icon is currently visible.
    fn is_visible(&self) -> bool;
}

/// System tray manager backed by the `ksni` crate.
///
/// This is a stub implementation. A fully functional implementation requires a
/// running D-Bus session and a desktop environment that supports the
/// StatusNotifierItem specification (KDE, GNOME with an extension, etc.).
pub struct KsniTray {
    state: TrayState,
    tooltip: String,
    visible: bool,
}

impl KsniTray {
    /// Create a new `KsniTray` instance.
    ///
    /// The tray starts in `Idle` state and is not visible until a desktop
    /// session registers it.
    pub fn new() -> Self {
        Self {
            state: TrayState::Idle,
            tooltip: String::from("Linux Whisper"),
            visible: false,
        }
    }

    /// Get the current tray state.
    pub fn state(&self) -> TrayState {
        self.state
    }

    /// Get the current tooltip text.
    pub fn tooltip(&self) -> &str {
        &self.tooltip
    }
}

impl TrayManager for KsniTray {
    fn set_state(&mut self, state: TrayState) {
        self.state = state;
        tracing::debug!("tray state changed to {:?}", state);
    }

    fn set_tooltip(&mut self, tooltip: &str) {
        self.tooltip = tooltip.to_string();
        tracing::debug!("tray tooltip set to {:?}", tooltip);
    }

    fn is_visible(&self) -> bool {
        self.visible
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ksni_tray_initial_state() {
        let tray = KsniTray::new();
        assert_eq!(tray.state(), TrayState::Idle);
        assert_eq!(tray.tooltip(), "Linux Whisper");
        assert!(!tray.is_visible());
    }

    #[test]
    fn ksni_tray_set_state() {
        let mut tray = KsniTray::new();
        tray.set_state(TrayState::Recording);
        assert_eq!(tray.state(), TrayState::Recording);

        tray.set_state(TrayState::Transcribing);
        assert_eq!(tray.state(), TrayState::Transcribing);

        tray.set_state(TrayState::Idle);
        assert_eq!(tray.state(), TrayState::Idle);
    }

    #[test]
    fn ksni_tray_set_tooltip() {
        let mut tray = KsniTray::new();
        tray.set_tooltip("Recording...");
        assert_eq!(tray.tooltip(), "Recording...");
    }

    #[test]
    fn mock_tray_manager_set_state() {
        let mut mock = MockTrayManager::new();

        mock.expect_set_state()
            .withf(|state: &TrayState| *state == TrayState::Recording)
            .times(1)
            .return_const(());

        mock.set_state(TrayState::Recording);
    }

    #[test]
    fn mock_tray_manager_set_tooltip() {
        let mut mock = MockTrayManager::new();

        mock.expect_set_tooltip()
            .withf(|tooltip: &str| tooltip == "Transcribing...")
            .times(1)
            .return_const(());

        mock.set_tooltip("Transcribing...");
    }

    #[test]
    fn mock_tray_manager_is_visible() {
        let mut mock = MockTrayManager::new();

        mock.expect_is_visible().times(1).returning(|| true);

        assert!(mock.is_visible());
    }

    #[test]
    fn tray_state_debug() {
        assert_eq!(format!("{:?}", TrayState::Idle), "Idle");
        assert_eq!(format!("{:?}", TrayState::Recording), "Recording");
        assert_eq!(format!("{:?}", TrayState::Transcribing), "Transcribing");
    }

    #[test]
    fn tray_action_debug() {
        assert_eq!(format!("{:?}", TrayAction::ToggleRecording), "ToggleRecording");
        assert_eq!(format!("{:?}", TrayAction::ShowWindow), "ShowWindow");
        assert_eq!(format!("{:?}", TrayAction::Preferences), "Preferences");
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
}
