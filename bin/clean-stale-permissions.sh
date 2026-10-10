#!/usr/bin/env bash
set -u
OLD_ID="dev.danielbaker.workspace-switcher"
LSR="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"

reset_all() { tccutil reset All "$OLD_ID" >/dev/null 2>&1; }

if reset_all; then
    echo "clean-stale-permissions: cleared stale grants for $OLD_ID"
    exit 0
fi

TMP="$HOME/workspace-switcher"
if [ -e "$TMP" ]; then
    echo "clean-stale-permissions: $TMP already exists — not touching it" >&2
    exit 0
fi
trap 'rm -rf "$TMP"' EXIT
APP="$TMP/workspace-switcher.app"
mkdir -p "$APP/Contents/MacOS"
cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>$OLD_ID</string>
<key>CFBundleExecutable</key><string>workspace-switcher</string>
<key>CFBundleName</key><string>workspace-switcher</string>
<key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>
EOF
cp /bin/echo "$APP/Contents/MacOS/workspace-switcher"
codesign --force --sign - "$APP" >/dev/null 2>&1 || true
"$LSR" -f "$APP" >/dev/null 2>&1 || true
if reset_all; then
    echo "clean-stale-permissions: cleared stale grants for $OLD_ID"
    "$LSR" -u "$APP" >/dev/null 2>&1 || true
    rm -rf "$TMP"
    exit 0
fi
echo "clean-stale-permissions: could not reach the privacy database (nothing cleaned)" >&2
"$LSR" -u "$APP" >/dev/null 2>&1 || true
rm -rf "$TMP"
exit 1
