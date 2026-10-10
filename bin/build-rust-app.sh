#!/usr/bin/env bash
# build-rust-app.sh — assemble the .app from the Rust crate (see PLAN-rust-port.md).
#
# Mirrors bin/build-app.sh: same bundle layout, Info.plist generation, icon,
# resource bundling (--dist), signing and TCC flow. The compiler is cargo, not
# swiftc. SwiftTerm linking + the shim land in Phase 0.7.
set -uo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$DIR/.." && pwd)"
. "$ROOT/install.conf"

# NOTE: do NOT export MACOSX_DEPLOYMENT_TARGET=$MACOS_MIN (26.7) here — the
# Homebrew rustc 1.92 produces a corrupt proc-macro dylib for that target.
# The SwiftTerm min-OS is handled at link time instead (see rust/.cargo/config.toml).

DIST=0
[ "${1:-}" = "--dist" ] && DIST=1
# During the port the Rust app is built to an ISOLATED bundle so it never
# clobbers or kills the live Swift app at $ROOT/$APP_NAME.app. The cutover
# (Phase 4) flips this to the real path.
APP="$ROOT/.build/rust-app/$APP_NAME.app"
[ "$DIST" = 1 ] && APP="$ROOT/.build/dist/$APP_NAME.app"
BIN="$APP/Contents/MacOS/$APP_NAME"
CRATE="$ROOT/rust/ws-rs"
CARGO_TARGET_DIR="$ROOT/.build/rust-target"
export CARGO_TARGET_DIR

"$DIR/check-macos.sh" || exit 1
TMP="${TMPDIR:-/tmp}"

stale() {
    [ -x "$BIN" ] || return 0
    local bin_ts src_ts
    bin_ts="$(stat -f %m "$BIN" 2>/dev/null || echo 0)"
    src_ts="$(find "$CRATE/src" "$CRATE/Cargo.toml" -newermt "@$bin_ts" 2>/dev/null | head -1)"
    [ -n "$src_ts" ] && return 0
    return 1
}

case "${1:-}" in
    --stale) stale; exit $? ;;
    --force|--dist|"") ;;
    *) echo "usage: build-rust-app.sh [--force|--stale|--dist]" >&2; exit 2 ;;
esac

if [ "$DIST" = 1 ]; then rm -rf "$APP"; fi
mkdir -p "$(dirname "$BIN")" "$APP/Contents/Resources"

BUILD_TMP="$(mktemp "$TMP/ws-rust.XXXXXX")" || exit 1
LOG="$BUILD_TMP.log"
PLIST="$BUILD_TMP.plist"
trap 'rm -f "$BUILD_TMP" "$PLIST" "$LOG"' EXIT

echo "build-rust-app: cargo build --release (target $CARGO_TARGET_DIR)"
if ! (cd "$CRATE" && cargo build --release) >"$LOG" 2>&1; then
    grep -E 'error(\[|:)' -A4 "$LOG" >&2 || cat "$LOG" >&2
    echo "build-rust-app: cargo build failed (log: $LOG)" >&2
    exit 1
fi
cp "$CARGO_TARGET_DIR/release/$APP_NAME" "$BUILD_TMP" || { echo "build-rust-app: no binary at $CARGO_TARGET_DIR/release/$APP_NAME" >&2; exit 1; }

cp "$ROOT/Info.plist" "$PLIST" || { echo "build-rust-app: cannot read $ROOT/Info.plist" >&2; exit 1; }
pl_set() {
    /usr/libexec/PlistBuddy -c "Add :$1 $2 $3" "$PLIST" >/dev/null 2>&1 \
        || /usr/libexec/PlistBuddy -c "Set :$1 $3" "$PLIST" >/dev/null 2>&1
}
pl_set CFBundleShortVersionString string "$APP_VERSION"
pl_set CFBundleVersion string "$APP_VERSION"
pl_set LSMinimumSystemVersion string "$MACOS_MIN"
pl_set CFBundleIconFile string AppIcon

# NOTE (Phase 0.8 follow-up): swift builds embed Info.plist into __TEXT,__info_plist
# so config-schema/config-check run before AppKit. The cargo build needs a build.rs
# rustc-link-arg for that; tracked in PLAN-rust-port.md.

build_icon() {
    local src="$ROOT/app_icon.png" out="$APP/Contents/Resources/AppIcon.icns" set n
    [ -f "$src" ] || return 0
    [ -f "$out" ] && [ ! "$src" -nt "$out" ] && return 0
    set="$(mktemp -d "$TMP/ws-icon.XXXXXX")/AppIcon.iconset"
    mkdir -p "$set"
    for n in 16 32 128 256; do
        sips -z "$n" "$n" "$src" --out "$set/icon_${n}x${n}.png" >/dev/null 2>&1
        sips -z $((n * 2)) $((n * 2)) "$src" --out "$set/icon_${n}x${n}@2x.png" >/dev/null 2>&1
    done
    sips -z 512 512 "$src" --out "$set/icon_512x512.png" >/dev/null 2>&1
    iconutil -c icns "$set" -o "$out" >/dev/null 2>&1
    rm -rf "$(dirname "$set")"
}

bundle_resources() {
    local res="$APP/Contents/Resources" d f src
    for d in $RESOURCE_LINK_DIRS $RESOURCE_SEED_DIRS; do
        [ "$d" = bin ] && continue
        [ -d "$ROOT/$d" ] || continue
        mkdir -p "$res/$d"
        (cd "$ROOT" && git ls-files -co --exclude-standard -- "$d") | while IFS= read -r f; do
            mkdir -p "$res/$(dirname "$f")"
            cp -p "$ROOT/$f" "$res/$f"
        done
    done
    mkdir -p "$res/bin"
    for f in $RESOURCE_BIN; do cp -p "$ROOT/bin/$f" "$res/bin/$f" || return 1; done
    for f in $RESOURCE_FILES; do cp -p "$ROOT/$f" "$res/$f" || return 1; done
    awk '
        /^\[/ { sec = $0 }
        sec == "[notes]" && /^paths[ \t]*=/ {
            print "paths = \"~/notes, ~/.config/kitchen-sink/commands.toml\""; next }
        sec == "[notes]" && /^pdf-css[ \t]*=/ { print "pdf-css = \"\""; next }
        /^config[ \t]*=.*fake\.json/ { next }
        /^vim-bin[ \t]*=/ { print "vim-bin = \"nvim\""; next }
        sec == "[jira]" && /^enabled[ \t]*=/ { print "enabled = false"; next }
        sec == "[screenshot]" && /^contrast-opacity[ \t]*=/ { print "contrast-opacity = 190"; next }
        sec == "[screenshot]" && /^draw-color[ \t]*=/ { print "draw-color = \"#ff0000\""; next }
        sec == "[screenshot]" && /^save-path[ \t]*=/ { print "save-path = \"~/Desktop\""; next }
        { print }' "$ROOT/commands.toml" > "$res/commands.default.toml"
    printf '%s\n' "$APP_VERSION" > "$res/VERSION"
    mkdir -p "$res/helpers-bin"
    for src in $HELPER_SOURCES; do
        swiftc -O -target "$(uname -m)-apple-macosx$MACOS_MIN" "$ROOT/$src" \
            -o "$res/helpers-bin/$(basename "$src" .swift)" >>"$LOG" 2>&1 || {
                cat "$LOG" >&2; echo "build-rust-app: helper $src failed" >&2; return 1; }
    done
    find "$res" -name '__pycache__' -type d -prune -exec rm -rf {} +
    find "$res" -name '.DS_Store' -delete
}

[ "$DIST" = 1 ] || { [ "${WS_RUST_KILL:-0}" = 1 ] && pkill -f "$APP_NAME.app/Contents/MacOS" 2>/dev/null || true; }
mv "$BUILD_TMP" "$BIN"
chmod 755 "$BIN"
cp "$PLIST" "$APP/Contents/Info.plist"
rm -f "$PLIST"
build_icon
[ "$DIST" = 1 ] && { bundle_resources || exit 1; }

clear_cstemp() { rm -f "$APP/Contents/MacOS/"*.cstemp; }
clear_cstemp
if [ "$DIST" = 1 ] && [ -n "${DEVELOPER_ID:-}" ]; then
    for f in "$APP/Contents/Resources/helpers-bin/"*; do
        codesign --force --options runtime --timestamp --sign "$DEVELOPER_ID" "$f" || exit 1
    done
    codesign --force --options runtime --timestamp --sign "$DEVELOPER_ID" \
        --identifier "$BUNDLE_ID" --entitlements "$ROOT/entitlements.plist" "$APP" || {
            echo "build-rust-app: Developer ID signing failed ($DEVELOPER_ID)" >&2; exit 1; }
    exit 0
fi
perl -e 'alarm 30; exec @ARGV' codesign --force --sign "$SIGN_ID" --identifier "$BUNDLE_ID" "$APP" >/dev/null 2>&1 \
    || { echo "build-rust-app: codesign with '$SIGN_ID' failed — run 'ws permissions fix'" >&2; exit 1; }
clear_cstemp

[ "$DIST" = 1 ] && exit 0
"$DIR/grant-permissions.sh" "$BUNDLE_ID" "$APP" >/dev/null 2>&1 || true
exit 0
