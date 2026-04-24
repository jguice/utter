#!/usr/bin/env bash
# Wrap target/release/utter in a minimal .app bundle so macOS TCC attributes
# permission requests to the bundle's CFBundleIdentifier (com.utter.app)
# rather than to whichever terminal launched the CLI. Side effect: the
# bundle's identity is stable across rebuilds (same CFBundleIdentifier,
# same ad-hoc signature over a predictable layout), so Input Monitoring /
# Accessibility / Microphone grants survive `cargo build --release`.
#
# Usage:
#   cargo build --release
#   ./scripts/make-bundle.sh          # produces target/release/utter.app
#
# Run the bundle's executable via `open` so LaunchServices attributes to
# the bundle. Direct invocation of Contents/MacOS/utter from a terminal
# can still get attributed to the terminal on some macOS versions.
#   open target/release/utter.app --args daemon
#   open target/release/utter.app --args watch
#   open target/release/utter.app --args set-key
# …or symlink target/release/utter.app/Contents/MacOS/utter somewhere on
# PATH once the initial grants are in place.

set -euo pipefail

cd "$(dirname "$0")/.."

BIN="target/release/utter"
APP="target/release/utter.app"
ICON_SRC="utter-icon.png"

if [[ ! -x "$BIN" ]]; then
    echo "error: $BIN not found. Run \`cargo build --release\` first." >&2
    exit 1
fi

VERSION="$(awk -F'"' '/^version = /{print $2; exit}' Cargo.toml)"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
mkdir -p "$APP/Contents/Resources"

cp "$BIN" "$APP/Contents/MacOS/utter"

# Generate a multi-resolution .icns from the source PNG and drop it in
# Contents/Resources/. macOS uses the highest-fidelity size that matches
# the display context (Finder, Dock, Cmd-Tab, Spotlight, alerts, menu
# bar) — so even though we launch as LSUIElement, the icon still gets
# pulled for alerts and onboarding-window title bars.
if [[ -f "$ICON_SRC" ]]; then
    ICONSET="target/release/utter.iconset"
    rm -rf "$ICONSET"
    mkdir -p "$ICONSET"
    sips -z 16  16   "$ICON_SRC" --out "$ICONSET/icon_16x16.png"    >/dev/null
    sips -z 32  32   "$ICON_SRC" --out "$ICONSET/icon_16x16@2x.png" >/dev/null
    sips -z 32  32   "$ICON_SRC" --out "$ICONSET/icon_32x32.png"    >/dev/null
    sips -z 64  64   "$ICON_SRC" --out "$ICONSET/icon_32x32@2x.png" >/dev/null
    sips -z 128 128  "$ICON_SRC" --out "$ICONSET/icon_128x128.png"    >/dev/null
    sips -z 256 256  "$ICON_SRC" --out "$ICONSET/icon_128x128@2x.png" >/dev/null
    sips -z 256 256  "$ICON_SRC" --out "$ICONSET/icon_256x256.png"    >/dev/null
    sips -z 512 512  "$ICON_SRC" --out "$ICONSET/icon_256x256@2x.png" >/dev/null
    sips -z 512 512  "$ICON_SRC" --out "$ICONSET/icon_512x512.png"    >/dev/null
    sips -z 1024 1024 "$ICON_SRC" --out "$ICONSET/icon_512x512@2x.png" >/dev/null
    iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/utter.icns"
    rm -rf "$ICONSET"
else
    echo "warning: $ICON_SRC not found; bundle will have no icon" >&2
fi

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>
    <string>com.utter.app</string>
    <key>CFBundleExecutable</key>
    <string>utter</string>
    <key>CFBundleIconFile</key>
    <string>utter</string>
    <key>CFBundleName</key>
    <string>utter</string>
    <key>CFBundleDisplayName</key>
    <string>utter</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>LSMinimumSystemVersion</key>
    <string>13.0</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSMicrophoneUsageDescription</key>
    <string>utter transcribes your dictation locally on this device. Audio never leaves your machine.</string>
</dict>
</plist>
PLIST

# Re-sign the whole bundle. Without this step the embedded Info.plist
# isn't sealed into the signature and LaunchServices can refuse to
# treat the bundle as a proper app.
#
# Default is ad-hoc (`-`), which is fine to launch but produces an
# unstable Designated Requirement: the cdhash changes on every build,
# so TCC grants (Microphone, Input Monitoring, Accessibility) reset on
# every rebuild AND `IOHIDRequestAccess` silently fails to surface the
# Input Monitoring prompt for ad-hoc bundles.
#
# Override with a real identity for stable TCC behavior:
#   UTTER_SIGN_IDENTITY="Apple Development: You (TEAMID)" ./scripts/make-bundle.sh
#   UTTER_SIGN_IDENTITY="Developer ID Application: You (TEAMID)" ./scripts/make-bundle.sh
# List installed identities with: security find-identity -v -p codesigning
SIGN_IDENTITY="${UTTER_SIGN_IDENTITY:--}"
ENTITLEMENTS="packaging/macos/entitlements.plist"
if [[ "$SIGN_IDENTITY" == "-" ]]; then
    codesign --force --sign - "$APP"
    echo "signed ad-hoc — TCC grants will reset on rebuild" >&2
    echo "  set UTTER_SIGN_IDENTITY to a real identity for stable grants" >&2
else
    # --options runtime enables Hardened Runtime (required for
    # notarization, recommended otherwise). HR blocks protected
    # resources at a layer above TCC unless the bundle declares
    # matching entitlements — without audio-input here, the mic
    # is blocked even when the user has allowed utter in Settings.
    codesign --force --sign "$SIGN_IDENTITY" \
        --options runtime \
        --entitlements "$ENTITLEMENTS" \
        "$APP"
    echo "signed with: $SIGN_IDENTITY (entitlements: $ENTITLEMENTS)" >&2
fi

# Register with LaunchServices so \`open\` and TCC find the bundle by its
# CFBundleIdentifier. Safe to re-run; picks up the latest plist.
/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister \
    -f "$APP"

echo "built $APP (CFBundleIdentifier=com.utter.app, version=${VERSION})"
echo
echo "Run via: open \"$APP\" --args <subcommand>"
