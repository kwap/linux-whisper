#!/bin/bash
# Generate cargo-sources.json for Flatpak builds.
# Requires: python3, pip install aiohttp toml
#
# Usage: ./build-aux/flatpak-cargo-generator.sh
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

# Pin to a specific commit for supply-chain safety.
GENERATOR_COMMIT="f03a673abe6ce189cea1c2857e2b44af2dd79d1f"
GENERATOR_URL="https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/${GENERATOR_COMMIT}/cargo/flatpak-cargo-generator.py"
GENERATOR_SHA256="b373c8ab1a05378ec5d8ed0645c7b127bcec7d2f7a1798694fbc627d570d856c"
GENERATOR="/tmp/flatpak-cargo-generator-${GENERATOR_COMMIT:0:12}.py"

if [ ! -f "$GENERATOR" ]; then
    echo "Downloading flatpak-cargo-generator.py (commit ${GENERATOR_COMMIT:0:12})..."
    curl -sL "$GENERATOR_URL" -o "$GENERATOR"

    # Verify integrity.
    ACTUAL_SHA256=$(sha256sum "$GENERATOR" | cut -d' ' -f1)
    if [ "$ACTUAL_SHA256" != "$GENERATOR_SHA256" ]; then
        echo "ERROR: SHA-256 mismatch for flatpak-cargo-generator.py"
        echo "  Expected: $GENERATOR_SHA256"
        echo "  Actual:   $ACTUAL_SHA256"
        rm -f "$GENERATOR"
        exit 1
    fi
fi

echo "Generating cargo-sources.json from Cargo.lock..."
python3 "$GENERATOR" "$PROJECT_DIR/Cargo.lock" -o "$PROJECT_DIR/cargo-sources.json"
echo "Done: cargo-sources.json"
