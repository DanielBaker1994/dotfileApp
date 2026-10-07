#!/usr/bin/env bash
# clean-stale-permissions.sh — remove Privacy (TCC) grants left behind by the
# app's PREVIOUS name: "workspace-switcher" (bundle id
# dev.danielbaker.workspace-switcher). After the rename to kitchen-sink, macOS
# keeps those rows, so an upgraded machine still shows the old app in System
# Settings ▸ Privacy & Security (Accessibility, Screen Recording, Full Disk
# Access, Input Monitoring) and looks like it is asking for the same
# permissions again.
#
# tccutil can only reset an app it can resolve through LaunchServices, and the
# old app is usually long deleted — so, when it is gone, we briefly recreate a
# throwaway bundle carrying the old id, reset every service, then remove it.
#
# Safe to run any time; a no-op when there is nothing to clean.
#   bin/clean-stale-permissions.sh
set -u
OLD_ID="dev.danielbaker.workspace-switcher"
LSR="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"

reset_all() { tccutil reset All "$OLD_ID" >/dev/null 2>&1; }

# 1. the old app may still exist somewhere → tccutil can target it directly
if reset_all; then
    echo "clean-stale-permissions: cleared stale grants for $OLD_ID"
    exit 0
fi

# 2. it's gone → make the id resolvable with a throwaway bundle, reset, remove.
#    tccutil only resolves apps in a real home location (a /tmp or
#    /var/folders bundle registers in LaunchServices but is NOT found by
#    tccutil), so recreate the app's old install path.
TMP="$HOME/workspace-switcher"
if [ -e "$TMP" ]; then
    echo "clean-stale-permissions: $TMP already exists — not touching it" >&2
    exit 0
fi
# an interrupt between here and the final rm must not leave the throwaway behind
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
