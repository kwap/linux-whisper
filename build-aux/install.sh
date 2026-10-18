#!/bin/bash
# Install Linux Whisper (default prefix: /usr/local). Asks for sudo when needed.
#
# Usage, from the source tree after `cargo build --release`:
#     ./build-aux/install.sh [prefix]
# or from an extracted release tarball:
#     ./install.sh [prefix]
set -euo pipefail

PREFIX=${1:-/usr/local}
HERE=$(cd "$(dirname "$0")" && pwd)
APP=com.linuxwhisper.LinuxWhisper

if [ -x "$HERE/linux-whisper" ]; then
    # Release tarball: binary and data/ next to this script.
    ROOT=$HERE
    BIN=$HERE/linux-whisper
elif [ -x "$HERE/../target/release/linux-whisper" ]; then
    ROOT=$(cd "$HERE/.." && pwd)
    BIN=$ROOT/target/release/linux-whisper
else
    echo "No linux-whisper binary found. Build it first: cargo build --release" >&2
    exit 1
fi
DATA=$ROOT/data

as_root() { if [ "$(id -u)" = 0 ]; then "$@"; else sudo "$@"; fi; }
# Install into $PREFIX, with sudo only if it is not writable (e.g. ~/.local is).
mkdir -p "$PREFIX" 2>/dev/null || true
if [ -w "$PREFIX" ]; then into_prefix() { "$@"; }; else into_prefix() { as_root "$@"; }; fi

echo "Installing Linux Whisper into $PREFIX"
into_prefix install -Dm755 "$BIN" "$PREFIX/bin/linux-whisper"
into_prefix install -Dm644 "$DATA/$APP.desktop" "$PREFIX/share/applications/$APP.desktop"
into_prefix install -Dm644 "$DATA/icons/$APP.svg" "$PREFIX/share/icons/hicolor/scalable/apps/$APP.svg"
into_prefix install -Dm644 "$DATA/icons/$APP-symbolic.svg" "$PREFIX/share/icons/hicolor/symbolic/apps/$APP-symbolic.svg"
into_prefix install -Dm644 "$DATA/$APP.metainfo.xml" "$PREFIX/share/metainfo/$APP.metainfo.xml"
into_prefix install -Dm644 "$DATA/$APP.gschema.xml" "$PREFIX/share/glib-2.0/schemas/$APP.gschema.xml"
into_prefix glib-compile-schemas "$PREFIX/share/glib-2.0/schemas" 2>/dev/null || true
into_prefix gtk-update-icon-cache -q -t -f "$PREFIX/share/icons/hicolor" 2>/dev/null || true
into_prefix update-desktop-database -q "$PREFIX/share/applications" 2>/dev/null || true

# Pasting on GNOME/KDE (Wayland) needs /dev/uinput. udev only reads rules
# from /etc and /usr/lib, whatever the prefix.
echo "Installing the udev rule for pasting on GNOME and KDE (needs sudo)"
as_root install -Dm644 "$DATA/60-linux-whisper-uinput.rules" /etc/udev/rules.d/60-linux-whisper-uinput.rules
as_root udevadm control --reload-rules 2>/dev/null || true
as_root udevadm trigger --name-match=uinput 2>/dev/null || true

echo "Done. Start Linux Whisper from your applications menu, or run: linux-whisper"
if ! id -nG "$USER" | tr ' ' '\n' | grep -qx input; then
    echo
    echo "For the global hotkey, add yourself to the input group, then log out and back in:"
    echo "    sudo usermod -aG input \$USER"
fi
