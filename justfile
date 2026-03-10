# Linux Whisper — project task runner
# Install just: https://github.com/casey/just

app_id := "com.linuxwhisper.LinuxWhisper"
prefix := "/usr/local"

# Default: list available recipes
default:
    @just --list

# Build all crates in debug mode
build:
    cargo build --workspace

# Build optimised release binary
release:
    cargo build --workspace --release

# Run the app (debug build)
run:
    cargo run -p linux-whisper-app

# Run all unit tests (library crates)
test:
    cargo test --workspace --lib

# Run all unit tests including binary tests
test-all:
    cargo test --workspace --lib
    cargo test --bin linux-whisper -p linux-whisper-app

# Run tests for a single crate
test-crate crate:
    cargo test -p {{crate}} --lib

# Lint with clippy
lint:
    cargo clippy --workspace -- -D warnings

# Format check
fmt-check:
    cargo fmt --all -- --check

# Format all source files
fmt:
    cargo fmt --all

# Run clippy + fmt check (CI-style)
check: lint fmt-check

# Install the app to prefix (default /usr/local)
install: release
    install -Dm755 target/release/linux-whisper "{{prefix}}/bin/linux-whisper"
    install -Dm644 data/{{app_id}}.desktop "{{prefix}}/share/applications/{{app_id}}.desktop"
    install -Dm644 data/icons/{{app_id}}.svg "{{prefix}}/share/icons/hicolor/scalable/apps/{{app_id}}.svg"
    install -Dm644 data/icons/{{app_id}}-symbolic.svg "{{prefix}}/share/icons/hicolor/symbolic/apps/{{app_id}}-symbolic.svg"
    install -Dm644 data/{{app_id}}.gschema.xml "{{prefix}}/share/glib-2.0/schemas/{{app_id}}.gschema.xml"
    install -Dm644 data/{{app_id}}.metainfo.xml "{{prefix}}/share/metainfo/{{app_id}}.metainfo.xml"
    glib-compile-schemas "{{prefix}}/share/glib-2.0/schemas/" || true
    gtk-update-icon-cache "{{prefix}}/share/icons/hicolor/" || true

# Uninstall the app
uninstall:
    rm -f "{{prefix}}/bin/linux-whisper"
    rm -f "{{prefix}}/share/applications/{{app_id}}.desktop"
    rm -f "{{prefix}}/share/icons/hicolor/scalable/apps/{{app_id}}.svg"
    rm -f "{{prefix}}/share/icons/hicolor/symbolic/apps/{{app_id}}-symbolic.svg"
    rm -f "{{prefix}}/share/glib-2.0/schemas/{{app_id}}.gschema.xml"
    rm -f "{{prefix}}/share/metainfo/{{app_id}}.metainfo.xml"

# Clean all build artifacts
clean:
    cargo clean

# Show workspace dependency tree
deps:
    cargo tree --workspace

# Count lines of source code
loc:
    @find crates -name '*.rs' | xargs wc -l | tail -1

# Generate cargo-sources.json for Flatpak builds
flatpak-sources:
    ./build-aux/flatpak-cargo-generator.sh

# Build and install as Flatpak (requires flatpak-builder, GNOME SDK)
flatpak-build: flatpak-sources
    flatpak-builder --user --install --force-clean build-dir {{app_id}}.yml
