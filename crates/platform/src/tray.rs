use ksni::menu::{MenuItem, StandardItem};
use ksni::{Icon, ToolTip, TrayMethods};
use tokio::sync::mpsc;

/// Embedded SVG source for the normal tray icon.
const SVG_NORMAL: &str =
    include_str!("../../../data/tray-icons/hicolor/scalable/status/linux-whisper-tray.svg");

/// Embedded SVG source for the recording tray icon.
const SVG_RECORDING: &str = include_str!(
    "../../../data/tray-icons/hicolor/scalable/status/linux-whisper-tray-recording.svg"
);

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
        // Return empty — we use icon_pixmap with SVG-rendered pixels.
        String::new()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        let svg = if self.recording {
            SVG_RECORDING
        } else {
            SVG_NORMAL
        };
        // Provide multiple sizes; the DE picks the best fit.
        let mut icons = Vec::new();
        for size in [24, 32, 48, 64, 128] {
            if let Some(icon) = render_svg_to_icon(svg, size) {
                icons.push(icon);
            }
        }
        icons
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
// Icon rendering — anti-aliased vector-style ARGB32 pixmap
// ---------------------------------------------------------------------------

/// Render an embedded SVG to an ARGB32 pixmap via resvg.
///
/// The SVG is rasterised at the requested `size` and the resulting RGBA
/// pixels are converted to big-endian ARGB as required by the ksni `Icon`.
/// Returns `None` if the SVG fails to parse (should never happen for our
/// embedded icons).
fn render_svg_to_icon(svg_data: &str, size: i32) -> Option<Icon> {
    use resvg::tiny_skia;
    use resvg::usvg;

    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_str(svg_data, &opt).ok()?;

    let s = size as u32;
    let mut pixmap = tiny_skia::Pixmap::new(s, s)?;

    // Scale the SVG viewBox to fit the target size.
    let svg_size = tree.size();
    let sx = s as f32 / svg_size.width();
    let sy = s as f32 / svg_size.height();
    let transform = tiny_skia::Transform::from_scale(sx, sy);

    resvg::render(&tree, transform, &mut pixmap.as_mut());

    // Convert from RGBA (tiny-skia premultiplied) to ARGB (ksni big-endian).
    let rgba = pixmap.data();
    let mut argb = Vec::with_capacity(rgba.len());
    for chunk in rgba.chunks_exact(4) {
        let (r, g, b, a) = (chunk[0], chunk[1], chunk[2], chunk[3]);
        // Un-premultiply.
        let (r, g, b) = if a > 0 && a < 255 {
            let af = a as f32 / 255.0;
            (
                (r as f32 / af).round().min(255.0) as u8,
                (g as f32 / af).round().min(255.0) as u8,
                (b as f32 / af).round().min(255.0) as u8,
            )
        } else {
            (r, g, b)
        };
        argb.push(a);
        argb.push(r);
        argb.push(g);
        argb.push(b);
    }

    Some(Icon {
        width: size,
        height: size,
        data: argb,
    })
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
        assert_eq!(
            format!("{:?}", TrayAction::ToggleRecording),
            "ToggleRecording"
        );
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
    fn tray_impl_icon_name_empty() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let tray = LinuxWhisperTray {
            recording: false,
            status_text: "Ready".into(),
            action_tx: tx,
        };
        use ksni::Tray;
        assert!(tray.icon_name().is_empty());
    }

    #[test]
    fn tray_impl_icon_pixmap_multiple_sizes() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let tray = LinuxWhisperTray {
            recording: false,
            status_text: "Ready".into(),
            action_tx: tx,
        };
        use ksni::Tray;
        let pixmaps = tray.icon_pixmap();
        assert_eq!(pixmaps.len(), 5);
        assert_eq!(pixmaps[0].width, 24);
        assert_eq!(pixmaps[4].width, 128);
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
    fn render_svg_icon_correct_size() {
        let icon = render_svg_to_icon(SVG_NORMAL, 48).unwrap();
        assert_eq!(icon.width, 48);
        assert_eq!(icon.height, 48);
        assert_eq!(icon.data.len(), 48 * 48 * 4);
    }

    #[test]
    fn render_svg_icon_has_blue_body_pixels() {
        let icon = render_svg_to_icon(SVG_NORMAL, 48).unwrap();
        // Should have blue-ish body pixels (A=0xFF, B > R)
        let blue_count = icon
            .data
            .chunks(4)
            .filter(|px| px[0] == 0xFF && px[3] > px[1] && px[3] > 80)
            .count();
        assert!(
            blue_count > 50,
            "Should have visible blue body pixels, got {blue_count}"
        );
    }

    #[test]
    fn render_svg_icon_recording_has_red() {
        let icon = render_svg_to_icon(SVG_RECORDING, 48).unwrap();
        // Recording icon should have red pixels (A=0xFF, R>0xC0, G<0x60).
        let red_count = icon
            .data
            .chunks(4)
            .filter(|px| px[0] == 0xFF && px[1] > 0xC0 && px[2] < 0x60)
            .count();
        assert!(
            red_count > 5,
            "Should have visible red eye pixels, got {red_count}"
        );
    }

    #[test]
    fn render_svg_icon_scales_to_multiple_sizes() {
        for size in [24, 32, 48, 64, 128] {
            let icon = render_svg_to_icon(SVG_NORMAL, size).unwrap();
            assert_eq!(icon.width, size);
            assert_eq!(icon.height, size);
            assert_eq!(icon.data.len(), (size * size * 4) as usize);
        }
    }
}
