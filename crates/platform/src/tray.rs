use ksni::menu::{MenuItem, StandardItem};
use ksni::{Icon, ToolTip, TrayMethods};
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
    ShowWindow,
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
        // Return empty — we use icon_pixmap instead for a custom icon.
        String::new()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        vec![render_tray_icon(22, self.recording)]
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
            StandardItem {
                label: "Transcription Window".into(),
                icon_name: "utilities-terminal-symbolic".into(),
                activate: Box::new(|tray: &mut Self| {
                    let _ = tray.action_tx.send(TrayAction::ShowWindow);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Preferences\u{2026}".into(),
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

// ---------------------------------------------------------------------------
// Icon rendering — ARGB32 pixmap
// ---------------------------------------------------------------------------

/// Render the tray icon as an ARGB32 pixmap.
///
/// Pixel-art style icon on transparent background: golden-yellow microphone
/// on deep blue. Designed for clarity at 22x22. When `recording` is true,
/// a bright red dot appears in the bottom-right.
fn render_tray_icon(size: i32, recording: bool) -> Icon {
    let s = size as usize;
    let mut data = vec![0u8; s * s * 4];

    // At 22px, we draw pixel-art style using a logical grid.
    // Map logical pixels to actual pixels.
    let cell = s as f64 / 22.0;

    // Colors
    let bg = (0x14u8, 0x2D, 0x4C);  // deep navy
    let gold = (0xFF, 0xD5, 0x4F);   // golden yellow
    let red = (0xFF, 0x33, 0x33);     // recording red

    // Background: rounded square (2px corner radius in logical space)
    for y in 0..s {
        for x in 0..s {
            let lx = x as f64 / cell;
            let ly = y as f64 / cell;
            if in_rounded_rect(lx, ly, 1.0, 1.0, 20.0, 20.0, 3.0) {
                set_pixel(&mut data, s, x, y, 0xFF, bg.0, bg.1, bg.2);
            }
        }
    }

    // Microphone drawn as pixel blocks on a 22x22 logical grid.
    // Mic head (capsule): columns 9-12, rows 3-10 with rounded top
    let mic_pixels: &[(i32, i32)] = &[
        // Top dome (row 3-4)
        (10, 3), (11, 3),
        (9, 4), (10, 4), (11, 4), (12, 4),
        // Body (rows 5-10)
        (9, 5), (10, 5), (11, 5), (12, 5),
        (9, 6), (10, 6), (11, 6), (12, 6),
        (9, 7), (10, 7), (11, 7), (12, 7),
        (9, 8), (10, 8), (11, 8), (12, 8),
        (9, 9), (10, 9), (11, 9), (12, 9),
        // Bottom dome (row 10)
        (10, 10), (11, 10),
        // Arc (row 11-12)
        (7, 11), (8, 11), (13, 11), (14, 11),
        (6, 12), (15, 12),
        // Stand (rows 13-14)
        (10, 13), (11, 13),
        (10, 14), (11, 14),
        // Base (row 15)
        (8, 15), (9, 15), (10, 15), (11, 15), (12, 15), (13, 15),
        // Sound wave 1 (right side)
        (14, 5), (14, 7), (14, 9),
        // Sound wave 2 (further right)
        (16, 4), (16, 6), (16, 8), (16, 10),
    ];

    for &(lx, ly) in mic_pixels {
        let x0 = (lx as f64 * cell) as usize;
        let y0 = (ly as f64 * cell) as usize;
        let x1 = ((lx + 1) as f64 * cell) as usize;
        let y1 = ((ly + 1) as f64 * cell) as usize;
        for py in y0..y1.min(s) {
            for px in x0..x1.min(s) {
                set_pixel(&mut data, s, px, py, 0xFF, gold.0, gold.1, gold.2);
            }
        }
    }

    // Recording indicator: red dot bottom-right (3x3 logical pixels)
    if recording {
        let rec_pixels: &[(i32, i32)] = &[
            (16, 16), (17, 16), (18, 16),
            (16, 17), (17, 17), (18, 17),
            (16, 18), (17, 18), (18, 18),
        ];
        for &(lx, ly) in rec_pixels {
            let x0 = (lx as f64 * cell) as usize;
            let y0 = (ly as f64 * cell) as usize;
            let x1 = ((lx + 1) as f64 * cell) as usize;
            let y1 = ((ly + 1) as f64 * cell) as usize;
            for py in y0..y1.min(s) {
                for px in x0..x1.min(s) {
                    set_pixel(&mut data, s, px, py, 0xFF, red.0, red.1, red.2);
                }
            }
        }
    }

    Icon {
        width: size,
        height: size,
        data,
    }
}

/// Check if a point is inside a rounded rectangle.
fn in_rounded_rect(px: f64, py: f64, rx: f64, ry: f64, rw: f64, rh: f64, radius: f64) -> bool {
    if px < rx || px > rx + rw || py < ry || py > ry + rh {
        return false;
    }
    // Check corners.
    let corners = [
        (rx + radius, ry + radius),
        (rx + rw - radius, ry + radius),
        (rx + radius, ry + rh - radius),
        (rx + rw - radius, ry + rh - radius),
    ];
    for (ccx, ccy) in corners {
        let dx = px - ccx;
        let dy = py - ccy;
        let in_corner_zone = (px < rx + radius || px > rx + rw - radius)
            && (py < ry + radius || py > ry + rh - radius);
        if in_corner_zone && dx * dx + dy * dy > radius * radius {
            return false;
        }
    }
    true
}

/// Set a single ARGB32 pixel in network byte order (big-endian).
fn set_pixel(data: &mut [u8], stride: usize, x: usize, y: usize, a: u8, r: u8, g: u8, b: u8) {
    let offset = (y * stride + x) * 4;
    data[offset] = a;
    data[offset + 1] = r;
    data[offset + 2] = g;
    data[offset + 3] = b;
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
    fn tray_impl_icon_pixmap_idle() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let tray = LinuxWhisperTray {
            recording: false,
            status_text: "Ready".into(),
            action_tx: tx,
        };
        use ksni::Tray;
        let pixmaps = tray.icon_pixmap();
        assert_eq!(pixmaps.len(), 1);
        assert_eq!(pixmaps[0].width, 22);
        assert_eq!(pixmaps[0].height, 22);
        assert_eq!(pixmaps[0].data.len(), 22 * 22 * 4);
    }

    #[test]
    fn tray_impl_icon_pixmap_recording() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let tray = LinuxWhisperTray {
            recording: true,
            status_text: "Recording...".into(),
            action_tx: tx,
        };
        use ksni::Tray;
        let pixmaps = tray.icon_pixmap();
        assert_eq!(pixmaps.len(), 1);
        // Recording icon should have coral/red indicator pixels.
        let has_coral = pixmaps[0].data.chunks(4).any(|px| px[0] > 0 && px[1] > 0xC0 && px[2] < 0x80);
        assert!(has_coral, "Recording icon should contain coral indicator pixels");
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
        // Record, Transcription Window, Separator, Preferences, About, Separator, Quit = 7 items
        assert_eq!(menu.len(), 7);
    }

    #[test]
    fn render_icon_idle_has_correct_size() {
        let icon = render_tray_icon(22, false);
        assert_eq!(icon.width, 22);
        assert_eq!(icon.height, 22);
        assert_eq!(icon.data.len(), 22 * 22 * 4);
    }

    #[test]
    fn render_icon_recording_has_red_dot() {
        let icon = render_tray_icon(22, true);
        // Count coral indicator pixels (R>0xC0, G<0x80, A>0).
        let coral_count = icon.data.chunks(4).filter(|px| {
            px[0] > 0 && px[1] > 0xC0 && px[2] < 0x80 && px[3] < 0x80
        }).count();
        assert!(coral_count > 5, "Should have visible coral dot pixels, got {coral_count}");
    }

    #[test]
    fn set_pixel_writes_correctly() {
        let mut data = vec![0u8; 4 * 4 * 4]; // 4x4
        set_pixel(&mut data, 4, 1, 2, 0xFF, 0xAA, 0xBB, 0xCC);
        let offset = (2 * 4 + 1) * 4;
        assert_eq!(data[offset], 0xFF);     // A
        assert_eq!(data[offset + 1], 0xAA); // R
        assert_eq!(data[offset + 2], 0xBB); // G
        assert_eq!(data[offset + 3], 0xCC); // B
    }
}
