#!/bin/bash
# Uninstall Linux Whisper installed with install.sh (default prefix:
# /usr/local). Asks for sudo when needed. Your settings and downloaded models
# stay in ~/.config/linux-whisper and ~/.local/share/linux-whisper.
#
# Usage: ./build-aux/uninstall.sh [prefix]   (or ./uninstall.sh from a release tarball)
set -euo pipefail

PREFIX=${1:-/usr/local}
APP=com.linuxwhisper.LinuxWhisper

as_root() { if [ "$(id -u)" = 0 ]; then "$@"; else sudo "$@"; fi; }
if [ -w "$PREFIX/bin" ]; then from_prefix() { "$@"; }; else from_prefix() { as_root "$@"; }; fi

# A running Linux Whisper holds the keyboard copy and the binary.
pkill -x linux-whisper 2>/dev/null || true

echo "Removing Linux Whisper from $PREFIX"
from_prefix rm -f "$PREFIX/bin/linux-whisper" \
    "$PREFIX/share/applications/$APP.desktop" \
    "$PREFIX/share/icons/hicolor/scalable/apps/$APP.svg" \
    "$PREFIX/share/icons/hicolor/symbolic/apps/$APP-symbolic.svg" \
    "$PREFIX/share/metainfo/$APP.metainfo.xml" \
    "$PREFIX/share/glib-2.0/schemas/$APP.gschema.xml"
from_prefix glib-compile-schemas "$PREFIX/share/glib-2.0/schemas" >/dev/null 2>&1 || true
from_prefix gtk-update-icon-cache -q -t -f "$PREFIX/share/icons/hicolor" 2>/dev/null || true
from_prefix update-desktop-database -q "$PREFIX/share/applications" 2>/dev/null || true

if [ -e /etc/udev/rules.d/60-linux-whisper-uinput.rules ]; then
    echo "Removing the udev rule (needs sudo)"
    as_root rm -f /etc/udev/rules.d/60-linux-whisper-uinput.rules
    as_root udevadm control --reload-rules 2>/dev/null || true
fi

echo "Done. Settings and models are kept in ~/.config/linux-whisper and ~/.local/share/linux-whisper."
