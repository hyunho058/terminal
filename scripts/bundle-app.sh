#!/usr/bin/env bash
# Build a minimal mac-terminal.app for local daily-driver use.
#
# Baseline only: no code signing, no notarization (explicit non-goals). The
# bundle runs on the machine that built it. Output: target/mac-terminal.app
set -euo pipefail

cd "$(dirname "$0")/.."

APP_NAME="mac-terminal"
BIN="app"
BUILD_DIR="target/release"
APP_DIR="target/${APP_NAME}.app"

echo "==> building release binary"
cargo build --release -p app

echo "==> assembling ${APP_DIR}"
rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"

cp "$BUILD_DIR/$BIN" "$APP_DIR/Contents/MacOS/$APP_NAME"

cat > "$APP_DIR/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>${APP_NAME}</string>
    <key>CFBundleDisplayName</key><string>mac-terminal</string>
    <key>CFBundleIdentifier</key><string>dev.local.mac-terminal</string>
    <key>CFBundleVersion</key><string>0.1.0</string>
    <key>CFBundleShortVersionString</key><string>0.1.0</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleExecutable</key><string>${APP_NAME}</string>
    <key>LSMinimumSystemVersion</key><string>13.0</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

echo "==> done: ${APP_DIR}"
echo "    run: open ${APP_DIR}   (or ${APP_DIR}/Contents/MacOS/${APP_NAME} for logs)"
