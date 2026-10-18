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

# Build optimised release binary (CPU)
release:
    cargo build --release -p linux-whisper-app

# Build optimised release binary with NVIDIA GPU acceleration (needs the CUDA toolkit)
release-cuda:
    cargo build --release -p linux-whisper-app --features cuda

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

# Build first with `just release` or `just release-cuda` (not as root).
# Install the release build to prefix (default /usr/local); asks for sudo when needed
install:
    ./build-aux/install.sh "{{prefix}}"

# Uninstall the app (settings and models are kept)
uninstall:
    ./build-aux/uninstall.sh "{{prefix}}"

# Check the virtual keyboards (paste, and keeping the hotkey from apps) against
# the real kernel; needs /dev/uinput and /dev/input access
test-uinput:
    cargo test -p linux-whisper-platform --lib -- --ignored --test-threads=1

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
