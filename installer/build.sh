#!/usr/bin/env bash
# build.sh — build the click-through installer app (SwiftUI wizard).
# Produces Installer.app in the repo root.
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$DIR/.." && pwd)"
APP="$ROOT/Installer.app"
set -e
mkdir -p "$APP/Contents/MacOS"
swiftc -O -swift-version 5 \
    -Xlinker -sectcreate -Xlinker __TEXT -Xlinker __info_plist -Xlinker "$DIR/Info.plist" \
    "$DIR/main.swift" \
    -o "$APP/Contents/MacOS/Installer"
codesign --force --sign - --identifier dev.danielbaker.workspace-installer "$APP"
echo "Installer.app built: $APP"