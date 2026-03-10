#!/bin/bash
# Generate cargo-sources.json for Flatpak builds.
# Requires: python3, pip install aiohttp toml
#
# Usage: ./build-aux/flatpak-cargo-generator.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

GENERATOR_URL="https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py"
GENERATOR="/tmp/flatpak-cargo-generator.py"

if [ ! -f "$GENERATOR" ]; then
    echo "Downloading flatpak-cargo-generator.py..."
    curl -sL "$GENERATOR_URL" -o "$GENERATOR"
fi

echo "Generating cargo-sources.json from Cargo.lock..."
python3 "$GENERATOR" "$PROJECT_DIR/Cargo.lock" -o "$PROJECT_DIR/cargo-sources.json"
echo "Done: cargo-sources.json"
