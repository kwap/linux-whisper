<p align="center">
  <img src="data/icons/pigeon.svg?v=3" width="128" alt="Linux Whisper">
</p>

<h1 align="center">Linux Whisper</h1>

<p align="center">
  Local, privacy-focused speech-to-text for Linux.<br>
  <em>No cloud. No tracking. Just whisper.</em>
</p>

<p align="center">
  <a href="https://kwap.io/linux-whisper/">Website</a> &middot;
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

- **System-wide hotkey dictation** — press your shortcut and speak: the words appear where your cursor is as you talk, like dictation on a phone, and the last few are corrected as Whisper hears more; the shortcut's own keys are kept out of it
- **Audio file transcription** — WAV, MP3, OGG, M4A, OPUS, FLAC, MP4, with searchable timestamps
- **Export** — TXT, SRT, VTT, CSV
- **50+ languages** — detected sentence by sentence among the languages you speak, so you can switch language between sentences, accent and all
- **Tidy text** — capitals, punctuation and paragraphs; optionally polished by a small local language model
- **NVIDIA GPU acceleration** — CUDA when built with `--features cuda`; the processor does the work otherwise
- **Privacy-focused** — all processing local, no accounts, no telemetry

## How It Works

1. Press the dictation shortcut anywhere on your desktop
2. Speak naturally — your voice is captured locally
3. Whisper transcribes your speech on-device
4. The words appear in your focused window as you speak

All inference runs through [whisper.cpp](https://github.com/ggerganov/whisper.cpp). Nothing leaves your machine.

## Supported Platforms

- <img src="https://cdn.simpleicons.org/popos" width="16" height="16" alt="Pop!_OS"> **Pop!_OS** 24.04
- <img src="https://cdn.simpleicons.org/ubuntu" width="16" height="16" alt="Ubuntu"> **Ubuntu** 24.04+
- <img src="https://cdn.simpleicons.org/fedora" width="16" height="16" alt="Fedora"> **Fedora** 40+
- <img src="https://cdn.simpleicons.org/archlinux" width="16" height="16" alt="Arch Linux"> **Arch Linux** Rolling
- <img src="https://cdn.simpleicons.org/debian" width="16" height="16" alt="Debian"> **Debian** 13+

Wayland (GNOME, KDE Plasma, wlroots desktops such as Sway) and X11.

## Prerequisites

Building needs Rust 1.89 or newer.

<details>
<summary><strong>Ubuntu / Pop!_OS / Debian</strong></summary>

```bash
# Build tools and the GTK4, libadwaita and ALSA development libraries
sudo apt install build-essential cmake pkg-config git curl \
  libgtk-4-dev libadwaita-1-dev libasound2-dev

# Clipboard and typing tools used for pasting the text
sudo apt install wl-clipboard xdotool wtype

# Global hotkey: add yourself to the input group, then log out and back in
sudo usermod -aG input $USER

# Rust (the version in apt is too old)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
. "$HOME/.cargo/env"
```

</details>

<details>
<summary><strong>Fedora</strong></summary>

```bash
sudo dnf install gcc gcc-c++ cmake pkgconf-pkg-config git rust cargo \
  gtk4-devel libadwaita-devel alsa-lib-devel wl-clipboard xdotool wtype
sudo usermod -aG input $USER    # then log out and back in
```

</details>

<details>
<summary><strong>Arch Linux</strong></summary>

```bash
sudo pacman -S --needed base-devel cmake git rust gtk4 libadwaita alsa-lib \
  wl-clipboard xdotool wtype
sudo usermod -aG input $USER    # then log out and back in
```

</details>

## Installation

### From source

```bash
git clone https://github.com/kwap/linux-whisper.git
cd linux-whisper
cargo build --release
```

Install it (asks for your password; installs to `/usr/local`):

```bash
./build-aux/install.sh
```

This installs the program, its menu entry and icon, and a udev rule that lets
Linux Whisper paste text on GNOME and KDE and keep its shortcut out of your
apps. Or use [just](https://github.com/casey/just): `just release` and then
`just install`. To uninstall, run `./build-aux/uninstall.sh` (your settings
and models are kept).

#### NVIDIA GPU

With the CUDA toolkit installed (`nvcc` and cuBLAS), build with:

```bash
cargo build --release --features cuda
```

If `nvcc` rejects your compiler (for example CUDA 13 with GCC 16 on Fedora 44),
point it to a supported one: `NVCC_CCBIN=/usr/bin/g++-15 cargo build --release --features cuda`
(Fedora package `gcc15-c++`). To also run the optional AI formatter on the GPU,
add the `llm-cuda` feature; it needs NVRTC and cuRAND as well.

### Pre-built packages

Download the latest `.deb` package or binary tarball from [GitHub Releases](https://github.com/kwap/linux-whisper/releases/latest).
They run on the processor; build from source for GPU acceleration. From the
tarball, run `./install.sh` in the extracted folder.

## Usage

On first start, Linux Whisper asks to download a Whisper model. You can choose
another model size later, in Preferences or in the window's model menu.

- **Dictation shortcut** (default `Super+Shift+Space`): press to start dictating, press again to stop. To change it, click it in Preferences and press the keys you want
- **Tray icon**: left-click to start/stop; right-click for the menu (Record/Stop, Transcription Window, Preferences, About, Quit)
- **Window**: click the pigeon to start or stop dictating (its eye glows red while recording, as in the tray); this session's transcripts are shown as cards you can search, copy and export; open or drop an audio file to transcribe it

**Languages.** By default each sentence is checked against every language
Whisper knows. In Preferences → Languages You Speak, tick yours (for example
English, Polish and German): detection then only chooses among them, which
is far more reliable, especially with an accent. Language switching works
best with the Small model or larger.

**The shortcut stays out of your apps.** Linux Whisper reads the keyboard
directly (which needs the input group), so the shortcut works on every
desktop. So that pressing it does not also type into the app you are using,
Linux Whisper takes the keyboard over and passes every key on unchanged
through a virtual keyboard, except the shortcut's own last key. This needs
the udev rule from the install step; it can be turned off in Preferences.
Keyboards that also act as a mouse (some wireless receivers) are only read,
so on those the shortcut's last key still reaches the app. Ctrl+Z in the
terminal Linux Whisper runs in releases the keyboard before it stops; a
debugger does not, so turn the option off while debugging.

Without a tray (stock GNOME has none), the window opens instead; the
[AppIndicator extension](https://extensions.gnome.org/extension/615/appindicator-support/)
adds the tray icon. Closing the window keeps Linux Whisper running so the
hotkey keeps working; start it again from the menu to bring the window back.

While you speak, the words are typed into the focused window on a virtual
keyboard, with your own keyboard layout, on any desktop (this needs the udev
rule from the install step). When Whisper hears more, the words since your
last pause may be corrected: they are erased and typed again. A sentence
followed by a pause no longer changes. Keep the cursor where it is until you
stop dictating.

With Type While You Speak turned off in Preferences, or without the udev
rule, the text is inserted when you stop, in the way that works on the
desktop:

| Desktop | How the text is inserted |
|---|---|
| X11 | typed with `xdotool` |
| Sway, Hyprland and other wlroots compositors | typed with `wtype` |
| GNOME and KDE (Wayland) | pasted with a virtual keyboard; needs the udev rule from the install step (Preferences → System shows whether it is in place) |

Either way the text is copied to the clipboard as well. The paste shortcut
is `Ctrl+V` by default; choose `Ctrl+Shift+V` in
Preferences if you mostly dictate into a terminal.

## Architecture

Rust workspace with seven crates:

| Crate | Purpose |
|---|---|
| `core` | Model definitions, language support, export formats, search, config |
| `audio` | CPAL audio capture, resampling, file decoding via symphonia (Opus via opus-decoder) |
| `whisper` | whisper.cpp engine via whisper-rs, model registry, download manager |
| `platform` | Display detection, evdev hotkeys, clipboard, text injection, system tray |
| `llm` | Optional local language model (Qwen2.5 0.5B via candle) that polishes dictated text |
| `i18n` | Fluent-based localization (en-US, es) |
| `app` | GTK4/libadwaita UI, dictation and transcription services |

## Security

- Zero `unsafe` code — all FFI through safe whisper-rs bindings
- No telemetry, analytics, or tracking
- SHA-256 model verification on download
- No command injection — external tools invoked with safe argument passing
- Downloads are written to a temporary file and only kept if their SHA-256 matches
- The evdev hotkey listener only looks for the configured shortcut, and whether a modifier such as Ctrl is held so dictated words are not typed as shortcuts; when it keeps the shortcut from other apps, every other key is passed on unchanged, never recorded or stored
- All CI/CD Actions pinned to commit hashes

See the full [Security Audit](https://kwap.io/linux-whisper/#security) on the project website.

## Contributing

Contributions are welcome! Please open an issue first to discuss what you'd like to change.

```bash
# Run tests
cargo test --workspace --lib
cargo test --bin linux-whisper -p linux-whisper-app

# Check formatting and lints
cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings

# Try speech recognition on a file without the GUI
cargo run --release -p linux-whisper-whisper --example transcribe -- \
  ~/.local/share/linux-whisper/models/ggml-base.bin recording.m4a

# See how a file would be typed while it is spoken, correction by correction
cargo run --release -p linux-whisper-whisper --example live -- \
  ~/.local/share/linux-whisper/models/ggml-base.bin recording.m4a en,pl,de
```

## License

This project is licensed under the [GNU General Public License v3.0](LICENSE).
