#!/bin/bash
# Create a macOS .app bundle for QMC Decoder
# Usage: ./scripts/create-app-bundle.sh [path-to-binary]
#
# If no binary path is given, defaults to target/release/qmc-decoder

set -euo pipefail

BINARY="${1:-target/release/qmc-decoder}"
APP_NAME="QMC Decoder"
APP_BUNDLE="${APP_NAME}.app"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

if [ ! -f "$BINARY" ]; then
    echo "Error: Binary not found at $BINARY"
    echo "Build first with: cargo build --release --features gui"
    exit 1
fi

echo "Creating .app bundle from: $BINARY"

# Create bundle structure
mkdir -p "${APP_BUNDLE}/Contents/MacOS"
mkdir -p "${APP_BUNDLE}/Contents/Resources"

# Copy binary
cp "$BINARY" "${APP_BUNDLE}/Contents/MacOS/qmc-decoder"
chmod +x "${APP_BUNDLE}/Contents/MacOS/qmc-decoder"

# Copy Info.plist
cp "${PROJECT_DIR}/macos/Info.plist" "${APP_BUNDLE}/Contents/Info.plist"

echo "Created: ${APP_BUNDLE}"
echo "To open: open '${APP_BUNDLE}'"