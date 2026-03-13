<p align="center">
  <img src="data/icons/pigeon.svg?v=3" width="128" alt="Linux Whisper">
</p>

<h1 align="center">Linux Whisper</h1>

<p align="center">
  Local, privacy-focused speech-to-text for Linux.<br>
  <em>No cloud. No tracking. Just whisper.</em>
</p>

<p align="center">
  <a href="https://kwap.github.io/linux-whisper">Website</a> &middot;
  <a href="https://github.com/kwap/linux-whisper/releases/latest">Download</a> &middot;
  <a href="#installation">Install</a>
</p>

<p align="center">
  <a href="https://github.com/kwap/linux-whisper/actions/workflows/ci.yml"><img src="https://github.com/kwap/linux-whisper/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/kwap/linux-whisper/releases/latest"><img src="https://img.shields.io/github/v/release/kwap/linux-whisper?label=release" alt="Release"></a>
  <a href="https://github.com/kwap/linux-whisper/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-GPLv3-blue" alt="License"></a>
</p>

---

## Features

- **System-wide hotkey dictation** — press a key, speak, text auto-pastes into any app
- **Audio file transcription** — WAV, MP3, OGG, M4A, OPUS, FLAC, MP4
- **Export** — TXT, SRT, VTT, CSV
- **50+ languages** — auto-detection, switch mid-sentence
- **CUDA GPU acceleration** — falls back to CPU gracefully
- **Privacy-focused** — all processing local, no accounts, no telemetry

## How It Works

1. Press a global hotkey anywhere on your desktop
2. Speak naturally — your voice is captured locally
3. Whisper transcribes your speech on-device
4. Text is auto-pasted into your focused window

All inference runs through [whisper.cpp](https://github.com/ggerganov/whisper.cpp). Nothing leaves your machine.

## Supported Platforms

- <img src="https://raw.githubusercontent.com/nicehash/logos/refs/heads/master/pop_os.svg" width="16" height="16" alt="Pop!_OS"> **Pop!_OS** 24.04
- <img src="https://assets.ubuntu.com/v1/ce518a18-CoF-2022_simplified.svg" width="16" height="16" alt="Ubuntu"> **Ubuntu** 24.04+
- <img src="https://fedoraproject.org/assets/images/logos/fedora-blue.png" width="16" height="16" alt="Fedora"> **Fedora** 40+
- <img src="https://archlinux.org/static/logos/archlinux-logo-dark-scalable.svg" width="16" height="16" alt="Arch Linux"> **Arch Linux** Rolling
- <img src="https://www.debian.org/logos/openlogo-nd.svg" width="16" height="16" alt="Debian"> **Debian** 13+

## Prerequisites

<details>
<summary><strong>Ubuntu / Pop!_OS / Debian</strong></summary>

```bash
# Build tools and GTK4/libadwaita development libraries
sudo apt install build-essential cmake libgtk-4-dev libadwaita-1-dev

# Audio capture (ALSA development headers)
sudo apt install libasound2-dev

# Wayland clipboard and text injection (for auto-paste)
sudo apt install wtype wl-clipboard

# Global hotkeys — add yourself to the input group (reboot required)
sudo usermod -aG input $USER

# Rust toolchain (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

</details>

<details>
<summary><strong>Fedora</strong></summary>

```bash
sudo dnf install gcc cmake gtk4-devel libadwaita-devel alsa-lib-devel wtype wl-clipboard
sudo usermod -aG input $USER
```

</details>

<details>
<summary><strong>Arch Linux</strong></summary>

```bash
sudo pacman -S base-devel cmake gtk4 libadwaita alsa-lib wtype wl-clipboard
sudo usermod -aG input $USER
```

</details>

## Installation

### From source

```bash
git clone https://github.com/kwap/linux-whisper.git
cd linux-whisper
cargo build --release
```

Install system-wide:

```bash
# Copy binary
sudo cp target/release/linux-whisper /usr/local/bin/

# Install desktop entry
APP=com.linuxwhisper.LinuxWhisper
sudo cp data/$APP.desktop /usr/share/applications/
sudo cp data/icons/hicolor/scalable/apps/$APP.svg \
  /usr/share/icons/hicolor/scalable/apps/

# Update icon cache
sudo gtk-update-icon-cache /usr/share/icons/hicolor/
```

Or use [just](https://github.com/casey/just):

```bash
just release
just install
```

### Pre-built packages

Download the latest `.deb` package or binary tarball from [GitHub Releases](https://github.com/kwap/linux-whisper/releases/latest).

## Usage

On first run, Linux Whisper will prompt you to download a Whisper model. You can change the model size in Preferences at any time.

The app runs in the system tray:

- **Left-click** the tray icon to toggle recording
- **Right-click** for the menu (Record/Stop, Preferences, About, Quit)
- **Global hotkey** (configurable) to start/stop dictation from anywhere

## Architecture

Rust workspace with six crates:

| Crate | Purpose |
|---|---|
| `core` | Model definitions, language support, export formats, search, config |
| `audio` | CPAL audio capture, resampling, file decoding via symphonia |
| `whisper` | whisper.cpp engine via whisper-rs, model registry, download manager |
| `platform` | Display detection, evdev hotkeys, clipboard, text injection, system tray |
| `i18n` | Fluent-based localization (en-US, es) |
| `app` | GTK4/libadwaita UI, dictation and transcription services |

## Security

- Zero `unsafe` code — all FFI through safe whisper-rs bindings
- No telemetry, analytics, or tracking
- SHA-256 model verification on download
- No command injection — external tools invoked with safe argument passing
- Evdev hotkey listener tracks only the configured combo
- All CI/CD Actions pinned to commit hashes

See the full [Security Audit](https://kwap.github.io/linux-whisper/#security) on the project website.

## Contributing

Contributions are welcome! Please open an issue first to discuss what you'd like to change.

```bash
# Run tests
cargo test --workspace --lib
cargo test --bin linux-whisper -p linux-whisper-app

# Check formatting and lints
cargo fmt --all -- --check
cargo clippy --workspace --no-default-features -- -D warnings
```

## License

This project is licensed under the [GNU General Public License v3.0](LICENSE).
